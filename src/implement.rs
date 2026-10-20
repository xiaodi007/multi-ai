//! Explicit, single-writer implementation through a fixed Codex command.
//! Codex workspace-write provides the execution boundary; Git and recursive mtime
//! snapshots check observed local changes afterward, not remote or unmonitored writes.
use crate::{
    args::Args,
    config::{Config, Route},
    review,
    runtime::{self, Attempt, OutputContext, OutputLock},
};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

#[cfg(test)]
#[path = "implement_tests.rs"]
mod tests;

#[derive(Serialize)]
struct GitSnapshot {
    head: Option<String>,
    status_short: String,
    status_porcelain: String,
    dirty_files: Vec<String>,
    files: BTreeMap<String, Value>,
}
impl GitSnapshot {
    fn summary(&self) -> Result<Value> {
        Ok(json!({"head":self.head,"status_short":self.status_short,
            "status_porcelain":self.status_porcelain,"dirty_files":self.dirty_files,
            "files_sha256":runtime::sha256(&serde_json::to_vec(&self.files)?)}))
    }
}

fn git(cwd: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("/usr/bin/git")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", runtime::home())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .context("cannot inspect Git working tree")?;
    if !output.status.success() {
        bail!(
            "Git inspection failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}
fn git_path(cwd: &Path, option: &str) -> Result<PathBuf> {
    let bytes = git(cwd, &["rev-parse", option])?;
    let path = PathBuf::from(String::from_utf8(bytes)?.trim());
    fs::canonicalize(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
    .context("cannot resolve Git metadata directory")
}
fn snapshot(cwd: &Path) -> Result<GitSnapshot> {
    let head = git(cwd, &["rev-parse", "--verify", "HEAD"])
        .ok()
        .map(String::from_utf8)
        .transpose()?
        .map(|s| s.trim().to_owned());
    let status_short =
        String::from_utf8(git(cwd, &["status", "--short", "--untracked-files=all"])?)?;
    let status_porcelain = String::from_utf8(git(
        cwd,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )?)?;
    let raw = git(
        cwd,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let mut rows = raw.split(|b| *b == 0).filter(|r| !r.is_empty());
    let mut dirty_files = BTreeSet::new();
    while let Some(row) = rows.next() {
        if row.len() < 4 {
            bail!("malformed git status");
        }
        dirty_files.insert(String::from_utf8(row[3..].to_vec())?);
        if (row[0] == b'R' || row[0] == b'C' || row[1] == b'R' || row[1] == b'C')
            && let Some(source) = rows.next()
        {
            dirty_files.insert(String::from_utf8(source.to_vec())?);
        }
    }
    let mut files = BTreeMap::new();
    let names = git(
        cwd,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?;
    for bytes in names.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let name = String::from_utf8(bytes.to_vec()).context("Git path must be UTF-8")?;
        let path = cwd.join(&name);
        let value = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => json!({
                "type":"symlink","sha256":runtime::sha256(fs::read_link(&path)?.as_os_str().as_encoded_bytes()),
                "mode":metadata.permissions().mode() & 0o777}),
            Ok(metadata) if metadata.is_file() => {
                if !fs::canonicalize(&path)?.starts_with(cwd) {
                    bail!("tracked path resolves outside implementation cwd: {name}");
                }
                json!({"type":"file","sha256":runtime::sha256(&fs::read(&path)?),
                    "mode":metadata.permissions().mode() & 0o777})
            }
            Ok(_) => json!({"type":"directory"}),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Null,
            Err(error) => return Err(error.into()),
        };
        files.insert(name, value);
    }
    Ok(GitSnapshot {
        head,
        status_short,
        status_porcelain,
        dirty_files: dirty_files.into_iter().collect(),
        files,
    })
}
fn changed_files(before: &GitSnapshot, after: &GitSnapshot) -> Vec<String> {
    before
        .files
        .keys()
        .chain(after.files.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|name| before.files.get(*name) != after.files.get(*name))
        .cloned()
        .collect()
}

// Metadata only: never read the contents of sensitive directories or follow nested symlinks.
#[derive(Serialize, PartialEq, Eq)]
struct MtimeEntry {
    kind: &'static str,
    mtime_seconds: i64,
    mtime_nanoseconds: i64,
    length: u64,
}
#[derive(Serialize)]
struct BoundaryError {
    path: PathBuf,
    error: String,
}
#[derive(Serialize)]
struct BoundarySnapshot {
    roots: BTreeSet<PathBuf>,
    entries: BTreeMap<PathBuf, MtimeEntry>,
    errors: Vec<BoundaryError>,
    excluded_artifacts: PathBuf,
    excluded_worktrees: BTreeSet<PathBuf>,
    elapsed_ms: u128,
}
impl BoundarySnapshot {
    fn summary(&self) -> Result<Value> {
        Ok(
            json!({"roots":self.roots,"entries":self.entries.len(),"elapsed_ms":self.elapsed_ms,"errors":self.errors,
            "excluded_artifacts":[self.excluded_artifacts],
            "excluded_worktrees":self.excluded_worktrees,
            "pruned_directories":["node_modules","target","dist",".git/objects",".claude/skills/synced"],
            "snapshot_sha256":runtime::sha256(&serde_json::to_vec(self)?)}),
        )
    }
}
fn scan_mtimes(path: &Path, cwd: &Path, snapshot: &mut BoundarySnapshot) {
    if (path.starts_with(cwd) && !path.starts_with(cwd.join(".git")))
        || path.starts_with(&snapshot.excluded_artifacts)
        || snapshot
            .excluded_worktrees
            .iter()
            .any(|root| path.starts_with(root))
        || matches!(
            path.file_name().and_then(|n| n.to_str()),
            Some("node_modules" | "target" | "dist")
        )
        || path.ends_with(".git/objects")
        || path.ends_with(".claude/skills/synced")
    {
        return;
    }
    let mut scan = || -> Result<()> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let kind = if metadata.file_type().is_symlink() {
            "symlink"
        } else if metadata.is_dir() {
            "directory"
        } else {
            "file"
        };
        // Directory mtimes change when an excluded child appears; only compare leaf entries.
        if !metadata.is_dir() {
            snapshot.entries.insert(
                path.to_path_buf(),
                MtimeEntry {
                    kind,
                    mtime_seconds: metadata.mtime(),
                    mtime_nanoseconds: metadata.mtime_nsec(),
                    length: metadata.len(),
                },
            );
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                scan_mtimes(&entry?.path(), cwd, snapshot);
            }
        }
        Ok(())
    };
    if let Err(error) = scan() {
        snapshot.errors.push(BoundaryError {
            path: path.to_path_buf(),
            error: format!("{error:#}"),
        });
    }
}
fn boundary_snapshot(
    cwd: &Path,
    previous: &BTreeSet<PathBuf>,
    artifacts: &Path,
    since: i64,
) -> BoundarySnapshot {
    let clock = Instant::now();
    let mut snapshot = BoundarySnapshot {
        elapsed_ms: 0,
        roots: previous.clone(),
        entries: BTreeMap::new(),
        errors: Vec::new(),
        excluded_artifacts: artifacts.to_path_buf(),
        excluded_worktrees: concurrent_worktrees(since),
    };
    snapshot.excluded_worktrees.remove(cwd);
    let claude = runtime::home().join(".claude");
    let mut sensitive = vec![
        runtime::home().join(".agents"),
        claude.join("skills"),
        claude.join("CLAUDE.md"),
        claude.join("settings.json"),
        claude.join("settings.local.json"),
    ];
    if let Ok(entries) = fs::read_dir(&claude) {
        sensitive.extend(entries.flatten().map(|e| e.path()).filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("settings") && n.ends_with(".json"))
        }));
    }
    for path in sensitive {
        snapshot.roots.insert(path.clone());
        // A designated root may itself be a symlink; also inspect its resolved tree.
        if let Ok(resolved) = fs::canonicalize(&path) {
            snapshot.roots.insert(resolved);
        }
    }
    // Only the repository's registered worktrees, never arbitrary sibling repositories.
    match git(cwd, &["worktree", "list", "--porcelain", "-z"]) {
        Ok(bytes) => {
            for field in bytes.split(|b| *b == 0) {
                if let Some(path) = field.strip_prefix(b"worktree ") {
                    snapshot
                        .roots
                        .insert(PathBuf::from(String::from_utf8_lossy(path).as_ref()));
                }
            }
        }
        Err(error) => snapshot.errors.push(BoundaryError {
            path: cwd.into(),
            error: error.to_string(),
        }),
    }
    if let Ok(common) = git_path(cwd, "--git-common-dir") {
        snapshot.roots.insert(common);
    }
    for path in snapshot.roots.clone() {
        scan_mtimes(&path, cwd, &mut snapshot);
    }
    snapshot.elapsed_ms = clock.elapsed().as_millis();
    snapshot
}
fn boundary_changes(before: &BoundarySnapshot, after: &BoundarySnapshot) -> Vec<PathBuf> {
    before
        .entries
        .keys()
        .chain(after.entries.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| {
            !before.excluded_worktrees.iter().chain(&after.excluded_worktrees).any(|root|path.starts_with(root))
                && before.entries.get(*path) != after.entries.get(*path)
                // Missing entries below an unreadable subtree are unknown, not deletions.
                && (after.entries.contains_key(*path)
                    || !after.errors.iter().any(|error| path.starts_with(&error.path)))
        })
        .cloned()
        .collect()
}

// Durable intervals also cover a concurrent task that finishes before our after-snapshot.
// A held, close-on-exec lock distinguishes live registrations from crashed old processes.
struct Registration {
    path: PathBuf,
    record: Value,
    _file: File,
}
impl Registration {
    fn new(cwd: &Path) -> Result<Self> {
        let root = runtime::state_dir().join("implementations");
        runtime::private_dir(&root)?;
        let path = root.join(format!("{}.json", uuid::Uuid::new_v4()));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC)
            .open(path.with_extension("lock"))?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            bail!("cannot register implementation");
        }
        let record = json!({"cwd":cwd,"started_ms":chrono::Utc::now().timestamp_millis(),"finished_ms":null});
        runtime::atomic_json(&path, &record)?;
        Ok(Self {
            path,
            record,
            _file: file,
        })
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.record["finished_ms"] = json!(chrono::Utc::now().timestamp_millis());
        let _ = runtime::atomic_json(&self.path, &self.record);
    }
}
fn concurrent_worktrees(since: i64) -> BTreeSet<PathBuf> {
    let mut roots = BTreeSet::new();
    let Ok(entries) = fs::read_dir(runtime::state_dir().join("implementations")) else {
        return roots;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|v| v != "json")
            || !entry.file_type().is_ok_and(|m| m.is_file())
        {
            continue;
        }
        let Some(record) = fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        else {
            continue;
        };
        let active = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path.with_extension("lock"))
            .is_ok_and(
                |f| unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0,
            );
        if (active
            || record["started_ms"].as_i64().is_some_and(|n| n >= since)
            || record["finished_ms"].as_i64().is_some_and(|n| n >= since))
            && let Some(cwd) = record["cwd"].as_str()
        {
            roots.insert(PathBuf::from(cwd));
        }
    }
    roots
}
fn controlled_route(route: &Route, codex: &Path, cwd: &Path, network: bool) -> Route {
    let mut controlled = route.clone();
    let mut argv = vec![
        codex.to_string_lossy().into_owned(),
        "exec".into(),
        "--json".into(),
        "-C".into(),
        cwd.to_string_lossy().into_owned(),
        "-s".into(),
        "workspace-write".into(),
        "--skip-git-repo-check".into(),
    ];
    if network {
        argv.extend([
            "-c".into(),
            "sandbox_workspace_write.network_access=true".into(),
        ]);
    }
    if let Some(model) = route.explicit_model() {
        argv.extend(["--model".into(), model]);
    }
    argv.extend(["-o".into(), "{outfile}".into(), "{prompt}".into()]);
    controlled.argv = argv;
    controlled.agentic = true;
    controlled.api_alias = None;
    controlled.extract = "last-message-file".into();
    controlled
}
fn prepend_paths(config: &Config, args: &Args) -> Result<Vec<PathBuf>> {
    args.all("path-prepend")
        .iter()
        .map(|path| {
            let path = if path == "~" || path.starts_with("~/") {
                PathBuf::from(
                    config
                        .environment()
                        .get("HOME")
                        .context("HOME unavailable for ~ expansion")?,
                )
                .join(path.strip_prefix("~/").unwrap_or(""))
            } else {
                PathBuf::from(path)
            };
            let path = fs::canonicalize(path).context("--path-prepend directory does not exist")?;
            if !path.is_dir() {
                bail!("--path-prepend must be a directory");
            }
            Ok(path)
        })
        .collect()
}

pub async fn command(config: &Config, args: &Args) -> Result<i32> {
    let mut defaults = args.clone();
    if !defaults.has("task-type") {
        defaults.set("task-type", "implement");
    }
    let args = &defaults;
    if args.positional != ["implement", "codex-cli"] {
        bail!("implementation requires delegate implement codex-cli");
    }
    for option in ["resume", "fallback", "outdir"] {
        if args.has(option) {
            bail!("implementation does not support --{option}");
        }
    }
    let cwd = fs::canonicalize(args.get("cwd").context("implementation requires --cwd")?)?;
    if !cwd.is_dir() {
        bail!("--cwd must be a directory");
    }
    let output = runtime::normalize_path(Path::new(
        args.get("out").context("implementation requires --out")?,
    ))?;
    if output.starts_with(&cwd) {
        bail!("implementation --out must be outside --cwd");
    }
    if runtime::normalize_path(&runtime::state_dir())?.starts_with(&cwd) {
        bail!("implementation state directory must be outside --cwd");
    }
    let seconds = args
        .get("timeout")
        .map(str::parse::<f64>)
        .transpose()?
        .unwrap_or(900.0);
    if !seconds.is_finite() || seconds <= 0.0 {
        bail!("--timeout must be positive seconds");
    }
    let _workspace_lock = OutputLock::acquire(&[(cwd.clone(), true)])?;
    let ctx = OutputContext::new(args, "implement", false)?;
    let registration = Registration::new(&cwd)?;
    let started = Instant::now();
    let attempt = ctx.attempt("codex-cli")?;
    attempt
        .update(json!({"route":"codex-cli","attempt_id":attempt.id,"cwd":cwd,"model_calls":0,"network":args.has("network")}))?;
    let operation = execute(
        config,
        args,
        &cwd,
        &attempt,
        seconds,
        started,
        registration.record["started_ms"].as_i64().unwrap(),
    )
    .await;
    let mut result = match operation {
        Ok(result) => result,
        Err(error) => json!({"status":"error","complete":false,"error":format!("{error:#}")}),
    };
    result["route"] = json!("codex-cli");
    result["run_id"] = json!(ctx.run_id);
    result["attempt_id"] = json!(attempt.id);
    result["artifacts"] = json!(attempt.dir);
    result["cwd"] = json!(cwd);
    result["seconds"] = json!(started.elapsed().as_secs_f64());
    result["automatic_commit"] = json!(false);
    result["network"] = json!(args.has("network"));
    result["task_type"] = json!(ctx.task_type);
    result["summary_published"] = json!(false);
    result["summary_kind"] = json!(if result["status"] == "ok" {
        "final"
    } else {
        "diagnostic"
    });
    let mut rc = 1;
    if result["status"] == "ok" {
        result["summary_published"] = json!(true);
        let summary = result["output"].as_str().unwrap_or("");
        ctx.report(&attempt, summary, &result)?;
        ctx.publish(summary)?;
        rc = 0;
    } else if result["status"] == "boundary_violation"
        && result["output"].as_str().is_some_and(|s| !s.is_empty())
    {
        result["summary_published"] = json!(true);
        ctx.report(&attempt, result["output"].as_str().unwrap(), &result)?;
        ctx.publish(result["output"].as_str().unwrap())?;
        attempt.finish("boundary_violation", review::without_output(&result))?;
    } else {
        attempt.finish(
            result["status"].as_str().unwrap_or("error"),
            review::without_output(&result),
        )?;
    }
    ctx.save_status(review::without_output(&result))?;
    ctx.finish(rc)?;
    if crate::stats::has_boundary_warnings(&result) {
        eprintln!(
            "[codex-cli] boundary warnings (unattributed); details={}",
            result["boundary_details"].as_str().unwrap_or("unknown")
        );
    }
    if args.has("json") {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if rc == 0 {
        println!("Wrote {}", output.display());
    } else {
        eprintln!(
            "[codex-cli] {}: {}",
            result["status"].as_str().unwrap_or("error"),
            result["error"].as_str().unwrap_or("")
        );
    }
    eprintln!(
        "multi-ai: status={} exit={} run={} attempt={}",
        result["status"].as_str().unwrap_or("error"),
        rc,
        ctx.run_id,
        attempt.id
    );
    Ok(rc)
}

async fn execute(
    config: &Config,
    args: &Args,
    cwd: &Path,
    attempt: &Attempt,
    seconds: f64,
    started: Instant,
    since: i64,
) -> Result<Value> {
    let root = git_path(cwd, "--show-toplevel")?;
    if root != cwd {
        bail!("implementation --cwd must be the Git working-tree root");
    }
    let before = snapshot(cwd)?;
    attempt.write("git-before.json", &serde_json::to_vec_pretty(&before)?)?;
    let before_summary = before.summary()?;
    attempt.update(json!({"git_before":before_summary,"preexisting_files":before.dirty_files}))?;
    if !args.has("allow-dirty") && !before.status_short.trim().is_empty() {
        return Ok(
            json!({"status":"dirty","complete":false,"error":"working tree contains uncommitted changes; use --allow-dirty explicitly","git_before":before_summary,"preexisting_files":before.dirty_files,"changed_files":[]}),
        );
    }
    let config = config.with_path_prepend(&prepend_paths(config, args)?)?;
    let mut route = config.route("codex-cli", false)?;
    let mut identity_args = args.clone();
    identity_args.set("cwd", cwd.to_string_lossy());
    let (identity, model_identity_known) = review::cli_identity(&route, &config, &identity_args)?;
    let route_revision = crate::scorecard::revision(&route, &identity);
    attempt.update(
        json!({"route_revision":route_revision,"model_identity_known":model_identity_known}),
    )?;
    let task = fs::read_to_string(
        args.get("prompt-file")
            .context("implementation requires --prompt-file")?,
    )?;
    if task.trim().is_empty() {
        bail!("implementation brief must not be empty");
    }
    let prompt = format!(
        "在指定工作树 {} 内实施下列任务。先阅读并遵守工作树内 AGENTS.md。只允许修改该目录内的文件，不得提交、修改 Git 元数据、运行其他实施器或写入外部路径。最终报告修改内容、验证命令与结果、未完成事项。\n\n{}",
        cwd.display(),
        task
    );
    let mut base = json!({"route_revision":route_revision,"model_identity_known":model_identity_known,"git_before":before_summary,"preexisting_files":before.dirty_files,"changed_files":[],"pool":route.pool,"family":route.family,"inputBytes":prompt.len(),"model":identity["model"],"model_source":identity["model_source"]});
    if let Some(blocked) = review::blocked(&route.pool) {
        let mut result = base;
        result["status"] = json!("cooldown");
        result["complete"] = json!(false);
        result["error"] = json!(format!(
            "pool {} cooldown: {}",
            route.pool,
            blocked["detail"].as_str().unwrap_or("")
        ));
        return Ok(result);
    }
    let Some(codex) = review::resolve_executable("codex", Some(&config)) else {
        let mut result = base;
        result["status"] = json!("missing");
        result["complete"] = json!(false);
        result["error"] = json!("optional Codex CLI executable unavailable on configured PATH");
        return Ok(result);
    };
    attempt.write("last-message.txt", b"")?;
    route = controlled_route(&route, &codex, cwd, args.has("network"));
    let watch = args
        .all("watch")
        .iter()
        .map(|p| runtime::normalize_path(Path::new(p)))
        .collect::<Result<BTreeSet<_>>>()?;
    let boundary_before = boundary_snapshot(cwd, &watch, &attempt.dir, since);
    attempt.write(
        "boundary-before.json",
        &serde_json::to_vec_pretty(&boundary_before)?,
    )?;
    base["boundary_before"] = boundary_before.summary()?;
    attempt.update(json!({"boundary_before":base["boundary_before"]}))?;
    if !boundary_before.errors.is_empty() {
        base["status"] = json!("boundary_snapshot_error");
        base["complete"] = json!(false);
        base["error"] = json!("cannot inspect implementation boundary; Codex was not started");
        return Ok(base);
    }
    let mut execution_args = args.clone();
    execution_args.set("cwd", cwd.to_string_lossy());
    let remaining = seconds - started.elapsed().as_secs_f64();
    if remaining <= 0.0 {
        let mut result = base;
        result["status"] = json!("timeout");
        result["complete"] = json!(false);
        result["error"] =
            json!("total_timeout: implementation preparation exhausted seconds budget");
        return Ok(result);
    }
    execution_args.set("timeout", remaining);
    execution_args.flags.insert("implementation-task".into());
    attempt.update(json!({"pool":route.pool,"family":route.family,"inputBytes":prompt.len(),"model_calls":1,"sandbox":"codex-workspace-write","model":base["model"],"model_source":base["model_source"]}))?;
    let response =
        review::execute_cli_env(&route, &execution_args, &prompt, attempt, Some(&config)).await;
    // Inspect both independently even when the CLI failed or Git metadata was damaged.
    let boundary_after = boundary_snapshot(cwd, &boundary_before.roots, &attempt.dir, since);
    let changes = boundary_changes(&boundary_before, &boundary_after);
    let events = edit_events(&attempt.dir.join("stdout.raw"), cwd);
    let buckets = classify_changes(&changes, &boundary_after.roots);
    let mut violations = events.outside.clone();
    violations.extend(buckets.get("claude_config").into_iter().flatten().cloned());
    violations.sort();
    violations.dedup();
    attempt.write(
        "boundary-changes.json",
        &serde_json::to_vec_pretty(
            &json!({"buckets":buckets,"attributed_edits":events.outside,"violations":violations}),
        )?,
    )?;
    let after = snapshot(cwd);
    attempt.write(
        "boundary-after.json",
        &serde_json::to_vec_pretty(&boundary_after)?,
    )?;
    let mut result = base;
    result["boundary_before"] = boundary_before.summary()?;
    result["boundary_after"] = boundary_after.summary()?;
    result["attribution"] = json!(if events.available {
        "available"
    } else {
        "unavailable"
    });
    result["boundary_details"] = json!(attempt.dir.join("boundary-changes.json"));
    result["boundary_violations"] = json!(violations.iter().take(20).collect::<Vec<_>>());
    result["boundary_violation_count"] = json!(violations.len());
    result["attributed_boundary_count"] = json!(events.outside.len());
    result["boundary_warnings"] = json!(
        buckets
            .iter()
            .filter(|(k, _)| k.as_str() != "claude_config")
            .map(|(k, v)| (
                k,
                json!({"count":v.len(),"samples":v.iter().take(20).collect::<Vec<_>>()})
            ))
            .collect::<BTreeMap<_, _>>()
    );
    result["boundary_buckets"] = json!(
        buckets
            .iter()
            .map(|(k, v)| (
                k,
                json!({"count":v.len(),"samples":v.iter().take(20).collect::<Vec<_>>()})
            ))
            .collect::<BTreeMap<_, _>>()
    );
    if let Some(model) = events.model {
        if result["model"].as_str() != Some(&model) {
            result["route_revision"] = json!(runtime::sha256(&serde_json::to_vec(
                &json!({"configured_revision":result["route_revision"], "observed_model":model})
            )?));
        }
        result["model_identity_known"] = json!(true);
        result["model"] = json!(model);
        result["model_source"] = json!("codex-event");
    }
    match response {
        Ok(value) => {
            result["status"] = value["status"].clone();
            result["complete"] = value["complete"].clone();
            result["error"] = value["error"].clone();
            result["response"] = value;
        }
        Err(error) => {
            let detail = format!("{error:#}");
            result["status"] = json!(review::classify(&detail, 1, false, &prompt));
            result["complete"] = json!(false);
            result["error"] = json!(detail);
        }
    }
    // Preserve quota/auth/transport handling even if post-checks override the final status.
    if matches!(
        result["status"].as_str(),
        Some("quota" | "auth" | "transport")
    ) {
        review::cool_down(
            &route.pool,
            result["status"].as_str().unwrap(),
            result["error"].as_str().unwrap_or(""),
        )?;
    }
    result["execution_status"] = result["status"].clone();
    result["execution_error"] = result["error"].clone();
    match after {
        Ok(after) => {
            attempt.write("git-after.json", &serde_json::to_vec_pretty(&after)?)?;
            let changed = changed_files(&before, &after);
            let changed_preexisting = changed
                .iter()
                .filter(|path| before.dirty_files.contains(path))
                .cloned()
                .collect::<Vec<_>>();
            result["git_after"] = after.summary()?;
            result["changed_files"] = json!(changed);
            result["changed_preexisting_files"] = json!(changed_preexisting);
            result["git_head_changed"] = json!(before.head != after.head);
            if before.head != after.head {
                result["status"] = json!("git_changed");
                result["error"] = json!(
                    "Git HEAD changed during implementation; inspect worktree before continuing"
                );
            }
            if result["status"] == "ok" {
                let files = changed
                    .iter()
                    .map(|path| format!("- `{path}`"))
                    .collect::<Vec<_>>()
                    .join("\n");
                result["output"] = json!(format!(
                    "# 实施摘要\n\n工作树：`{}`\n\n本轮改变的文件：\n{}\n\n既有未提交文件：{}\n\n{}",
                    cwd.display(),
                    if files.is_empty() { "- 无" } else { &files },
                    if before.dirty_files.is_empty() {
                        "无".into()
                    } else {
                        before.dirty_files.join("、")
                    },
                    result["response"]["content"].as_str().unwrap_or("")
                ));
            }
        }
        Err(error) => {
            result["git_after_error"] = json!(format!("{error:#}"));
            if result["status"] == "ok" {
                result["status"] = json!("git_snapshot_error");
                result["error"] = json!("implementation returned but Git inspection failed");
            }
        }
    }
    if !boundary_after.errors.is_empty() && result["status"] == "ok" {
        result["status"] = json!("boundary_snapshot_error");
        result["error"] = json!("implementation returned but boundary inspection was incomplete");
    }
    if !violations.is_empty() {
        result["status"] = json!("boundary_violation");
        result["error"] = json!(
            "changes observed outside implementation cwd; inspect boundary_violations (no rollback performed)"
        );
    }
    if result["status"] != "ok" {
        result["complete"] = json!(false);
        if result["status"] == "boundary_violation"
            && result["execution_status"] == "ok"
            && result["output"].is_string()
        {
            result["output"] = json!(format!(
                "# 警告：观察到工作树外变化（boundary_violation）\n\n请核对 status 的 boundary_violations；本轮未通过边界检查，未回滚。\n\n{}",
                result["output"].as_str().unwrap()
            ));
        } else {
            result["output"] = Value::Null;
        }
    }
    result["outputBytes"] = json!(result["output"].as_str().map_or(0, str::len));
    attempt.finish(
        result["status"].as_str().unwrap_or("error"),
        review::without_output(&result),
    )?;
    Ok(result)
}

#[derive(Default)]
struct EditEvents {
    available: bool,
    outside: Vec<PathBuf>,
    model: Option<String>,
}
// exec JSONL file_change events describe apply_patch edits. Command execution is opaque:
// shell redirections cannot be attributed from this stream, so snapshots remain warnings.
fn edit_events(path: &Path, cwd: &Path) -> EditEvents {
    let mut result = EditEvents::default();
    let mut started = false;
    let mut finished = false;
    let mut malformed = false;
    for line in fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
    {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            malformed = true;
            continue;
        };
        match event["type"].as_str() {
            Some("thread.started") => started = true,
            Some("turn.completed") => finished = true,
            Some("item.completed") if event["item"]["type"] == "file_change" => {
                if let Some(changes) = event["item"]["changes"].as_array() {
                    for change in changes {
                        if let Some(path) = change["path"].as_str() {
                            let path = PathBuf::from(path);
                            let absolute = if path.is_absolute() {
                                path
                            } else {
                                cwd.join(path)
                            };
                            match runtime::normalize_path(&absolute) {
                                Ok(path) if !path.starts_with(cwd) => result.outside.push(path),
                                Ok(_) => (),
                                Err(_) => malformed = true,
                            }
                        } else {
                            malformed = true;
                        }
                    }
                } else {
                    malformed = true;
                }
            }
            _ => (),
        }
        // Some versions emit model metadata; never substitute a model guess.
        if let Some(model) = event["model"].as_str().filter(|m| !m.trim().is_empty()) {
            result.model = Some(model.into());
        }
    }
    result.available = started && finished && !malformed;
    result.outside.sort();
    result.outside.dedup();
    result
}
fn classify_changes(
    changes: &[PathBuf],
    roots: &BTreeSet<PathBuf>,
) -> BTreeMap<String, Vec<PathBuf>> {
    let mut buckets: BTreeMap<String, Vec<PathBuf>> = [
        "git_ignored",
        "git_internal",
        "nested_worktree",
        "sibling_worktree_tracked",
        "claude_config",
        "other",
    ]
    .into_iter()
    .map(|k| (k.into(), Vec::new()))
    .collect();
    let home = runtime::home();
    let designated = |path: &Path| {
        path.starts_with(home.join(".agents"))
            || path.starts_with(home.join(".claude/skills"))
            || (path.parent() == Some(home.join(".claude").as_path())
                && path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                    n == "CLAUDE.md" || (n.starts_with("settings") && n.ends_with(".json"))
                }))
    };
    let resolved_sensitive = roots
        .iter()
        .filter(|p| designated(p))
        .filter_map(|p| fs::canonicalize(p).ok())
        .collect::<Vec<_>>();
    let mut repos = BTreeMap::<PathBuf, Vec<PathBuf>>::new();
    for path in changes {
        let components = path
            .components()
            .map(|p| p.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let sensitive =
            designated(path) || resolved_sensitive.iter().any(|root| path.starts_with(root));
        let bucket = if sensitive {
            Some("claude_config")
        } else if components
            .iter()
            .position(|c| c == ".git")
            .is_some_and(|i| {
                components
                    .get(i + 1)
                    .is_some_and(|c| c != "config" && c != "hooks")
            })
        {
            Some("git_internal")
        } else {
            None
        };
        if let Some(bucket) = bucket {
            buckets.get_mut(bucket).unwrap().push(path.clone());
            continue;
        }
        if let Some(repo) = path.ancestors().skip(1).find(|p| p.join(".git").exists()) {
            repos.entry(repo.into()).or_default().push(path.clone());
        } else {
            buckets.get_mut("other").unwrap().push(path.clone());
        }
    }
    for (repo, paths) in repos {
        let mut ignored = BTreeSet::new();
        for chunk in paths.chunks(128) {
            let mut cmd = Command::new("/usr/bin/git");
            cmd.current_dir(&repo)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", runtime::home())
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_OPTIONAL_LOCKS", "0")
                .args([
                    "-c",
                    "core.fsmonitor=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                    "check-ignore",
                    "-z",
                ]);
            cmd.arg("--stdin")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            let mut input = Vec::new();
            for path in chunk {
                input.extend_from_slice(
                    path.strip_prefix(&repo)
                        .unwrap()
                        .as_os_str()
                        .as_encoded_bytes(),
                );
                input.push(0);
            }
            if let Ok(mut child) = cmd.spawn() {
                let mut stdin = child.stdin.take().unwrap();
                let writer = std::thread::spawn(move || {
                    use std::io::Write;
                    stdin.write_all(&input)
                });
                let output = child.wait_with_output();
                let _ = writer.join();
                if let Ok(out) = output {
                    for name in out.stdout.split(|b| *b == 0).filter(|b| !b.is_empty()) {
                        ignored.insert(repo.join(String::from_utf8_lossy(name).as_ref()));
                    }
                }
            }
        }
        let tracked: BTreeSet<PathBuf> = git(&repo, &["ls-files", "-z"])
            .unwrap_or_default()
            .split(|b| *b == 0)
            .filter(|b| !b.is_empty())
            .map(|b| repo.join(String::from_utf8_lossy(b).as_ref()))
            .collect();
        for path in paths {
            let nested = repo.to_string_lossy().contains("/.claude/worktrees/")
                || roots
                    .iter()
                    .any(|r| repo != *r && repo.starts_with(r) && r.join(".git").exists());
            let bucket = if ignored.contains(&path) {
                "git_ignored"
            } else if nested {
                "nested_worktree"
            } else if tracked.contains(&path) {
                "sibling_worktree_tracked"
            } else {
                "other"
            };
            buckets.get_mut(bucket).unwrap().push(path);
        }
    }
    buckets
}

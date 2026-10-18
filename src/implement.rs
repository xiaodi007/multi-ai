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
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
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
}
impl BoundarySnapshot {
    fn summary(&self) -> Result<Value> {
        Ok(
            json!({"roots":self.roots,"entries":self.entries.len(),"errors":self.errors,
            "excluded_artifacts":[self.excluded_artifacts],
            "snapshot_sha256":runtime::sha256(&serde_json::to_vec(self)?)}),
        )
    }
}
fn scan_mtimes(path: &Path, cwd: &Path, snapshot: &mut BoundarySnapshot) {
    if path.starts_with(cwd) || path.starts_with(&snapshot.excluded_artifacts) {
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
        snapshot.entries.insert(
            path.to_path_buf(),
            MtimeEntry {
                kind,
                mtime_seconds: metadata.mtime(),
                mtime_nanoseconds: metadata.mtime_nsec(),
                length: metadata.len(),
            },
        );
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
) -> BoundarySnapshot {
    let mut snapshot = BoundarySnapshot {
        roots: previous.clone(),
        entries: BTreeMap::new(),
        errors: Vec::new(),
        excluded_artifacts: artifacts.to_path_buf(),
    };
    for name in [".claude", ".agents"] {
        let path = runtime::home().join(name);
        snapshot.roots.insert(path.clone());
        // A designated root may itself be a symlink; also inspect its resolved tree.
        if let Ok(resolved) = fs::canonicalize(&path) {
            snapshot.roots.insert(resolved);
        }
    }
    // Discover again after execution so newly created sibling worktrees are included.
    let siblings = || -> Result<Vec<PathBuf>> {
        let mut roots = Vec::new();
        for entry in fs::read_dir(cwd.parent().context("cwd has no parent")?)? {
            let entry = entry?;
            let path = entry.path();
            if path != cwd && entry.file_type()?.is_dir() && path.join(".git").exists() {
                roots.push(path);
            }
        }
        Ok(roots)
    };
    match siblings() {
        Ok(roots) => snapshot.roots.extend(roots),
        Err(error) => snapshot.errors.push(BoundaryError {
            path: cwd.parent().unwrap_or(cwd).to_path_buf(),
            error: format!("sibling worktrees: {error:#}"),
        }),
    }
    for path in snapshot.roots.clone() {
        scan_mtimes(&path, cwd, &mut snapshot);
    }
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
            before.entries.get(*path) != after.entries.get(*path)
                // Missing entries below an unreadable subtree are unknown, not deletions.
                && (after.entries.contains_key(*path)
                    || !after.errors.iter().any(|error| path.starts_with(&error.path)))
        })
        .cloned()
        .collect()
}
fn controlled_route(route: &Route, codex: &Path, cwd: &Path) -> Route {
    let mut controlled = route.clone();
    let mut argv = vec![
        codex.to_string_lossy().into_owned(),
        "exec".into(),
        "-C".into(),
        cwd.to_string_lossy().into_owned(),
        "-s".into(),
        "workspace-write".into(),
        "--skip-git-repo-check".into(),
    ];
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
    let started = Instant::now();
    let attempt = ctx.attempt("codex-cli")?;
    attempt
        .update(json!({"route":"codex-cli","attempt_id":attempt.id,"cwd":cwd,"model_calls":0}))?;
    let operation = execute(config, args, &cwd, &attempt, seconds, started).await;
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
    let mut rc = 1;
    if result["status"] == "ok" {
        let summary = result["output"].as_str().unwrap_or("");
        ctx.report(&attempt, summary, &result)?;
        ctx.publish(summary)?;
        rc = 0;
    } else {
        attempt.finish(
            result["status"].as_str().unwrap_or("error"),
            review::without_output(&result),
        )?;
    }
    ctx.save_status(review::without_output(&result))?;
    ctx.finish(rc)?;
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
    Ok(rc)
}

async fn execute(
    config: &Config,
    args: &Args,
    cwd: &Path,
    attempt: &Attempt,
    seconds: f64,
    started: Instant,
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
    let mut base = json!({"git_before":before_summary,"preexisting_files":before.dirty_files,"changed_files":[],"pool":route.pool,"family":route.family,"inputBytes":prompt.len(),"model":route.explicit_model(),"model_source":if route.explicit_model().is_some(){"route"}else{"cli-default-unknown"}});
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
    route = controlled_route(&route, &codex, cwd);
    let boundary_before = boundary_snapshot(cwd, &BTreeSet::new(), &attempt.dir);
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
    let boundary_after = boundary_snapshot(cwd, &boundary_before.roots, &attempt.dir);
    let violations = boundary_changes(&boundary_before, &boundary_after);
    let after = snapshot(cwd);
    attempt.write(
        "boundary-after.json",
        &serde_json::to_vec_pretty(&boundary_after)?,
    )?;
    let mut result = base;
    result["boundary_before"] = boundary_before.summary()?;
    result["boundary_after"] = boundary_after.summary()?;
    result["boundary_violations"] = json!(violations);
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
        result["output"] = Value::Null;
    }
    result["outputBytes"] = json!(result["output"].as_str().map_or(0, str::len));
    attempt.finish(
        result["status"].as_str().unwrap_or("error"),
        review::without_output(&result),
    )?;
    Ok(result)
}

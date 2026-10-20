use crate::args::Args;
use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::Instant,
};
use uuid::Uuid;

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}
pub fn state_dir() -> PathBuf {
    std::env::var_os("MULTI_AI_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/state/multi-ai"))
}
pub fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
/// A consumer closing a listing pipe early is a normal command termination.
pub fn stdout_line(args: std::fmt::Arguments<'_>) -> Result<()> {
    let result = (|| -> std::io::Result<()> {
        let mut out = std::io::stdout().lock();
        out.write_fmt(args)?;
        out.write_all(b"\n")
    })();
    match result {
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => other.map_err(Into::into),
    }
}
pub fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
pub fn atomic_text(path: &Path, text: &str) -> Result<()> {
    atomic_bytes(path, text.as_bytes())
}
pub fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    atomic_bytes(path, &serde_json::to_vec_pretty(value)?)
}
fn atomic_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("output has no parent")?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".multi-ai-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
pub fn normalize_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut lexical = PathBuf::new();
    for part in absolute.components() {
        match part {
            std::path::Component::ParentDir => {
                lexical.pop();
            }
            std::path::Component::CurDir => {}
            part => lexical.push(part.as_os_str()),
        }
    }
    let mut existing = lexical.as_path();
    let mut tail = Vec::new();
    while !existing.exists() {
        tail.push(
            existing
                .file_name()
                .context("cannot normalize output")?
                .to_os_string(),
        );
        existing = existing.parent().context("cannot normalize output")?;
    }
    let mut resolved = fs::canonicalize(existing)?;
    for part in tail.into_iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

pub struct OutputLock {
    _files: Vec<File>,
}
impl OutputLock {
    pub fn acquire(targets: &[(PathBuf, bool)]) -> Result<Self> {
        let mut modes = BTreeMap::<PathBuf, bool>::new();
        for (path, exclusive) in targets {
            let path = normalize_path(path)?;
            modes
                .entry(path.clone())
                .and_modify(|v| *v |= *exclusive)
                .or_insert(*exclusive);
            for ancestor in path.ancestors().skip(1) {
                modes.entry(ancestor.to_path_buf()).or_insert(false);
            }
        }
        let locks = state_dir().join("locks");
        private_dir(&locks)?;
        let mut files = Vec::new();
        for (path, exclusive) in modes {
            let name = locks.join(format!(
                "{}.lock",
                sha256(path.as_os_str().as_encoded_bytes())
            ));
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(libc::O_CLOEXEC)
                .open(name)?;
            let mode = if exclusive {
                libc::LOCK_EX
            } else {
                libc::LOCK_SH
            };
            if unsafe { libc::flock(file.as_raw_fd(), mode | libc::LOCK_NB) } != 0 {
                bail!(
                    "output_busy: another managed invocation owns {}",
                    path.display()
                );
            }
            files.push(file);
        }
        Ok(Self { _files: files })
    }
}

#[derive(Clone)]
pub struct OutputContext {
    pub task_type: Option<String>,
    pub canaries: Vec<crate::canary::Canary>,
    pub run_id: String,
    pub dir: PathBuf,
    pub output: Option<PathBuf>,
    pub outdir: Option<PathBuf>,
    pub progress_dir: PathBuf,
    pub excludes: Vec<PathBuf>,
    status_path: Option<PathBuf>,
    _lock: Arc<OutputLock>,
    manifest: Arc<Mutex<Value>>,
    published: Arc<Mutex<Vec<PathBuf>>>,
    attempts: Arc<Mutex<Vec<Attempt>>>,
}
impl OutputContext {
    pub fn new(args: &Args, command: &str, dispatch: bool) -> Result<Self> {
        let canaries = crate::canary::load(args)?;
        let output = args
            .get("out")
            .map(PathBuf::from)
            .map(|p| {
                if fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
                    bail!("output report must not be a symlink: {}", p.display());
                }
                normalize_path(&p)
            })
            .transpose()?;
        let outdir = if dispatch {
            args.get("outdir")
                .map(PathBuf::from)
                .map(|p| normalize_path(&p))
                .transpose()?
        } else {
            None
        };
        if output.as_ref().is_some_and(|p| p.is_dir()) {
            bail!("--out must be a file");
        }
        if outdir.as_ref().is_some_and(|p| p.is_file()) {
            bail!("--outdir must be a directory");
        }
        if let Some(directory) = &outdir {
            let cache = directory.join(".cache");
            if fs::symlink_metadata(&cache).is_ok_and(|m| m.file_type().is_symlink())
                || (cache.exists() && !cache.is_dir())
            {
                bail!(
                    "derived cache directory must be a real directory: {}",
                    cache.display()
                );
            }
            for name in [
                "grok",
                "codex",
                "agy",
                "kimi",
                "kimi-k3",
                "deepseek",
                "glm",
                "kimi-k3-api",
                "claude-ds",
            ] {
                for path in [
                    directory.join(format!("{name}.md")),
                    cache.join(format!("{name}.json")),
                ] {
                    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
                        bail!("managed output must not be a symlink: {}", path.display());
                    }
                }
            }
        }
        let status_path = if let Some(p) = &output {
            Some(PathBuf::from(format!("{}.status.json", p.display())))
        } else {
            outdir.as_ref().map(|p| p.join("dispatch.json"))
        };
        let progress_base = args
            .get("progress-dir")
            .map(PathBuf::from)
            .or_else(|| {
                output
                    .as_ref()
                    .map(|p| PathBuf::from(format!("{}.progress", p.display())))
            })
            .or_else(|| outdir.as_ref().map(|p| p.join("runs")))
            .unwrap_or_else(|| state_dir().join("progress"));
        let progress_base = normalize_path(&progress_base)?;
        let run_id = Uuid::new_v4().to_string();
        let progress_dir = progress_base.join(format!("run-{run_id}"));
        let mut targets = vec![(progress_dir.clone(), true)];
        if let Some(p) = &outdir {
            targets.push((p.clone(), true));
        }
        if let Some(p) = &output {
            targets.push((p.clone(), true));
            targets.push((PathBuf::from(format!("{}.cache.json", p.display())), true));
            targets.push((PathBuf::from(format!("{}.partial", p.display())), true));
        }
        if let Some(p) = &status_path {
            targets.push((p.clone(), true));
        }
        let lock = Arc::new(OutputLock::acquire(&targets)?);
        let dir = state_dir().join("runs").join(&run_id);
        private_dir(&dir)?;
        private_dir(&progress_dir)?;
        let mut excludes = vec![dir.clone(), state_dir(), progress_base.clone()];
        if let Some(p) = &outdir {
            excludes.push(p.clone());
        }
        if let Some(p) = &output {
            excludes.push(p.clone());
            excludes.push(PathBuf::from(format!("{}.cache.json", p.display())));
            excludes.push(PathBuf::from(format!("{}.partial", p.display())));
        }
        if let Some(p) = &status_path {
            excludes.push(p.clone());
        }
        let manifest = json!({"schemaVersion":1,"engine":"multi-ai-rust","engineVersion":env!("CARGO_PKG_VERSION"),
            "run_id":run_id,"command":command,"task_type":args.get("task-type"),"status":"running","started_at":Utc::now().to_rfc3339(),
            "reports":[],"outputs":{"out":output,"outdir":outdir,"progress":progress_dir},"excludes":excludes});
        let ctx = Self {
            task_type: args.get("task-type").map(str::to_owned),
            canaries,
            run_id,
            dir,
            output,
            outdir,
            progress_dir,
            excludes,
            status_path,
            _lock: lock,
            manifest: Arc::new(Mutex::new(manifest)),
            published: Arc::new(Mutex::new(Vec::new())),
            attempts: Arc::new(Mutex::new(Vec::new())),
        };
        let mut old = Vec::new();
        if let Some(p) = &ctx.output {
            old.push(p.clone());
            old.push(PathBuf::from(format!("{}.cache.json", p.display())));
            old.push(PathBuf::from(format!("{}.partial", p.display())));
        }
        if let Some(p) = &ctx.status_path {
            old.push(p.clone());
        }
        if let Some(p) = &ctx.outdir {
            let mut names = vec![
                "grok",
                "codex",
                "agy",
                "kimi",
                "kimi-k3",
                "deepseek",
                "glm",
                "kimi-k3-api",
                "claude-ds",
            ]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
            if let Ok(text) = fs::read_to_string(p.join("dispatch.json"))
                && let Ok(value) = serde_json::from_str::<Value>(&text)
                && let Some(rows) = value["results"].as_array()
            {
                for row in rows {
                    if let Some(name) = row["provider"].as_str()
                        && safe_id(name)
                    {
                        names.push(name.into());
                    }
                }
            }
            for name in names {
                old.push(p.join(format!("{name}.md")));
                old.push(p.join(".cache").join(format!("{name}.json")));
            }
        }
        ctx.archive(&old)?;
        ctx.save_manifest()?;
        ctx.save_status(json!({"status":"running","command":command}))?;
        Ok(ctx)
    }
    /// A batch owns the whole directory lock before constructing any job outputs.
    pub fn job_output(&self, id: &str) -> Result<Self> {
        if !safe_id(id) {
            bail!("invalid job id: {id}");
        }
        let root = self.outdir.as_ref().context("batch requires --outdir")?;
        let output = root.join(format!("{id}.md"));
        let status = root.join(format!("{id}.md.status.json"));
        let paths = vec![
            output.clone(),
            status.clone(),
            root.join(format!("{id}.md.cache.json")),
            root.join(format!("{id}.md.partial")),
        ];
        for path in &paths {
            if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink() || !m.is_file())
            {
                bail!(
                    "managed job output must be a regular file: {}",
                    path.display()
                );
            }
        }
        self.archive(&paths)?;
        let mut child = self.clone();
        child.output = Some(output);
        child.outdir = None;
        child.status_path = Some(status);
        Ok(child)
    }
    fn save_manifest(&self) -> Result<()> {
        atomic_json(
            &self.dir.join("manifest.json"),
            &self.manifest.lock().unwrap(),
        )
    }
    fn archive(&self, paths: &[PathBuf]) -> Result<()> {
        let history = state_dir().join("history").join(&self.run_id);
        let mut rows = Vec::new();
        let mut seen = self.published.lock().unwrap();
        for path in paths {
            if !path.is_file() || seen.contains(path) {
                continue;
            }
            let bytes = fs::read(path)?;
            private_dir(&history)?;
            let archived = history.join(format!(
                "{}-{}",
                sha256(path.as_os_str().as_encoded_bytes()),
                path.file_name().unwrap().to_string_lossy()
            ));
            atomic_bytes(&archived, &bytes)?;
            rows.push(json!({"original":path,"archive":archived,"sha256":sha256(&bytes)}));
            seen.push(path.clone());
        }
        if !rows.is_empty() {
            let file = history.join("manifest.json");
            let mut value = fs::read_to_string(&file)
                .ok()
                .and_then(|v| serde_json::from_str::<Value>(&v).ok())
                .unwrap_or_else(|| json!({"run_id":self.run_id,"files":[]}));
            value["files"].as_array_mut().unwrap().extend(rows);
            atomic_json(&file, &value)?;
        }
        Ok(())
    }
    pub fn attempt(&self, label: &str) -> Result<Attempt> {
        let label: String = label
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .take(80)
            .collect();
        let id = format!("{label}-{}", Uuid::new_v4());
        let dir = self.progress_dir.join(&id);
        private_dir(&dir)?;
        let attempt = Attempt {
            dir,
            id,
            run_id: self.run_id.clone(),
            started: Instant::now(),
            metadata: Arc::new(Mutex::new(
                json!({"status":"running","run_id":self.run_id,"label":label,"task_type":self.task_type,"started_at":Utc::now().to_rfc3339()}),
            )),
        };
        attempt.event("started", json!({}))?;
        self.attempts.lock().unwrap().push(attempt.clone());
        Ok(attempt)
    }
    pub fn report(&self, attempt: &Attempt, content: &str, result: &Value) -> Result<()> {
        if content.trim().is_empty() {
            bail!("empty: cannot publish empty report");
        }
        attempt.write("report.md", content.as_bytes())?;
        attempt.finish(
            "complete",
            json!({"reportSha256":sha256(content.as_bytes())}),
        )?;
        let cached = result["cached"].as_bool().unwrap_or(false);
        let mut result = result.clone();
        self.annotate(&mut result, content);
        attempt.update(json!({"task_type":self.task_type,"canary_hits":result["canary_hits"],"canary_total":result["canary_total"]}))?;
        let row = json!({"attempt_id":attempt.id,"run_id":self.run_id,"path":attempt.dir.join("report.md"),
            "report_sha256":sha256(content.as_bytes()),"result":result,"cached":cached,
            "model_calls":if cached {0}else{1}});
        {
            let mut manifest = self.manifest.lock().unwrap();
            let reports = manifest["reports"].as_array_mut().unwrap();
            if let Some(old) = reports.iter_mut().find(|r| r["attempt_id"] == attempt.id) {
                *old = row;
            } else {
                reports.push(row);
            }
        }
        self.save_manifest()?;
        if !self.canaries.is_empty() {
            append_feedback(
                &self.dir,
                &json!({
                    "event_id":Uuid::new_v4().to_string(),"run_id":self.run_id,"attempt_id":attempt.id,
                    "report_sha256":sha256(content.as_bytes()),"source":"canary","recorded_by":"multi-ai",
                    "counts":{"accepted":null,"false-positive":null,"duplicate":null,"uncertain":null},
                    "timing":{"screen-min":null,"rework-min":null},
                    "canary_hits":result["canary_hits"],"canary_total":result["canary_total"],
                    "evidence":result["canary_results"],"created_at":Utc::now().to_rfc3339()
                }),
            )?;
        }
        Ok(())
    }
    pub fn annotate(&self, result: &mut Value, content: &str) {
        result["task_type"] = json!(self.task_type);
        if let Some(fields) = result.as_object_mut() {
            for key in ["canary_hits", "canary_total", "canary_results"] {
                fields.remove(key);
            }
        }
        if !self.canaries.is_empty() {
            let fields = crate::canary::evaluate(&self.canaries, content);
            for (key, value) in fields.as_object().unwrap() {
                result[key] = value.clone();
            }
        }
    }
    pub fn publish(&self, content: &str) -> Result<()> {
        if let Some(path) = &self.output {
            self.publish_to(path, content)?;
        }
        Ok(())
    }
    pub fn publish_to(&self, path: &Path, content: &str) -> Result<()> {
        if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            bail!("output report must not be a symlink: {}", path.display());
        }
        let path = normalize_path(path)?;
        let covered = self.output.as_ref().is_some_and(|p| {
            p == &path || PathBuf::from(format!("{}.partial", p.display())) == path
        }) || self.outdir.as_ref().is_some_and(|p| path.starts_with(p));
        if !covered {
            bail!("unowned_output: {}", path.display());
        }
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            bail!("output report must not be a symlink");
        }
        self.archive(std::slice::from_ref(&path))?;
        atomic_text(&path, content)
    }
    pub fn save_status(&self, mut value: Value) -> Result<()> {
        if value["task_type"].is_null() {
            value["task_type"] = json!(self.task_type);
        }
        value["run_id"] = json!(self.run_id);
        value["manifest"] = json!(self.dir.join("manifest.json"));
        if let Some(path) = &self.status_path {
            atomic_json(path, &value)?;
        }
        let mut manifest = self.manifest.lock().unwrap();
        manifest["current_status"] = value;
        drop(manifest);
        self.save_manifest()
    }
    pub fn finish(&self, rc: i32) -> Result<()> {
        let status = if rc == 0 { "complete" } else { "error" };
        for attempt in self.attempts.lock().unwrap().iter() {
            let current = attempt.metadata.lock().unwrap()["status"]
                .as_str()
                .unwrap_or("running")
                .to_string();
            if current == "running" {
                attempt.finish(
                    if rc == 0 { "complete" } else { "error" },
                    json!({"exitCode":rc}),
                )?;
            }
        }
        {
            let mut manifest = self.manifest.lock().unwrap();
            manifest["status"] = json!(status);
            manifest["exitCode"] = json!(rc);
            manifest["finished_at"] = json!(Utc::now().to_rfc3339());
        }
        self.save_manifest()?;
        if let Some(path) = &self.status_path {
            let mut value = fs::read_to_string(path)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .unwrap_or_else(|| json!({}));
            if value["status"] == "running" || value["status"].is_null() {
                value["status"] = json!(status);
            }
            value["exitCode"] = json!(rc);
            self.save_status(value)?;
        }
        Ok(())
    }
}
impl Drop for OutputContext {
    fn drop(&mut self) {
        if Arc::strong_count(&self._lock) == 1
            && self.manifest.lock().unwrap()["status"] == "running"
        {
            let _ = self.save_status(
                json!({"status":"error","error":"invocation ended before final result"}),
            );
            let _ = self.finish(1);
        }
    }
}
pub fn offline_publish(args: &Args, content: &str) -> Result<()> {
    if args.get("out").is_some() {
        let ctx = OutputContext::new(args, "host-adapter", false)?;
        ctx.publish(content)?;
        ctx.save_status(json!({"status":"complete","model_calls":0,"adapter":"host"}))?;
        ctx.finish(0)?;
    }
    Ok(())
}
#[derive(Clone)]
pub struct Attempt {
    pub dir: PathBuf,
    pub id: String,
    pub run_id: String,
    pub started: Instant,
    metadata: Arc<Mutex<Value>>,
}
impl Attempt {
    pub fn append(&self, name: &str, bytes: &[u8]) -> Result<()> {
        if !safe_id(name) {
            bail!("invalid artifact name");
        }
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(self.dir.join(name))?;
        file.write_all(bytes)?;
        Ok(())
    }
    pub fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        if !safe_id(name) {
            bail!("invalid artifact name");
        }
        atomic_bytes(&self.dir.join(name), bytes)
    }
    pub fn update(&self, extra: Value) -> Result<()> {
        let mut value = self.metadata.lock().unwrap();
        if let Some(extra) = extra.as_object() {
            for (key, item) in extra {
                value[key] = item.clone();
            }
        }
        value["attempt_id"] = json!(self.id);
        value["totalMs"] = json!(self.started.elapsed().as_millis());
        atomic_json(&self.dir.join("metadata.json"), &value)
    }
    pub fn event(&self, kind: &str, mut extra: Value) -> Result<()> {
        extra["type"] = json!(kind);
        extra["elapsedMs"] = json!(self.started.elapsed().as_millis());
        self.append(
            "events.jsonl",
            format!("{}\n", serde_json::to_string(&extra)?).as_bytes(),
        )?;
        self.update(json!({}))
    }
    pub fn finish(&self, status: &str, mut extra: Value) -> Result<()> {
        extra["status"] = json!(status);
        self.update(extra)?;
        self.event(status, json!({}))
    }
}
fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

pub fn workspace_identity(
    cwd: &Path,
    source_id: Option<&str>,
    excludes: &[PathBuf],
) -> Result<Option<Value>> {
    let cwd = normalize_path(cwd)?;
    let list = Command::new("git")
        .args(["ls-files", "-co", "--exclude-standard", "-z"])
        .current_dir(&cwd)
        .output();
    let Ok(list) = list else {
        return Ok(source_id.map(|id| json!({"source_id":id,"cwd":cwd})));
    };
    if !list.status.success() {
        return Ok(source_id.map(|id| json!({"source_id":id,"cwd":cwd})));
    }
    let mut entries = Vec::new();
    for bytes in list.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()) {
        let name = String::from_utf8(bytes.to_vec()).context("non-UTF8 source path")?;
        let path = cwd.join(&name);
        let normalized = normalize_path(&path)?;
        if excludes
            .iter()
            .any(|p| normalized == *p || normalized.starts_with(p))
        {
            continue;
        }
        if !normalized.starts_with(&cwd) && source_id.is_none() {
            bail!("外部符号链接上下文恢复需要 --source-id <包含链接目标的不可变快照身份>");
        }
        let data = if path.is_file() {
            fs::read(&path)?
        } else {
            b"<missing>".to_vec()
        };
        entries.push((name, sha256(&data)));
    }
    entries.sort();
    entries.dedup();
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&cwd)
        .output()
        .ok()
        .filter(|v| v.status.success())
        .map(|v| String::from_utf8_lossy(&v.stdout).trim().to_string());
    Ok(Some(
        json!({"cwd":cwd,"head":head,"source_id":source_id,"files_sha256":sha256(&serde_json::to_vec(&entries)?)}),
    ))
}
pub fn cache_get(path: &Path, identity: &Value) -> Result<Option<Value>> {
    if !path.exists() {
        return Ok(None);
    }
    let value = fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let Some(value) = value else {
        return Ok(None);
    };
    if value["engine"] != "multi-ai-rust" {
        eprintln!(
            "[multi-ai] legacy cache retained; Rust identity differs, a new request may be made."
        );
        return Ok(None);
    }
    if value["identity"] != *identity || value["engineVersion"] != env!("CARGO_PKG_VERSION") {
        return Ok(None);
    }
    let Some(content) = value["content"].as_str() else {
        return Ok(None);
    };
    if content.trim().is_empty()
        || value["contentSha256"] != sha256(content.as_bytes())
        || value["result"]["complete"] != true
    {
        return Ok(None);
    }
    let mut result = value["result"].clone();
    result["cached"] = json!(true);
    result["output"] = json!(content);
    result["content"] = json!(content);
    result["reused_from"] = json!({"run_id":value["run_id"],"attempt_id":value["attempt_id"],"report_sha256":value["contentSha256"]});
    Ok(Some(result))
}
pub fn cache_put(path: &Path, identity: &Value, result: &Value, content: &str) -> Result<()> {
    if result["complete"] != true || content.trim().is_empty() {
        return Ok(());
    }
    atomic_json(
        path,
        &json!({"engine":"multi-ai-rust","engineVersion":env!("CARGO_PKG_VERSION"),
        "identity":identity,"result":result,"content":content,"contentSha256":sha256(content.as_bytes()),
        "run_id":result["run_id"],"attempt_id":result["attempt_id"]}),
    )
}

pub async fn passthrough(command: &str, tokens: &[String]) -> Result<i32> {
    use std::os::unix::process::ExitStatusExt;
    use std::time::Duration;
    use tokio::signal::unix::{SignalKind, signal};
    let config = crate::config::Config::load()?;
    let mut cmd = tokio::process::Command::new(command);
    cmd.args(tokens)
        .envs(config.environment())
        .process_group(0)
        .kill_on_drop(true);
    let mut child = cmd.spawn().with_context(|| {
        format!("Cannot launch {command}; install and login to this optional CLI")
    })?;
    let pid = child.id().context("child has no PID")? as i32;
    let mut term = signal(SignalKind::terminate())?;
    let exit = tokio::select! {
        result=child.wait()=>Some(result?),
        _=tokio::signal::ctrl_c()=>None,
        _=term.recv()=>None,
    };
    if let Some(exit) = exit {
        return Ok(exit
            .code()
            .unwrap_or_else(|| 128 + exit.signal().unwrap_or(1)));
    }
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    if tokio::time::timeout(Duration::from_millis(250), child.wait())
        .await
        .is_err()
    {
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
        let _ = child.wait().await;
    }
    Ok(130)
}

fn prepare_feedback(args: &Args) -> Result<(PathBuf, Value)> {
    let run = args.get("run").context("feedback requires --run UUID")?;
    if Uuid::parse_str(run).is_err() {
        bail!("invalid run ID");
    }
    let report = args
        .get("report")
        .context("feedback requires --report ATTEMPT-ID")?;
    if !safe_id(report) {
        bail!("invalid report ID");
    }
    let manifest_path = state_dir().join("runs").join(run).join("manifest.json");
    let manifest: Value = serde_json::from_str(&fs::read_to_string(&manifest_path)?)?;
    let row = manifest["reports"]
        .as_array()
        .context("no reports")?
        .iter()
        .find(|r| r["attempt_id"] == report)
        .context("report not found in run")?;
    let path = PathBuf::from(row["path"].as_str().context("report path missing")?);
    let content = fs::read(&path)?;
    if row["report_sha256"] != sha256(&content) {
        bail!("report hash mismatch: feedback refused");
    }
    let source = args.get("source").unwrap_or("agent");
    if !["agent", "human"].contains(&source) {
        bail!("--source must be agent or human");
    }
    if source == "human" && !args.has("human-confirmed") {
        bail!("human source requires --human-confirmed and explicit user evidence");
    }
    let evidence = args
        .get("evidence")
        .filter(|s| !s.trim().is_empty())
        .context("feedback requires --evidence")?;
    let mut counts = json!({});
    let mut has_count = false;
    for key in ["accepted", "false-positive", "duplicate", "uncertain"] {
        counts[key] = match args.get(key) {
            Some(value) => {
                has_count = true;
                json!(value.parse::<u64>()?)
            }
            None => Value::Null,
        };
    }
    let mut implementation = json!({});
    if let Some(outcome) = args.get("outcome") {
        if !["merged", "merged-after-rework", "discarded"].contains(&outcome) {
            bail!("invalid --outcome");
        }
        implementation["outcome"] = json!(outcome);
        has_count = true;
    }
    for field in ["rework-items", "rework-rounds"] {
        if let Some(value) = args.get(field) {
            implementation[field] = json!(value.parse::<u64>()?);
            has_count = true;
        }
    }
    if let Some(tags) = args.get("defect-tags") {
        implementation["defect-tags"] = json!(
            tags.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect::<std::collections::BTreeSet<_>>()
        );
        has_count = true;
    }
    if !has_count {
        bail!("feedback requires at least one report-level count");
    }
    let mut timing = json!({});
    for key in ["screen-min", "rework-min"] {
        timing[key] = match args.get(key) {
            Some(value) => {
                let n = value.parse::<f64>()?;
                if !n.is_finite() || n < 0.0 {
                    bail!("invalid --{key}");
                }
                json!(n)
            }
            None => Value::Null,
        };
    }
    let event = json!({"event_id":Uuid::new_v4().to_string(),"run_id":run,"attempt_id":report,
        "report_sha256":row["report_sha256"],"source":source,"recorded_by":"host",
        "counts":counts,"implementation":implementation,"timing":timing,"evidence":evidence,"created_at":Utc::now().to_rfc3339()});
    Ok((manifest_path.parent().unwrap().to_path_buf(), event))
}
fn append_feedback(directory: &Path, event: &Value) -> Result<()> {
    let file = directory.join("feedback.jsonl");
    let _lock = OutputLock::acquire(&[(file.clone(), true)])?;
    let mut out = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(&file)?;
    out.write_all(format!("{}\n", serde_json::to_string(event)?).as_bytes())?;
    out.sync_all()?;
    Ok(())
}
pub fn feedback_command(args: &Args) -> Result<i32> {
    if args.has("batch") || args.has("from") {
        return batch_feedback(args);
    }
    let (directory, event) = prepare_feedback(args)?;
    append_feedback(&directory, &event)?;
    stdout_line(format_args!("{}", serde_json::to_string_pretty(&event)?))?;
    Ok(0)
}
fn batch_feedback(args: &Args) -> Result<i32> {
    for key in [
        "run",
        "report",
        "accepted",
        "false-positive",
        "duplicate",
        "uncertain",
        "evidence",
        "outcome",
        "rework-items",
        "rework-rounds",
        "defect-tags",
        "screen-min",
        "rework-min",
    ] {
        if args.has(key) {
            bail!("--batch/--from cannot be combined with --{key}");
        }
    }
    let directory = Path::new(
        args.get("batch")
            .context("feedback requires --batch OUTDIR")?,
    );
    let text = fs::read_to_string(args.get("from").context("feedback requires --from TSV")?)?;
    let mut lines = text.lines();
    let header: Vec<_> = lines
        .next()
        .context("empty verdict TSV")?
        .split('\t')
        .collect();
    if header.len() < 5
        || header[..5] != ["id", "accepted", "false_positive", "uncertain", "evidence"]
        || header[5..].iter().any(|v| {
            ![
                "screen_min",
                "rework_min",
                "outcome",
                "rework_items",
                "rework_rounds",
                "defect_tags",
            ]
            .contains(v)
        })
        || header
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != header.len()
    {
        bail!(
            "TSV header must be id accepted false_positive uncertain evidence (tab-separated); optional screen_min/rework_min/outcome/rework_items/rework_rounds/defect_tags"
        );
    }
    let mut prepared = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (index, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != header.len() || !safe_id(fields[0]) || !seen.insert(fields[0].to_owned())
        {
            bail!("invalid or duplicate TSV row {}", index + 2);
        }
        let id = fields[0];
        let status: Value =
            serde_json::from_slice(&fs::read(directory.join(format!("{id}.md.status.json")))?)?;
        if (status["status"] != "ok" || status["complete"] != true)
            && !(status["status"] == "boundary_violation"
                && status["summary_published"] == true
                && status["execution_status"] == "ok")
        {
            bail!("batch job {id} has no current complete report");
        }
        let mut row_args = args.clone();
        row_args.set("run", status["run_id"].as_str().context("missing run_id")?);
        row_args.set(
            "report",
            status["attempt_id"]
                .as_str()
                .context("missing attempt_id")?,
        );
        for (key, value) in header.iter().zip(&fields).skip(1) {
            if !value.is_empty() {
                row_args.set(&key.replace('_', "-"), value);
            }
        }
        let (run_dir, mut event) =
            prepare_feedback(&row_args).with_context(|| format!("TSV row {} ({id})", index + 2))?;
        if sha256(&fs::read(directory.join(format!("{id}.md")))?) != event["report_sha256"] {
            bail!("batch job {id} published report hash mismatch");
        }
        event["batch_id"] = json!(id);
        prepared.push((run_dir, event));
    }
    if prepared.is_empty() {
        bail!("TSV has no verdict rows");
    }
    // No append until every row, count and bound report has passed validation.
    for (run_dir, event) in &prepared {
        append_feedback(run_dir, event)?;
    }
    stdout_line(format_args!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"written":prepared.len(),"events":prepared.iter().map(|(_,v)|v).collect::<Vec<_>>()})
        )?
    ))?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn path_aliases_share_lock_and_different_outputs_can_parallel() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a.md");
        let first = OutputLock::acquire(&[(a.clone(), true)]).unwrap();
        assert!(OutputLock::acquire(&[(a, true)]).is_err());
        assert!(OutputLock::acquire(&[(root.path().join("b.md"), true)]).is_ok());
        assert!(OutputLock::acquire(&[(root.path().to_path_buf(), true)]).is_err());
        drop(first);
    }
    #[test]
    fn external_source_symlink_requires_explicit_snapshot_for_resume_identity() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        assert!(
            Command::new("/usr/bin/git")
                .args(["init", "--quiet"])
                .current_dir(root.path())
                .status()
                .unwrap()
                .success()
        );
        fs::write(external.path().join("source.txt"), "external source").unwrap();
        symlink(
            external.path().join("source.txt"),
            root.path().join("source.txt"),
        )
        .unwrap();
        assert!(workspace_identity(root.path(), None, &[]).is_err());
        assert!(
            workspace_identity(root.path(), Some("immutable-linked-snapshot"), &[])
                .unwrap()
                .is_some()
        );
    }
    #[test]
    fn cache_integrity_partial_and_version() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("cache.json");
        let identity = json!({"model":"fixed"});
        cache_put(&path, &identity, &json!({"complete":false}), "partial").unwrap();
        assert!(!path.exists());
        cache_put(
            &path,
            &identity,
            &json!({"complete":true,"run_id":"original","attempt_id":"first"}),
            "report",
        )
        .unwrap();
        let cached = cache_get(&path, &identity).unwrap().unwrap();
        assert_eq!(cached["cached"], true);
        assert_eq!(cached["reused_from"]["run_id"], "original");
        assert!(
            cache_get(&path, &json!({"model":"changed"}))
                .unwrap()
                .is_none()
        );
        let mut value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        value["content"] = json!("tampered");
        atomic_json(&path, &value).unwrap();
        assert!(cache_get(&path, &identity).unwrap().is_none());
    }
}

//! Native review scheduler. Billing pools, not route names, are the selection unit.
use crate::{
    api,
    args::Args,
    config::{Config, Route},
    runtime::{self, Attempt, OutputContext},
};
use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use futures_util::{StreamExt, stream::FuturesUnordered};
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    fs::{self, OpenOptions},
    io::Read,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{io::AsyncReadExt, process::Command, sync::mpsc};

const QUOTA_TIGHT: f64 = 0.40;
const QUOTA_LOOSE: f64 = 0.70;
const QUOTA_FRESH_SEC: f64 = 21600.0;
fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
fn iso(t: f64) -> String {
    DateTime::<Utc>::from_timestamp(t as i64, 0)
        .map(|v| v.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}
fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}
fn review_state() -> PathBuf {
    runtime::state_dir().join("review")
}
fn state_file() -> PathBuf {
    review_state().join("state.json")
}
fn quota_file() -> PathBuf {
    review_state().join("quota.json")
}
fn sha(v: &[u8]) -> String {
    hex::encode(Sha256::digest(v))
}
fn engine_sha() -> Result<String> {
    static ENGINE_SHA: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    if let Some(value) = ENGINE_SHA.get() {
        return Ok(value.clone());
    }
    let value = sha(&fs::read(std::env::current_exe()?)?);
    Ok(ENGINE_SHA.get_or_init(|| value).clone())
}
fn secret_values(config: Option<&Config>, r: Option<&Route>) -> Vec<String> {
    let env = config
        .map(|c| c.environment().clone())
        .unwrap_or_else(|| std::env::vars().collect());
    let mut names = HashSet::new();
    if let Some(config) = config {
        for provider in config.profiles["providers"]
            .as_object()
            .into_iter()
            .flat_map(|p| p.values())
        {
            for name in provider["keyEnv"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                names.insert(name.to_owned());
            }
        }
    }
    let mut values = env
        .iter()
        .filter(|(name, value)| {
            !value.is_empty()
                && (names.contains(*name)
                    || name.ends_with("_API_KEY")
                    || name.ends_with("_TOKEN")
                    || name.ends_with("_PASSWORD"))
        })
        .map(|(_, v)| v.clone())
        .collect::<Vec<_>>();
    // Legacy wrappers can set private credentials themselves. Read only to redact; never log.
    if let Some(r) = r
        && let Some(program) = r.argv.first()
        && let Some(path) = resolve_executable(program, config)
        && let Ok(bytes) = script_bytes(&path)
        && bytes.starts_with(b"#!")
    {
        let text = String::from_utf8_lossy(&bytes);
        let pattern = Regex::new(
            r#"(?m)^\s*(?:export\s+)?([A-Z_]*(?:API_KEY|TOKEN|PASSWORD))\s*=\s*["']([^"']+)["']"#,
        )
        .unwrap();
        for row in pattern.captures_iter(&text) {
            if !row[2].contains('$') {
                values.push(row[2].to_owned());
            }
        }
    }
    values.sort_by_key(|v| std::cmp::Reverse(v.len()));
    values.dedup();
    values
}
fn mask(text: &str, secrets: &[String]) -> String {
    let mut clean = text.to_owned();
    for value in secrets {
        if !value.is_empty() {
            clean = clean.replace(value, "[REDACTED]");
            let encoded = serde_json::to_string(value).unwrap_or_default();
            if encoded.len() > 2 {
                clean = clean.replace(&encoded[1..encoded.len() - 1], "[REDACTED]");
            }
        }
    }
    for pattern in [
        r"\bsk-[A-Za-z0-9_-]{16,}\b",
        r"\bAKIA[A-Z0-9]{16}\b",
        r"\bgh[pousr]_[A-Za-z0-9_]{20,}\b",
        r"\bgithub_pat_[A-Za-z0-9_]{40,}\b",
        r"\bxox[baprs]-[A-Za-z0-9-]{20,}\b",
    ] {
        clean = Regex::new(pattern)
            .unwrap()
            .replace_all(&clean, "[REDACTED_API_KEY]")
            .into_owned();
    }
    clean
}
fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}
fn short(v: &str, n: usize) -> String {
    v.chars().take(n).collect()
}
fn read_json(path: &Path) -> Option<Value> {
    fs::read(path)
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
}
fn load_state() -> Value {
    read_json(&state_file())
        .or_else(|| read_json(&home().join(".claude/state/ai-review/state.json")))
        .unwrap_or_else(|| json!({"pools":{}}))
}
fn state_update<F: FnOnce(&mut Value)>(update: F) -> Result<()> {
    fs::create_dir_all(review_state())?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(review_state().join("state.lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut state = load_state();
    if !state["pools"].is_object() {
        state["pools"] = json!({});
    }
    update(&mut state);
    runtime::atomic_json(&state_file(), &state)?;
    Ok(())
}
fn blocked(pool: &str) -> Option<Value> {
    let entry = load_state()["pools"][pool].clone();
    if entry["until"].as_f64().unwrap_or(0.0) > now() {
        Some(entry)
    } else {
        None
    }
}

/// Do not classify input-size problems as wallet exhaustion, or echoed prompts as errors.
pub fn classify(text: &str, rc: i32, timed_out: bool, prompt: &str) -> String {
    if timed_out {
        return "timeout".into();
    }
    let re=Regex::new(r"Error \[(truncated|incomplete|empty|headers_timeout|idle_timeout|total_timeout|auth|quota|transport)\]").unwrap();
    if let Some(c) = re.captures(text) {
        return if c[1].ends_with("timeout") {
            "timeout".into()
        } else {
            c[1].into()
        };
    }
    if rc == 0 {
        return "ok".into();
    }
    let clean = if prompt.is_empty() {
        text.to_owned()
    } else {
        text.replace(prompt, "\n")
    };
    for (kind, pattern) in [
        (
            "error",
            r"(?i)context (length|window|size)|maximum context|too many tokens|input .{0,20}too long|prompt is too long|上下文(长度|过长|超限)|输入过长",
        ),
        (
            "quota",
            r"(?i)429|402|rate.?limit|quota (exceeded|exhausted)|insufficient (balance|quota|credit)|usage limit|limit exceeded|too many requests|余额不足|额度(不足|用尽|超限)|限额",
        ),
        (
            "auth",
            r"(?i)401|403|unauthorized|forbidden|not logged ?in|please log ?in|invalid (bearer|api.?key|token)|token expired|authentication",
        ),
        (
            "transport",
            r"(?i)ECONNREFUSED|ENOTFOUND|ETIMEDOUT|ECONNRESET|socket hang up|connection closed|connection reset|network|proxy|tunnel|EOF|broken pipe|fetch failed|UND_ERR_",
        ),
    ] {
        if Regex::new(pattern).unwrap().is_match(&clean) {
            return kind.into();
        }
    }
    "error".into()
}
fn parse_time(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| {
        v.as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.timestamp() as f64)
    })
}
fn retry_clock(text: &str) -> Option<f64> {
    let c=Regex::new(r"(?i)try again at\s+([A-Za-z]{3})[a-z]*\s+(\d{1,2})(?:st|nd|rd|th)?,?\s+(\d{4})[,\s]+(\d{1,2}):(\d{2})\s*([AP])\.?M\.?").unwrap();
    let c = c.captures(text)?;
    let months = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let month = months.iter().position(|m| *m == c[1].to_lowercase())? + 1;
    let hour = c[4].parse::<u32>().ok()? % 12
        + if c[6].eq_ignore_ascii_case("P") {
            12
        } else {
            0
        };
    let date = NaiveDateTime::parse_from_str(
        &format!(
            "{}-{month:02}-{:02} {hour:02}:{}",
            &c[3],
            c[2].parse::<u32>().ok()?,
            &c[5]
        ),
        "%Y-%m-%d %H:%M",
    )
    .ok()?;
    Some(
        ((Local.from_local_datetime(&date).earliest()?.timestamp() as f64 - now()) / 60.0).max(1.0),
    )
}
fn retry_after(text: &str) -> Option<f64> {
    let re = Regex::new(r"(?i)retry[- ]after[:\s]+(\d+)\s*(second|sec|s)?\b").unwrap();
    if let Some(c) = re.captures(text) {
        return Some((c[1].parse::<f64>().ok()? / 60.0).max(1.0));
    }
    Regex::new(r"(?i)try again in\s+(\d+)\s*(minute|min)")
        .unwrap()
        .captures(text)
        .and_then(|c| c[1].parse().ok())
}
pub fn quota_schedulable(info: &Value) -> bool {
    if info["stale"].as_bool().unwrap_or(false)
        || info
            .get("error")
            .and_then(Value::as_str)
            .is_some_and(|v| !v.is_empty())
        || !matches!(s(info, "source"), "live" | "record")
    {
        return false;
    }
    let names: HashSet<&str> = info["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| s(v, "name"))
        .collect();
    names.contains("5h") && names.contains("周")
}
fn tightest(info: &Value) -> Option<f64> {
    if !quota_schedulable(info) {
        return None;
    }
    info["windows"]
        .as_array()?
        .iter()
        .filter_map(|v| v["remaining"].as_f64())
        .reduce(f64::min)
}
pub fn quota_penalty(pool: &str, quotas: &Value) -> u8 {
    let Some(mine) = tightest(&quotas[pool]) else {
        return 0;
    };
    if mine >= QUOTA_TIGHT {
        return 0;
    }
    u8::from(quotas.as_object().is_some_and(|q| {
        q.iter()
            .any(|(p, v)| p != pool && tightest(v).is_some_and(|n| n > QUOTA_LOOSE))
    }))
}
fn snapshot_wait(pool: &str) -> Option<f64> {
    let quota = read_json(&quota_file())?;
    let info = &quota["pools"][pool];
    if !quota_schedulable(info) {
        return None;
    }
    info["windows"]
        .as_array()?
        .iter()
        .filter(|v| v["remaining"].as_f64().unwrap_or(1.0) < QUOTA_TIGHT)
        .filter_map(|v| parse_time(&v["reset"]))
        .map(|t| (t - now()) / 60.0)
        .filter(|t| *t >= 1.0)
        .reduce(f64::min)
}
fn cool_down(pool: &str, kind: &str, detail: &str) -> Result<()> {
    let default = match kind {
        "quota" => 60.0,
        "auth" => 240.0,
        "transport" => 5.0,
        _ => return Ok(()),
    };
    let minutes = if kind == "quota" {
        retry_clock(detail)
            .or_else(|| snapshot_wait(pool))
            .unwrap_or_else(|| {
                retry_after(detail)
                    .filter(|v| *v >= default)
                    .unwrap_or(default)
            })
    } else {
        retry_after(detail).unwrap_or(default).max(default)
    };
    state_update(
        |state| state["pools"][pool] = json!({"until":now()+minutes*60.0,"kind":kind,"detail":short(detail,400),"since":Local::now().format("%Y-%m-%d %H:%M:%S").to_string()}),
    )
}
fn name(route: &Route, legacy: bool) -> &str {
    if legacy { &route.legacy_id } else { &route.id }
}
fn route<'a>(config: &'a Config, id: &str, legacy: bool) -> Result<&'a Route> {
    config
        .routes
        .iter()
        .find(|r| r.id == id || (legacy && r.legacy_id == id))
        .ok_or_else(|| {
            anyhow!(
                "未知通路 {id}；可选：{}",
                config
                    .routes
                    .iter()
                    .map(|r| name(r, legacy))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}
fn eligible(
    r: &Route,
    config: &Config,
    agentic: bool,
    no_metered: bool,
    exclude: &[String],
    legacy: bool,
) -> bool {
    let pools: HashSet<&str> = exclude
        .iter()
        .filter_map(|id| route(config, id, legacy).ok())
        .map(|p| p.pool.as_str())
        .collect();
    !exclude.iter().any(|id| id == &r.id || id == &r.legacy_id)
        && !pools.contains(r.pool.as_str())
        && (!agentic || r.agentic)
        && (!no_metered || r.cost != "metered")
        && blocked(&r.pool).is_none()
}
#[allow(clippy::too_many_arguments)] // Separate scheduling filters preserve the legacy interface.
pub fn pick_seats(
    config: &Config,
    want: usize,
    only: &[String],
    exclude: &[String],
    agentic: bool,
    no_metered: bool,
    quotas: &Value,
    legacy: bool,
) -> Vec<Route> {
    let mut candidates = if !only.is_empty() {
        only.iter()
            .filter_map(|id| route(config, id, legacy).ok())
            .cloned()
            .collect::<Vec<_>>()
    } else {
        let mut r = config.routes.clone();
        r.sort_by_key(|p| (quota_penalty(&p.pool, quotas), p.rank, p.id.clone()));
        r
    };
    let mut seen = HashSet::new();
    candidates.retain(|r| {
        eligible(r, config, agentic, no_metered, exclude, legacy) && seen.insert(r.pool.clone())
    });
    candidates.truncate(want);
    candidates
}
fn backups(
    config: &Config,
    used: &[Route],
    exclude: &[String],
    agentic: bool,
    no_metered: bool,
    quotas: &Value,
    legacy: bool,
) -> Vec<Route> {
    let mut all = pick_seats(
        config,
        config.routes.len(),
        &[],
        exclude,
        agentic,
        no_metered,
        quotas,
        legacy,
    );
    let pools: HashSet<&str> = used.iter().map(|r| r.pool.as_str()).collect();
    all.retain(|r| !pools.contains(r.pool.as_str()));
    all
}
fn executable_exists(name: &str) -> bool {
    if name.contains('/') {
        return Path::new(name).is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}
fn resolve_executable(name: &str, config: Option<&Config>) -> Option<PathBuf> {
    if name.contains('/') {
        return Path::new(name)
            .is_file()
            .then(|| fs::canonicalize(name).ok())
            .flatten();
    }
    let path = config
        .and_then(|c| c.environment().get("PATH").map(String::as_str))
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"))?;
    std::env::split_paths(&path)
        .map(|p| p.join(name))
        .find(|p| p.is_file())
        .and_then(|p| fs::canonicalize(p).ok())
}
fn script_bytes(path: &Path) -> Result<Vec<u8>> {
    let mut file = fs::File::open(path)?;
    let mut marker = [0; 2];
    if file.read(&mut marker)? == 2 && marker == *b"#!" {
        Ok(fs::read(path)?)
    } else {
        Ok(Vec::new())
    }
}
fn add_fingerprint(path: &Path, files: &mut BTreeMap<String, Value>) -> Result<()> {
    let key = path.to_string_lossy().to_string();
    if files.contains_key(&key) {
        return Ok(());
    }
    if !path.exists() {
        files.insert(key, json!({"exists":false}));
        return Ok(());
    }
    let real = fs::canonicalize(path)?;
    if real.is_file() {
        files.insert(
            key,
            json!({"exists":true,"target":real,"sha256":sha(&fs::read(path)?)}),
        );
    }
    Ok(())
}
fn tree_fingerprints(path: &Path, files: &mut BTreeMap<String, Value>) -> Result<()> {
    if !path.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        let path = entry?.path();
        if path.is_dir() {
            tree_fingerprints(&path, files)?;
        } else if path.is_file() {
            add_fingerprint(&path, files)?;
        }
    }
    Ok(())
}
fn program_fingerprints(
    path: &Path,
    config: &Config,
    files: &mut BTreeMap<String, Value>,
) -> Result<()> {
    add_fingerprint(path, files)?;
    let mut header = [0; 4096];
    let count = fs::File::open(path)?.read(&mut header)?;
    let bytes = &header[..count];
    if bytes.starts_with(b"#!") {
        let head = String::from_utf8_lossy(bytes);
        let line = head
            .lines()
            .next()
            .unwrap_or_default()
            .trim_start_matches("#!")
            .trim();
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        let program = if tokens.first().is_some_and(|v| v.ends_with("/env")) {
            tokens.get(1)
        } else {
            tokens.first()
        };
        if let Some(program) = program
            && let Some(interpreter) = resolve_executable(program, Some(config))
        {
            add_fingerprint(&interpreter, files)?;
        }
        // Codex's npm entry point delegates to a platform binary. Bind both package metadata
        // and bundled/vendor executables, rather than only its small JavaScript wrapper.
        for ancestor in path.ancestors().skip(1).take(3) {
            if ancestor.join("package.json").is_file() {
                add_fingerprint(&ancestor.join("package.json"), files)?;
                tree_fingerprints(&ancestor.join("vendor"), files)?;
                if ancestor.file_name().is_some_and(|n| n == "codex")
                    && let Some(scope) = ancestor.parent()
                {
                    for entry in fs::read_dir(scope)? {
                        let package = entry?.path();
                        if package
                            .file_name()
                            .is_some_and(|n| n.to_string_lossy().starts_with("codex-"))
                        {
                            add_fingerprint(&package.join("package.json"), files)?;
                            tree_fingerprints(&package.join("vendor"), files)?;
                        }
                    }
                }
                break;
            }
        }
    }
    Ok(())
}
fn cli_identity(r: &Route, config: &Config, args: &Args) -> Result<(Value, bool)> {
    let Some(first) = r.argv.first() else {
        return Ok((json!({"missingExecutable":true}), false));
    };
    let Some(executable) = resolve_executable(first, Some(config)) else {
        return Ok((json!({"missingExecutable":true}), false));
    };
    let mut files = BTreeMap::new();
    program_fingerprints(&executable, config, &mut files)?;
    let cwd = fs::canonicalize(args.get("cwd").unwrap_or("."))
        .unwrap_or_else(|_| PathBuf::from(args.get("cwd").unwrap_or(".")));
    // A shell fixture/wrapper is implementation too. Hash existing literal file arguments;
    // generated prompt/output placeholders are deliberately excluded.
    for arg in r
        .argv
        .iter()
        .skip(1)
        .filter(|v| !v.starts_with('-') && !v.contains('{'))
    {
        let path = PathBuf::from(arg);
        let path = if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        };
        if path.is_file() {
            program_fingerprints(&path, config, &mut files)?;
        }
    }
    let env = config.environment();
    let home = env.get("HOME").map(PathBuf::from).unwrap_or_else(home);
    let mut candidates = Vec::new();
    let known_vendor = match r.legacy_id.as_str() {
        "codex" => {
            let root = env
                .get("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".codex"));
            candidates.push(root.join("config.toml"));
            Some(".codex")
        }
        "kimi" | "kimi-k3" => {
            for root in [home.join(".kimi-code"), home.join(".kimi")] {
                candidates.push(root.join("config.toml"));
            }
            for key in ["KIMI_HOME", "KIMI_CODE_HOME"] {
                if let Some(root) = env.get(key) {
                    candidates.push(PathBuf::from(root).join("config.toml"));
                }
            }
            Some(".kimi")
        }
        "grok" => {
            candidates.push(home.join(".grok/config.toml"));
            candidates.push(home.join(".config/grok/config.toml"));
            Some(".grok")
        }
        "agy" => {
            candidates.push(home.join(".config/agy/config.json"));
            Some(".agy")
        }
        "claude-ds" => {
            candidates.push(home.join(".claude/settings.json"));
            if let Some(claude) = resolve_executable("claude", Some(config)) {
                program_fingerprints(&claude, config, &mut files)?;
            }
            Some(".claude")
        }
        _ => None,
    };
    if let Some(folder) = known_vendor {
        for ancestor in cwd.ancestors() {
            for file in ["config.toml", "settings.json", "settings.local.json"] {
                candidates.push(ancestor.join(folder).join(file));
            }
        }
    }
    for pair in r.argv.windows(2) {
        if ["--config", "--config-file", "--settings"].contains(&pair[0].as_str())
            && !pair[1].contains('{')
        {
            let path = PathBuf::from(&pair[1]);
            candidates.push(if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            });
        }
    }
    for (key, value) in env {
        if (key.contains("CONFIG") || key.ends_with("_SETTINGS")) && Path::new(value).is_file() {
            candidates.push(PathBuf::from(value));
        }
    }
    let explicit_model = r
        .argv
        .windows(2)
        .any(|p| ["-m", "--model"].contains(&p[0].as_str()) && !p[1].contains('{'));
    let default_key = if ["kimi", "kimi-k3"].contains(&r.legacy_id.as_str()) {
        "default_model"
    } else {
        "model"
    };
    let model_pattern =
        Regex::new(&format!(r#"(?m)^\s*{default_key}\s*=\s*["'][^"']+["']"#)).unwrap();
    let mut known_model = explicit_model || known_vendor.is_none() || r.legacy_id == "claude-ds";
    for path in candidates {
        if let Ok(bytes) = fs::read(&path) {
            let text = String::from_utf8_lossy(&bytes);
            let top = text
                .lines()
                .take_while(|line| !line.trim_start().starts_with('['))
                .collect::<Vec<_>>()
                .join("\n");
            known_model |= model_pattern.is_match(&top)
                || serde_json::from_slice::<Value>(&bytes)
                    .ok()
                    .is_some_and(|v| v[default_key].as_str().is_some_and(|s| !s.is_empty()));
        }
        add_fingerprint(&path, &mut files)?;
    }
    let overrides: BTreeMap<&str, &str> = env
        .iter()
        .filter(|(key, _)| {
            key.contains("MODEL")
                || key.ends_with("_BASE_URL")
                || key.contains("CONFIG")
                || key.ends_with("_HOME")
        })
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    // Only hashes enter durable metadata. Config files and environment values may contain keys.
    Ok((
        json!({"files":files,"overridesSha256":sha(&serde_json::to_vec(&overrides)?),"modelIdentityKnown":known_model}),
        known_model,
    ))
}
fn numeric(args: &Args, key: &str, default: f64) -> Result<f64> {
    let v = args
        .get(key)
        .map(str::parse::<f64>)
        .transpose()?
        .unwrap_or(default);
    if !v.is_finite() {
        bail!("{key} 必须为有限数值");
    }
    Ok(v)
}
fn append_material(mut prompt: String, args: &Args) -> Result<String> {
    for file in args.all("input") {
        let path = fs::canonicalize(&file).with_context(|| format!("读取材料 {file}"))?;
        let text = fs::read_to_string(&path)?;
        if text.trim().is_empty() {
            bail!("材料为空：{}", path.display());
        }
        prompt.push_str(&format!(
            "\n\n--- 附带材料：{} ---\n{text}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    if prompt.trim().is_empty() {
        bail!("提示词／材料为空");
    }
    Ok(prompt)
}
fn read_prompt(args: &Args, skip: usize) -> Result<String> {
    let prompt = if let Some(p) = args.get("prompt-file") {
        fs::read_to_string(p)?
    } else if let Some(p) = args.get("prompt") {
        p.into()
    } else if args.positional.len() > skip {
        args.positional[skip..].join(" ")
    } else {
        let mut p = String::new();
        std::io::stdin().read_to_string(&mut p)?;
        p
    };
    append_material(prompt, args)
}
fn cache_path(ctx: &OutputContext, r: &Route) -> Option<PathBuf> {
    ctx.outdir
        .as_ref()
        .map(|p| p.join(".cache").join(format!("{}.json", r.legacy_id)))
        .or_else(|| {
            ctx.output
                .as_ref()
                .map(|p| PathBuf::from(format!("{}.cache.json", p.display())))
        })
}

pub async fn execute_cli(r: &Route, args: &Args, prompt: &str, attempt: &Attempt) -> Result<Value> {
    execute_cli_env(r, args, prompt, attempt, None).await
}
struct RequestCleanup(PathBuf);
impl Drop for RequestCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
struct ProcessGroup(i32);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if self.0 > 0 {
            unsafe {
                libc::kill(-self.0, libc::SIGKILL);
            }
        }
    }
}
fn cli_line_progress(
    r: &Route,
    attempt: &Attempt,
    bytes: &[u8],
    consumed: &mut usize,
    flush: bool,
    secrets: &[String],
) -> Result<()> {
    if r.extract != "jsonl-assistant" {
        return Ok(());
    }
    loop {
        let rest = &bytes[*consumed..];
        let length = if let Some(n) = rest.iter().position(|b| *b == b'\n') {
            n + 1
        } else if flush && !rest.is_empty() {
            rest.len()
        } else {
            break;
        };
        if let Ok(row) = serde_json::from_slice::<Value>(&rest[..length]) {
            if row["role"] == "assistant" && row["content"].is_string() && !has_tools(&row) {
                attempt.write(
                    "answer.partial.txt",
                    mask(row["content"].as_str().unwrap(), secrets).as_bytes(),
                )?;
            }
            if let Some(id) = row.get("session_id") {
                let id = mask(&id.to_string(), secrets);
                attempt.update(json!({"sessionId":serde_json::from_str::<Value>(&id).unwrap_or_else(|_|json!("[REDACTED]"))}))?;
            }
        }
        *consumed += length;
    }
    Ok(())
}
fn log_snapshot(
    attempt: &Attempt,
    file: &str,
    bytes: &[u8],
    secrets: &[String],
    flush: bool,
) -> Result<()> {
    let clean = mask(&String::from_utf8_lossy(bytes), secrets);
    // Rewrite atomically instead of appending: a credential split across pipe chunks never
    // becomes a full literal in the file. Carry a possible suffix until its identity is known.
    let mut hold = 0;
    if !flush {
        for secret in secrets {
            for (n, _) in secret.char_indices().rev().filter(|(n, _)| *n > 0) {
                if clean.ends_with(&secret[..n]) {
                    hold = hold.max(n);
                    break;
                }
            }
        }
    }
    attempt.write(file, &clean.as_bytes()[..clean.len() - hold])
}
async fn execute_cli_env(
    r: &Route,
    args: &Args,
    prompt: &str,
    attempt: &Attempt,
    config: Option<&Config>,
) -> Result<Value> {
    let total = numeric(args, "timeout", r.default_timeout as f64)?;
    let secrets = secret_values(config, Some(r));
    let idle = numeric(args, "idle-timeout", 0.0)?;
    if total <= 0.0 || idle < 0.0 {
        bail!("时间上限必须为正数；idle-timeout可为0");
    }
    let promptfile = attempt.dir.join("request.txt");
    let _request_cleanup = RequestCleanup(promptfile.clone());
    attempt.write("request.txt", prompt.as_bytes())?;
    let outfile = attempt.dir.join("last-message.txt");
    let cli_prompt = if r.agentic && prompt.len() > 32768 {
        format!(
            "请完整读取任务材料文件 {}，按文件内要求进行只读取证／复核。交回最终正文与覆盖限制。",
            promptfile.display()
        )
    } else {
        prompt.to_owned()
    };
    let argv: Vec<String> = r
        .argv
        .iter()
        .map(|s| {
            s.replace("{prompt}", &cli_prompt)
                .replace("{promptfile}", &promptfile.to_string_lossy())
                .replace("{outfile}", &outfile.to_string_lossy())
                .replace(
                    "{transportdir}",
                    &attempt.dir.join("transport").to_string_lossy(),
                )
        })
        .collect();
    let first = argv.first().context("通路缺少argv")?;
    if !executable_exists(first) {
        let _ = fs::remove_file(&promptfile);
        return Ok(
            json!({"status":"missing","error":format!("找不到可执行文件：{first}"),"content":"","complete":false,"seconds":0.0}),
        );
    }
    let mut c = Command::new(first);
    c.args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    if let Some(path) = args.get("cwd") {
        c.current_dir(path);
    }
    if let Some(config) = config {
        c.envs(config.environment());
    }
    let start = Instant::now();
    let mut child = c.spawn().with_context(|| format!("启动 {first}"))?;
    let pid = child.id().unwrap_or(0) as i32;
    let _process_group = ProcessGroup(pid);
    let stdout = child.stdout.take().context("stdout")?;
    let stderr = child.stderr.take().context("stderr")?;
    let (tx, mut rx) = mpsc::unbounded_channel::<(bool, Vec<u8>)>();
    async fn pipe<R: tokio::io::AsyncRead + Unpin>(
        mut reader: R,
        channel: bool,
        tx: mpsc::UnboundedSender<(bool, Vec<u8>)>,
    ) -> Result<()> {
        let mut b = vec![0; 65536];
        loop {
            let n = reader.read(&mut b).await?;
            if n == 0 {
                break;
            }
            if tx.send((channel, b[..n].to_vec())).is_err() {
                break;
            }
        }
        Ok(())
    }
    let t1 = tokio::spawn(pipe(stdout, true, tx.clone()));
    let t2 = tokio::spawn(pipe(stderr, false, tx.clone()));
    drop(tx);
    let (mut out, mut err) = (vec![], vec![]);
    let mut last = Instant::now();
    let mut notice = Instant::now();
    let mut cause = None;
    let mut consumed = 0;
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut pipes = true;
    let status;
    loop {
        tokio::select! {
            row=rx.recv(),if pipes=>match row{Some((channel,bytes))=>{
                last=Instant::now();
                if channel{out.extend_from_slice(&bytes);log_snapshot(attempt,"stdout.raw",&out,&secrets,false)?;cli_line_progress(r,attempt,&out,&mut consumed,false,&secrets)?;}
                else{err.extend_from_slice(&bytes);log_snapshot(attempt,"stderr.raw",&err,&secrets,false)?;}
                attempt.update(json!({"stdoutBytes":out.len(),"stderrBytes":err.len(),"lastDataSeconds":start.elapsed().as_secs_f64()}))?;
            },None=>pipes=false},
            _=ticker.tick()=>{
                if cause.is_none(){
                    if start.elapsed().as_secs_f64()>=total{cause=Some("total_timeout");}
                    else if idle>0.0&&last.elapsed().as_secs_f64()>=idle{cause=Some("idle_timeout");}
                }
                if let Some(reason)=cause{
                    attempt.event("terminated",json!({"reason":reason}))?;
                    if pid>0{unsafe{libc::kill(-pid,libc::SIGKILL);}}
                    let _=child.start_kill();status=child.wait().await?;break;
                }
                if notice.elapsed().as_secs()>=15{
                    notice=Instant::now();
                    eprintln!("[{}] {:.0}s，距最后输出 {:.0}s；进度 {}",r.id,start.elapsed().as_secs_f64(),last.elapsed().as_secs_f64(),attempt.dir.display());
                }
                if !pipes&&let Some(done)=child.try_wait()?{status=done;break;}
            },
            _=interrupt.recv()=>cause=Some("cancelled"),_ =terminate.recv()=>cause=Some("cancelled"),
        }
    }
    let drain = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some((channel, bytes)) = rx.recv().await {
            if channel {
                out.extend_from_slice(&bytes);
                log_snapshot(attempt, "stdout.raw", &out, &secrets, false)?;
            } else {
                err.extend_from_slice(&bytes);
                log_snapshot(attempt, "stderr.raw", &err, &secrets, false)?;
            }
        }
        Ok::<(), anyhow::Error>(())
    })
    .await;
    if drain.is_err() {
        t1.abort();
        t2.abort();
    }
    let _ = t1.await;
    let _ = t2.await;
    let _ = fs::remove_file(&promptfile);
    let stdout = String::from_utf8_lossy(&out).to_string();
    let stderr = String::from_utf8_lossy(&err).to_string();
    log_snapshot(attempt, "stdout.raw", &out, &secrets, true)?;
    log_snapshot(attempt, "stderr.raw", &err, &secrets, true)?;
    cli_line_progress(r, attempt, &out, &mut consumed, true, &secrets)?;
    let mut kind = if cause == Some("cancelled") {
        "cancelled".into()
    } else {
        classify(
            &mask(&format!("{stderr}\n{stdout}"), &secrets),
            status.code().unwrap_or(1),
            cause.is_some(),
            &mask(prompt, &secrets),
        )
    };
    let body = if kind == "ok" {
        let body = extract_output(r, &stdout, &outfile)?;
        mask(&body, &secrets)
    } else {
        String::new()
    };
    // Vendor CLI owns this staging file until it exits; sanitize it after extracting its body.
    if let Ok(bytes) = fs::read(&outfile) {
        attempt.write(
            "last-message.txt",
            mask(&String::from_utf8_lossy(&bytes), &secrets).as_bytes(),
        )?;
    }
    let error = if kind == "ok" && body.is_empty() {
        kind = "empty".into();
        "退出码0但没有完整的最终正文".into()
    } else if let Some(reason) = cause {
        format!(
            "{reason}：总耗时 {:.1}s，距最后输出 {:.1}s；上限 {total}s。现场已保留",
            start.elapsed().as_secs_f64(),
            last.elapsed().as_secs_f64()
        )
    } else if kind != "ok" {
        let raw = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        raw.chars()
            .rev()
            .take(1600)
            .collect::<String>()
            .chars()
            .rev()
            .collect()
    } else {
        String::new()
    };
    let error = mask(&error, &secrets);
    Ok(
        json!({"status":kind,"content":body,"complete":kind=="ok","error":error,"seconds":(start.elapsed().as_secs_f64()*10.0).round()/10.0,"exitCode":status.code()}),
    )
}
fn has_tools(v: &Value) -> bool {
    v.get("tool_calls")
        .is_some_and(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
}
fn extract_output(r: &Route, stdout: &str, outfile: &Path) -> Result<String> {
    match r.extract.as_str() {
        "last-message-file" => Ok(fs::read_to_string(outfile)
            .unwrap_or_default()
            .trim()
            .to_owned()),
        "jsonl-assistant" => {
            let row = stdout
                .lines()
                .filter_map(|s| serde_json::from_str::<Value>(s).ok())
                .filter(|v| v["role"] == "assistant")
                .next_back();
            Ok(row
                .filter(|v| !has_tools(v))
                .and_then(|v| v["content"].as_str().map(str::to_owned))
                .unwrap_or_default()
                .trim()
                .to_owned())
        }
        "stdout" => Ok(stdout.trim().to_owned()),
        "llm-json" => bail!("API通路必须使用原生adapter"),
        _ => bail!("未知输出提取方式 {}", r.extract),
    }
}

async fn run_one(
    config: &Config,
    r: &Route,
    args: &Args,
    prompt: &str,
    ctx: &OutputContext,
    legacy: bool,
) -> Result<Value> {
    let start = Instant::now();
    let attempt = ctx.attempt(&r.id)?;
    let mut result = json!({"provider":name(r,legacy),"route":r.id,"pool":r.pool,"family":r.family,"label":r.label,"status":"error","output":"","error":"","seconds":0.0,"artifacts":attempt.dir,"run_id":ctx.run_id,"attempt_id":attempt.id,"complete":false});
    let total = numeric(args, "timeout", r.default_timeout as f64)?;
    let idle = numeric(args, "idle-timeout", 0.0)?;
    if total <= 0.0
        || idle < 0.0
        || args.get("headers-timeout").is_some_and(|v| {
            v.parse::<f64>()
                .map_or(true, |v| !v.is_finite() || v <= 0.0)
        })
        || args
            .get("max-tokens")
            .is_some_and(|v| v.parse::<u64>().map_or(true, |v| v == 0))
    {
        bail!("超时和输出预算必须为正数；idle-timeout可为0");
    }
    let attached = !args.all("input").is_empty();
    if !r.agentic && args.has("cwd") && !(attached || args.has("text-ready")) {
        result["status"] = json!("missing_input");
        result["error"] =
            json!("文本通路不能读cwd；附 --input FILE，或用 --text-ready 声明prompt已含完整证据");
        attempt.finish("missing_input", result.clone())?;
        return Ok(result);
    }
    let mut api_args = args.clone();
    api_args.set("timeout-ms", (total * 1000.0).max(50.0) as u64);
    if idle > 0.0 {
        api_args.set("idle-timeout-ms", (idle * 1000.0) as u64);
    }
    if let Some(h) = args.get("headers-timeout") {
        api_args.set("headers-timeout-ms", (h.parse::<f64>()? * 1000.0) as u64);
    }
    let (resolved, model_identity_known) = if let Some(alias) = &r.api_alias {
        (config.alias_metadata(alias, &api_args)?, true)
    } else {
        cli_identity(r, config, args)?
    };
    let cwd = PathBuf::from(args.get("cwd").unwrap_or("."));
    let mut cache = cache_path(ctx, r);
    let cache_disabled_reason = if !model_identity_known && cache.is_some() {
        cache = None;
        let message = "CLI默认模型或执行身份不能可靠确定；本轮不缓存，也不复用旧报告";
        eprintln!("[{}] {message}", r.id);
        result["cache_disabled"] = json!(message);
        Some(message)
    } else {
        None
    };
    let workspace = if cache.is_some() {
        match runtime::workspace_identity(&cwd, args.get("source-id"), &ctx.excludes) {
            Ok(Some(v)) => Some(v),
            Ok(None) => {
                if args.has("resume") {
                    bail!("非Git工作区恢复需要 --source-id <不可变源码快照身份>");
                }
                cache = None;
                None
            }
            Err(e) => {
                if args.has("resume") {
                    return Err(e);
                }
                cache = None;
                None
            }
        }
    } else {
        None
    };
    let identity = json!({"version":env!("CARGO_PKG_VERSION"),"engineSha256":engine_sha()?,"implementation":"rust-native-v1","provider":r.id,"routeConfigSha256":sha(&serde_json::to_vec(r)?),"configSha256":sha(&serde_json::to_vec(&json!({"routes":config.routes,"profiles":config.profiles}))?),"promptSha256":sha(prompt.as_bytes()),"cwd":fs::canonicalize(&cwd).unwrap_or(cwd),"sourceId":args.get("source-id"),"workspace":workspace,"timeout":total,"idleTimeout":idle,"headersTimeout":args.get("headers-timeout"),"maxTokens":args.get("max-tokens"),"resolved":resolved});
    attempt.update(json!({"inputIdentity":identity,"promptSha256":sha(prompt.as_bytes()),"inputAttached":attached,"textReady":args.has("text-ready")}))?;
    if args.has("resume") && cache_disabled_reason.is_none() {
        let path = cache.as_ref().context("恢复需要 --out 或 --outdir")?;
        if let Some(mut cached) = runtime::cache_get(path, &identity)? {
            cached["cached"] = json!(true);
            cached["reused_from"] = json!({"run_id":cached["run_id"],"attempt_id":cached["attempt_id"],"artifacts":cached["artifacts"]});
            cached["run_id"] = json!(ctx.run_id);
            cached["attempt_id"] = json!(attempt.id);
            cached["artifacts"] = json!(attempt.dir);
            cached["provider"] = json!(name(r, legacy));
            attempt.finish("ok", cached.clone())?;
            ctx.report(&attempt, s(&cached, "output"), &cached)?;
            return Ok(cached);
        }
        eprintln!(
            "[{}] 缓存身份未匹配，本轮重新请求；旧Python/Node缓存不能直接复用",
            r.id
        );
    }
    if !args.has("ignore-cooldown")
        && let Some(entry) = blocked(&r.pool)
    {
        result["status"] = json!("cooldown");
        result["error"] = json!(format!(
            "额度池 {} 冷却中（{}）：{}",
            r.pool,
            s(&entry, "kind"),
            short(s(&entry, "detail"), 120)
        ));
        attempt.finish("cooldown", result.clone())?;
        return Ok(result);
    }
    let response = if let Some(alias) = &r.api_alias {
        api::execute_alias(config, alias, &api_args, prompt, &attempt).await
    } else {
        execute_cli_env(r, args, prompt, &attempt, Some(config)).await
    };
    match response {
        Ok(v) => {
            let complete = v["complete"].as_bool().unwrap_or(s(&v, "status") == "ok");
            let body = v["content"].as_str().unwrap_or("").trim();
            let status = if !s(&v, "status").is_empty() {
                s(&v, "status")
            } else if complete && !body.is_empty() {
                "ok"
            } else {
                "incomplete"
            };
            result["status"] = json!(status);
            result["output"] = json!(if status == "ok" && complete { body } else { "" });
            result["complete"] = json!(status == "ok" && complete);
            result["error"] = json!(s(&v, "error"));
            result["response"] = v;
        }
        Err(e) => {
            let detail = format!("{e:#}");
            result["status"] = json!(classify(&detail, 1, false, prompt));
            result["error"] = json!(detail);
        }
    }
    if result["status"] == "ok" && s(&result, "output").is_empty() {
        result["status"] = json!("empty");
        result["complete"] = json!(false);
        result["error"] = json!("没有完整最终正文");
    }
    result["seconds"] = json!((start.elapsed().as_secs_f64() * 10.0).round() / 10.0);
    if matches!(s(&result, "status"), "quota" | "auth" | "transport") {
        cool_down(&r.pool, s(&result, "status"), s(&result, "error"))?;
    }
    attempt.finish(s(&result, "status"), result.clone())?;
    if result["status"] == "ok" {
        let body = s(&result, "output");
        ctx.report(&attempt, body, &result)?;
        if let Some(path) = cache {
            runtime::cache_put(&path, &identity, &result, body)?;
        }
    }
    Ok(result)
}
fn without_output(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("output");
        if let Some(r) = o.get_mut("response").and_then(Value::as_object_mut) {
            r.remove("content");
            r.remove("reasoning");
        }
    }
    v
}
fn emit(ctx: &OutputContext, v: &Value, r: &Route) -> Result<()> {
    if v["cached"] == true {
        eprintln!("↩ {} 复用完整报告，未发新请求", r.id);
    }
    if let Some(dir) = &ctx.outdir {
        let path = dir.join(format!("{}.md", r.legacy_id));
        ctx.publish_to(
            &path,
            &format!(
                "<!-- {} · {}s -->\n\n{}\n",
                r.label,
                v["seconds"],
                s(v, "output")
            ),
        )?;
        eprintln!("✅ {} → {}", r.id, path.display());
    } else {
        println!(
            "\n{}\n## {} — {} ({}s)\n{}\n\n{}",
            "=".repeat(70),
            r.id,
            r.label,
            v["seconds"],
            "=".repeat(70),
            s(v, "output")
        );
    }
    Ok(())
}

pub async fn command(config: &Config, command: &str, args: &Args, legacy: bool) -> Result<i32> {
    match command {
        "run" => {
            let r = route(
                config,
                args.positional.first().context("run需要指定通路")?,
                legacy,
            )?;
            let ctx = OutputContext::new(args, command, false)?;
            let prompt = read_prompt(args, 1)?;
            let v = run_one(config, r, args, &prompt, &ctx, legacy).await?;
            ctx.save_status(without_output(&v))?;
            let rc = if v["status"] == "ok" {
                if ctx.output.is_some() {
                    ctx.publish(s(&v, "output"))?;
                } else if !args.has("json") {
                    println!("{}", s(&v, "output"));
                }
                if args.has("json") {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                }
                0
            } else {
                eprintln!(
                    "[{}] {}: {}",
                    name(r, legacy),
                    s(&v, "status"),
                    s(&v, "error")
                );
                1
            };
            ctx.finish(rc)?;
            Ok(rc)
        }
        "dispatch" | "review" => dispatch(config, args, legacy).await,
        "verify" => verify(config, args, legacy).await,
        "probe" => probe(config, args, legacy).await,
        "usage" => usage(config, args).await,
        "status" => {
            show_status(config, args)?;
            Ok(0)
        }
        "reset" => {
            let pool = args.positional.first();
            if !args.has("all") && pool.is_none() {
                bail!("reset需要pool或 --all");
            }
            if let Some(p) = pool
                && !config.routes.iter().any(|r| &r.pool == p)
            {
                bail!("未知额度池 {p}");
            }
            state_update(|s| {
                if args.has("all") {
                    s["pools"] = json!({});
                } else if let Some(o) = s["pools"].as_object_mut() {
                    o.remove(pool.unwrap());
                }
            })?;
            println!("已清冷却。");
            Ok(0)
        }
        _ => bail!("未知调度命令 {command}"),
    }
}

async fn dispatch(config: &Config, args: &Args, legacy: bool) -> Result<i32> {
    let kind = args.get("kind").unwrap_or("review");
    let (defaults, def_agentic, def_no_metered) = match kind {
        "explore" => (1, true, true),
        "review" => (2, false, false),
        "verify" => (1, true, false),
        _ => bail!("kind必须为explore、review或verify"),
    };
    let only = args.all("only");
    let exclude = args.all("exclude");
    for id in &only {
        route(config, id, legacy)?;
    }
    let want = if args.has("seats") {
        args.number("seats", defaults)? as usize
    } else if !only.is_empty() {
        only.iter()
            .filter_map(|id| route(config, id, legacy).ok())
            .map(|r| r.pool.clone())
            .collect::<HashSet<_>>()
            .len()
    } else {
        defaults as usize
    };
    if want == 0 {
        bail!("seats必须为正数");
    }
    let ctx = if args.has("dry-run") {
        None
    } else {
        Some(OutputContext::new(args, "dispatch", true)?)
    };
    let prompt = read_prompt(args, 0)?;
    let repo_only = args.has("cwd") && args.all("input").is_empty() && !args.has("text-ready");
    let agentic = args.has("agentic") || def_agentic || repo_only;
    let no_metered = args.has("no-metered") || def_no_metered;
    let quotas = if args.has("no-quota") {
        read_json(&quota_file())
            .map(|v| v["pools"].clone())
            .unwrap_or_else(|| json!({}))
    } else {
        collect_quotas().await?
    };
    let seats = pick_seats(
        config, want, &only, &exclude, agentic, no_metered, &quotas, legacy,
    );
    if seats.is_empty() {
        eprintln!("没有可用席位；查看multi-ai status。未冷却不代表已验证健康。");
        if let Some(c) = ctx {
            c.finish(1)?;
        }
        return Ok(1);
    }
    let mut queue = VecDeque::from(backups(
        config, &seats, &exclude, agentic, no_metered, &quotas, legacy,
    ));
    eprintln!(
        "分发（{kind}，目标 {want} 席）：{}",
        seats
            .iter()
            .map(|r| name(r, legacy))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if repo_only {
        eprintln!("cwd读取任务仅派可读仓库通路；文本通路须 --input 或 --text-ready。");
    }
    if kind == "review" {
        eprintln!("复核只附需求、契约和diff，不附实施结论或任务卡。");
    }
    if kind == "verify" && exclude.is_empty() {
        eprintln!("验证须 --exclude 出具方及同额度池，避免自己验证自己。");
    }
    for r in &seats {
        eprintln!(
            "  {} rank={} cost={} pool={}  {}",
            name(r, legacy),
            r.rank,
            r.cost,
            r.pool,
            format_windows(&quotas[&r.pool])
        );
    }
    if args.has("dry-run") {
        eprintln!("--dry-run只选池，未发送模型请求（额度采集仍可能联网）");
        return Ok(0);
    }
    let ctx = ctx.unwrap();
    let mut done = vec![];
    let mut results = vec![];
    let mut stopped = false;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut attempted: HashSet<String> = seats.iter().map(|r| r.id.clone()).collect();
    async fn job(
        config: &Config,
        r: Route,
        args: &Args,
        prompt: &str,
        ctx: &OutputContext,
        legacy: bool,
    ) -> (Route, Result<Value>) {
        let v = run_one(config, &r, args, prompt, ctx, legacy).await;
        (r, v)
    }
    let mut jobs = FuturesUnordered::new();
    for r in seats {
        jobs.push(job(config, r, args, &prompt, &ctx, legacy));
    }
    ctx.save_status(
        json!({"status":"running","targetSeats":want,"completedSeats":0,"results":[]}),
    )?;
    while !jobs.is_empty() {
        let next = tokio::select! {
            next=jobs.next()=>next,
            _=interrupt.recv(),if !stopped=>{stopped=true;queue.clear();None},
            _=terminate.recv(),if !stopped=>{stopped=true;queue.clear();None},
        };
        let Some((r, response)) = next else {
            continue;
        };
        let v=response.unwrap_or_else(|e|json!({"provider":name(&r,legacy),"route":r.id,"pool":r.pool,"family":r.family,"label":r.label,"status":"error","output":"","error":format!("{e:#}"),"seconds":0.0}));
        if v["status"] == "cancelled" {
            stopped = true;
            queue.clear();
        }
        if v["status"] == "ok" {
            emit(&ctx, &v, &r)?;
            done.push(v.clone());
        } else {
            eprintln!(
                "⚠ {} {}: {}；现场 {}",
                r.id,
                s(&v, "status"),
                short(s(&v, "error"), 200),
                s(&v, "artifacts")
            );
            if !stopped
                && matches!(
                    s(&v, "status"),
                    "quota"
                        | "auth"
                        | "transport"
                        | "timeout"
                        | "empty"
                        | "incomplete"
                        | "truncated"
                        | "cooldown"
                )
            {
                while let Some(next) = queue.pop_front() {
                    if attempted.insert(next.id.clone())
                        && eligible(&next, config, agentic, no_metered, &exclude, legacy)
                    {
                        eprintln!("↳ 立即换 {}（pool={}）", next.id, next.pool);
                        jobs.push(job(config, next, args, &prompt, &ctx, legacy));
                        break;
                    }
                }
            }
        }
        results.push(v);
        ctx.save_status(json!({"status":"running","targetSeats":want,"completedSeats":done.len(),"results":results.iter().map(without_output).collect::<Vec<_>>()}))?;
    }
    let complete = done.len() >= want && !stopped;
    ctx.save_status(json!({"status":if stopped{"cancelled"}else if complete{"complete"}else{"incomplete"},"targetSeats":want,"completedSeats":done.len(),"results":results.iter().map(without_output).collect::<Vec<_>>()}))?;
    let families: HashSet<&str> = done
        .iter()
        .map(|v| s(v, "family"))
        .filter(|s| !s.is_empty() && *s != "unknown")
        .collect();
    let pools: HashSet<&str> = done.iter().map(|v| s(v, "pool")).collect();
    eprintln!(
        "{}份报告 / {}个已配置模型家族 / {}个额度池；家族和钱包均不保证判断独立",
        done.len(),
        families.len(),
        pools.len()
    );
    if !complete {
        if stopped {
            eprintln!("本轮已取消，未再启动备援。");
        } else {
            eprintln!("仍缺{}席，整轮未完成", want.saturating_sub(done.len()));
        }
    }
    let rc = if complete { 0 } else { 1 };
    ctx.finish(rc)?;
    Ok(rc)
}
const VERIFY_TEMPLATE: &str = r#"你在做**对抗性验证**。下面是另一个模型对这个仓库出具的复核发现。
它可能对，也可能错 —— 你的任务是逐条判定真假，**不是重新做一遍复核**。

# 纪律（最重要的部分）

1. **每一条都要你自己去代码里取证**。不要因为它写得有条理就采信；也不要因为你没见过就否定。
2. 特别留意这三类错误：
   - **引用了不存在的东西**：文件、函数、字段、行号对不上；
   - **误读**：代码确实长那样，但它推出的结论不成立（比如漏看了上游已有的拦截）；
   - **前提本身不成立**：它假设的那张表/那个字段/那条路径在本仓根本不存在，
     或者早被另一处判据堵住了。
3. **判定用词固定**：
   - `CONFIRMED` —— 你独立取证后确认成立，附**你自己找到的**证据（`文件:行号` + 原文引用）；
   - `REFUTED` —— 不成立，写清它错在哪，附反证；
   - `UNCLEAR` —— 你没能确认，写清你查了什么、卡在哪。**不确定就用这个，不要猜。**
4. 不要修改任何文件。不要补充新的发现（那是另一轮的事）。

# 输出格式（严格遵守）

先给一张总表，再给详情，最后一行给汇总计数：

```
| # | 原发现（压缩成一句） | 判定 | 我的依据 |
|---|---|---|---|
| 1 | … | CONFIRMED | `文件:行号` |

## 详情
### 1. <原标题> — CONFIRMED / REFUTED / UNCLEAR
<你自己的取证过程与证据；REFUTED 必须给反证>

## 汇总
CONFIRMED=<n> REFUTED=<n> UNCLEAR=<n>
```

中文输出。

---

# 待验证的发现

"#;
async fn verify(config: &Config, args: &Args, legacy: bool) -> Result<i32> {
    let ctx = OutputContext::new(args, "verify", false)?;
    let findings = fs::read_to_string(args.get("findings").context("verify需要 --findings")?)?;
    if findings.trim().is_empty() {
        bail!("发现文件为空");
    }
    let exclude = args.all("exclude");
    let r = if let Some(by) = args.get("by") {
        let r = route(config, by, legacy)?;
        if !eligible(r, config, false, args.has("no-metered"), &exclude, legacy) {
            eprintln!("指定验证者与出具方共池、被排除或当前冷却中");
            ctx.finish(1)?;
            return Ok(1);
        }
        r.clone()
    } else {
        let q = if args.has("no-quota") {
            json!({})
        } else {
            collect_quotas().await?
        };
        let c = pick_seats(
            config,
            1,
            &[],
            &exclude,
            true,
            args.has("no-metered"),
            &q,
            legacy,
        );
        let Some(r) = c.first() else {
            eprintln!("没有可用验证者");
            ctx.finish(1)?;
            return Ok(1);
        };
        r.clone()
    };
    eprintln!(
        "验证者：{}（rank={}，{}）",
        name(&r, legacy),
        r.rank,
        r.label
    );
    if exclude.is_empty() {
        eprintln!("未声明出具方，请明确 --exclude；模型判定不等于人工采纳。");
    }
    let prompt = append_material(format!("{VERIFY_TEMPLATE}{findings}"), args)?;
    let v = run_one(config, &r, args, &prompt, &ctx, legacy).await?;
    ctx.save_status(without_output(&v))?;
    if v["status"] != "ok" {
        eprintln!("[{}] {}: {}", r.id, s(&v, "status"), s(&v, "error"));
        ctx.finish(1)?;
        return Ok(1);
    }
    let body = s(&v, "output");
    if ctx.output.is_some() {
        ctx.publish(&format!(
            "<!-- 验证者 {} · {}s -->\n\n{body}\n",
            name(&r, legacy),
            v["seconds"]
        ))?;
    } else if !args.has("json") {
        println!("{body}");
    }
    if args.has("json") {
        println!("{}", serde_json::to_string_pretty(&v)?);
    }
    if let Some(c) = Regex::new(r"CONFIRMED=(\d+)\s+REFUTED=(\d+)\s+UNCLEAR=(\d+)")
        .unwrap()
        .captures(body)
    {
        eprintln!(
            "汇总：CONFIRMED={} REFUTED={} UNCLEAR={}；这是模型取证判断，采纳仍须结合业务与验收证据",
            &c[1], &c[2], &c[3]
        );
    }
    ctx.finish(0)?;
    Ok(0)
}
async fn probe(config: &Config, args: &Args, legacy: bool) -> Result<i32> {
    let only = args.all("only");
    let targets = if only.is_empty() {
        let mut r = config.routes.clone();
        r.sort_by_key(|p| (p.rank, p.id.clone()));
        r
    } else {
        only.iter()
            .map(|id| route(config, id, legacy).cloned())
            .collect::<Result<Vec<_>>>()?
    };
    let temp = runtime::state_dir()
        .join("probe")
        .join(uuid::Uuid::new_v4().to_string());
    let mut a = args.clone();
    a.set("cwd", temp.display());
    if !a.has("timeout") {
        a.set("timeout", 120);
    }
    a.flags.insert("text-ready".into());
    a.flags.insert("ignore-cooldown".into());
    let ctx = OutputContext::new(&a, "probe", false)?;
    fs::create_dir_all(&temp)?;
    async fn job(
        config: &Config,
        r: Route,
        a: &Args,
        ctx: &OutputContext,
        legacy: bool,
    ) -> Result<Value> {
        run_one(config, &r, a, "只回复两个字：正常", ctx, legacy).await
    }
    let mut jobs = FuturesUnordered::new();
    for r in targets.clone() {
        jobs.push(job(config, r, &a, &ctx, legacy));
    }
    let mut results = BTreeMap::new();
    while let Some(v) = jobs.next().await {
        let v = v?;
        results.insert(s(&v, "route").to_owned(), v);
    }
    let ok: HashSet<&str> = results
        .values()
        .filter(|v| v["status"] == "ok")
        .map(|v| s(v, "pool"))
        .collect();
    if args.has("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"results":results,"availablePools":ok}))?
        );
    } else {
        for r in targets {
            let v = &results[&r.id];
            println!(
                "{} {} {:.1}s pool={} {}",
                if v["status"] == "ok" { "✅" } else { "❌" },
                name(&r, legacy),
                v["seconds"].as_f64().unwrap_or(0.0),
                r.pool,
                r.label
            );
            if v["status"] != "ok" {
                println!("  {}: {}", s(v, "status"), short(s(v, "error"), 200));
            }
        }
        println!("可用额度池 {}个", ok.len());
    }
    let rc = if ok.is_empty() { 1 } else { 0 };
    ctx.finish(rc)?;
    let _ = fs::remove_dir(&temp);
    Ok(rc)
}

fn empty_quota(error: &str, source: &str) -> Value {
    json!({"source":source,"fetched_at":iso(now()),"stale":false,"windows":[],"error":error})
}
async fn agy_quota() -> Value {
    let mut c = Command::new("agy");
    c.args([
        "-p",
        "/usage",
        "--output-format",
        "json",
        "--print-timeout",
        "20s",
    ])
    .kill_on_drop(true);
    let out = match tokio::time::timeout(Duration::from_secs(25), c.output()).await {
        Ok(Ok(o)) => o,
        _ => return empty_quota("无法读取Antigravity额度", "live"),
    };
    if !out.status.success() {
        return empty_quota("Antigravity额度命令失败", "live");
    }
    let data: Value = match serde_json::from_slice(&out.stdout) {
        Ok(v) => v,
        Err(_) => return empty_quota("agy输出不是JSON", "live"),
    };
    let mut w = vec![];
    for g in data["command"]["data"]["groups"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if !s(g, "name").contains("Gemini") {
            continue;
        }
        for b in g["buckets"].as_array().into_iter().flatten() {
            if let Some(n) = b["remaining_fraction"].as_f64() {
                let name = match s(b, "window") {
                    "weekly" | "7d" => "周",
                    "five_hour" | "5h" => "5h",
                    s => s,
                };
                w.push(json!({"name":name,"remaining":n,"reset":b["reset_time"]}));
            }
        }
    }
    if w.is_empty() {
        empty_quota("没有Gemini额度窗口", "live")
    } else {
        json!({"source":"live","fetched_at":iso(now()),"stale":false,"windows":w})
    }
}
async fn kimi_quota() -> Value {
    let Some(cred) = read_json(&home().join(".kimi-code/credentials/kimi-code.json")) else {
        return empty_quota("无可读登录凭据", "live");
    };
    if cred["expires_at"].as_f64().is_some_and(|t| t < now()) {
        return empty_quota(
            "access token已过期；CLI可能自动刷新，通路状态以小请求为准",
            "metadata",
        );
    }
    let Some(token) = cred["access_token"].as_str().filter(|s| !s.is_empty()) else {
        return empty_quota("无access_token", "live");
    };
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
    {
        Ok(c) => c,
        Err(_) => return empty_quota("额度客户端初始化失败", "live"),
    };
    let data = match client
        .get("https://api.kimi.com/coding/v1/usages")
        .bearer_auth(token)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => match r.json::<Value>().await {
            Ok(v) => v,
            Err(_) => return empty_quota("额度响应不可解析", "live"),
        },
        _ => return empty_quota("额度接口请求失败", "live"),
    };
    let mut w = vec![];
    for (k, name) in [("limit_5h", "5h"), ("limit_7d", "周")] {
        let row = &data["usages"][k];
        if let Some(r) = row["used_ratio"].as_f64() {
            w.push(json!({"name":name,"remaining":(1.0-r).max(0.0),"reset":row["reset_time"]}));
        }
    }
    if w.is_empty() {
        empty_quota("用量接口没有窗口", "live")
    } else {
        json!({"source":"live","fetched_at":iso(now()),"stale":false,"windows":w})
    }
}
fn codex_record() -> Value {
    fn walk(p: &Path, out: &mut Vec<PathBuf>) {
        if let Ok(entries) = fs::read_dir(p) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.file_name().is_some_and(|n| {
                    n.to_string_lossy().starts_with("rollout-")
                        && n.to_string_lossy().ends_with(".jsonl")
                }) {
                    out.push(p);
                }
            }
        }
    }
    let mut files = vec![];
    walk(&home().join(".codex/sessions"), &mut files);
    files.sort_by_key(|p| {
        std::cmp::Reverse(
            fs::metadata(p)
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH),
        )
    });
    for p in files.into_iter().take(40) {
        let Ok(text) = fs::read_to_string(&p) else {
            continue;
        };
        let Some(line) = text.lines().rfind(|l| l.contains("\"rate_limits\"")) else {
            continue;
        };
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let mut w = vec![];
        for k in ["primary", "secondary"] {
            let b = &event["payload"]["rate_limits"][k];
            if let Some(n) = b["used_percent"].as_f64() {
                let name = match b["window_minutes"].as_u64() {
                    Some(300) => "5h",
                    Some(10080) => "周",
                    _ => "额度",
                };
                w.push(json!({"name":name,"remaining":(1.0-n/100.0).max(0.0),"reset":b["resets_at"].as_f64().map(iso)}));
            }
        }
        if w.is_empty() {
            continue;
        }
        let observed = parse_time(&event["timestamp"])
            .or_else(|| {
                fs::metadata(&p)
                    .ok()?
                    .modified()
                    .ok()?
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs_f64())
            })
            .unwrap_or(0.0);
        let note = if w.iter().any(|v| v["name"] == "5h") {
            ""
        } else {
            "这次记录没有5小时窗口"
        };
        return json!({"source":"record","fetched_at":iso(now()),"observed_at":iso(observed),"stale":now()-observed>QUOTA_FRESH_SEC,"windows":w,"note":note});
    }
    empty_quota("最近会话里没有额度记录", "record")
}
async fn collect_quotas() -> Result<Value> {
    let (google, kimi) = tokio::join!(agy_quota(), kimi_quota());
    let codex = tokio::task::spawn_blocking(codex_record).await?;
    let q = json!({"google":google,"kimi-code":kimi,"openai":codex,"xai":empty_quota("只有单次会话token统计，没有账户额度","none"),"zhipu":empty_quota("没有公开余额接口","none")});
    runtime::atomic_json(&quota_file(), &json!({"fetched_at":iso(now()),"pools":q}))?;
    Ok(q)
}
fn format_windows(info: &Value) -> String {
    let Some(w) = info["windows"].as_array().filter(|a| !a.is_empty()) else {
        return if s(info, "error").is_empty() {
            "—".into()
        } else {
            s(info, "error").into()
        };
    };
    let items = w
        .iter()
        .map(|r| {
            let reset = parse_time(&r["reset"])
                .and_then(|t| DateTime::<Utc>::from_timestamp(t as i64, 0))
                .map(|t| format!("·{} UTC", t.format("%m-%d %H:%M")))
                .unwrap_or_default();
            format!(
                "{}剩{:.0}%{reset}",
                s(r, "name"),
                r["remaining"].as_f64().unwrap_or(0.0) * 100.0
            )
        })
        .collect::<Vec<_>>();
    format!(
        "{} [{}]",
        items.join("；"),
        if info["source"] == "record" {
            if info["stale"] == true {
                "记录已过期"
            } else {
                "会话记录"
            }
        } else {
            "实时"
        }
    )
}
fn show_status(config: &Config, args: &Args) -> Result<()> {
    let state = load_state();
    if args.has("json") {
        println!("{}", serde_json::to_string_pretty(&state)?);
        return Ok(());
    }
    let mut pools = BTreeMap::<String, Vec<&Route>>::new();
    for r in &config.routes {
        pools.entry(r.pool.clone()).or_default().push(r);
    }
    println!("额度池冷却状态（未冷却不等于已验证健康；cost是本地计费标注）：");
    for (pool, members) in pools {
        let e = &state["pools"][&pool];
        let names = members
            .iter()
            .map(|r| r.legacy_id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if e["until"].as_f64().unwrap_or(0.0) > now() {
            println!(
                "🧊 {pool} 冷却中 {:.0}分钟 ({}) {names}\n  {}",
                ((e["until"].as_f64().unwrap() - now()) / 60.0).max(0.0),
                s(e, "kind"),
                short(s(e, "detail"), 160)
            );
        } else {
            println!("  {pool} 未冷却 {names}");
        }
    }
    Ok(())
}
async fn balance(pool: &str, config: &Config) -> String {
    let (url, keys) = match pool {
        "deepseek-api" => (
            "https://api.deepseek.com/user/balance",
            vec!["DEEPSEEK_API_KEY"],
        ),
        "moonshot-api" => (
            "https://api.moonshot.cn/v1/users/me/balance",
            vec!["KIMI_API_KEY", "MOONSHOT_API_KEY"],
        ),
        _ => return "—".into(),
    };
    let env = config.environment();
    let Some(key) = keys
        .iter()
        .find_map(|k| env.get(*k))
        .filter(|s| !s.is_empty())
    else {
        return "无key".into();
    };
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
    {
        Ok(c) => c,
        Err(_) => return "查询失败（客户端）".into(),
    };
    let v = match client.get(url).bearer_auth(key).send().await {
        Ok(r) if r.status().is_success() => r.json::<Value>().await.ok(),
        _ => None,
    };
    let Some(v) = v else {
        return "查询失败（接口）".into();
    };
    if pool == "deepseek-api" {
        format!(
            "{} {}{}",
            s(&v["balance_infos"][0], "currency"),
            s(&v["balance_infos"][0], "total_balance"),
            if v["is_available"] == true {
                ""
            } else {
                "（不可用）"
            }
        )
    } else {
        format!(
            "CNY {:.2}",
            v["data"]["available_balance"].as_f64().unwrap_or(0.0)
        )
    }
}
async fn usage(config: &Config, args: &Args) -> Result<i32> {
    let q = collect_quotas().await?;
    let mut pools = BTreeMap::<String, Vec<&Route>>::new();
    for r in &config.routes {
        pools.entry(r.pool.clone()).or_default().push(r);
    }
    let mut ordered = pools.into_iter().collect::<Vec<_>>();
    ordered.sort_by_key(|(_, m)| m.iter().map(|r| r.rank).min().unwrap_or(100));
    let mut rows = vec![];
    for (pool, members) in ordered {
        let rank = members.iter().map(|r| r.rank).min().unwrap_or(100);
        let metered = members.iter().any(|r| r.cost == "metered");
        let quota = &q[&pool];
        let text = if quota["windows"].as_array().is_some_and(|a| !a.is_empty()) {
            format_windows(quota)
        } else if metered {
            let b = balance(&pool, config).await;
            if b == "—" && !s(quota, "error").is_empty() {
                s(quota, "error").into()
            } else {
                b
            }
        } else {
            format_windows(quota)
        };
        rows.push(json!({"pool":pool,"rank":rank,"cost":if metered{"metered"}else{"sub"},"quota":quota,"display":text,"cooldown":blocked(&pool),"penalty":quota_penalty(&pool,&q),"routes":members.iter().map(|r|r.id.clone()).collect::<Vec<_>>()}));
    }
    if args.has("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"pools":rows,"snapshot":quota_file()}))?
        );
    } else {
        println!("额度与余额（rank小的先用；完整额度低于40%且另有池高于70%才让位）：");
        for r in rows {
            println!(
                "{} rank={} {} {}\n  {}",
                s(&r, "pool"),
                r["rank"],
                s(&r, "cost"),
                s(&r, "display"),
                r["routes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        println!("快照已写入 {}；缺5h或周窗口只展示", quota_file().display());
    }
    Ok(0)
}

//! Best-effort cross-session lifecycle events, compatible with ai-ledger.mjs.
use serde_json::{Value, json};
use std::{env, fs::OpenOptions, io::Write, path::PathBuf, process::Command};

pub(crate) fn tracked(command: &str) -> bool {
    matches!(command, "delegate" | "batch" | "review" | "run" | "verify")
}

pub(crate) fn ledger_file() -> Option<PathBuf> {
    match env::var_os("AI_LEDGER_FILE") {
        Some(v) if v.is_empty() || v == "off" => None,
        Some(v) => Some(PathBuf::from(v)),
        None => {
            Some(PathBuf::from(env::var_os("HOME")?).join(".local/state/ai-ledger/events.jsonl"))
        }
    }
}

fn hostname() -> String {
    Command::new("hostname")
        .output()
        .ok()
        .filter(|v| v.status.success())
        .map(|v| String::from_utf8_lossy(&v.stdout).trim().to_owned())
        .unwrap_or_default()
}

/// Unfinished multi-ai starts recorded on this host, oldest first.
fn unfinished() -> anyhow::Result<Vec<Value>> {
    let file = ledger_file().ok_or_else(|| anyhow::anyhow!("ledger is disabled"))?;
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let events: Vec<Value> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let finished: std::collections::HashSet<&str> = events
        .iter()
        .filter(|e| e["event"] == "finish")
        .filter_map(|e| e["id"].as_str())
        .collect();
    let host = hostname();
    Ok(events
        .iter()
        .filter(|e| e["event"] == "start" && e["actor"] == "multi-ai" && e["host"] == host.as_str())
        .filter(|e| e["id"].as_str().is_some_and(|id| !finished.contains(id)))
        .cloned()
        .collect())
}

/// The pid still runs the recorded multi-ai subcommand; a reused pid is never signalled.
fn still_running(event: &Value) -> Option<i32> {
    let pid = event["pid"].as_i64()? as i32;
    let out = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .ok()?;
    let command = String::from_utf8_lossy(&out.stdout);
    let sub = event["what"].as_str()?.split_whitespace().next()?;
    let mut words = command.split_whitespace();
    let program = words.next()?;
    (["multi-ai", "ai-review"]
        .iter()
        .any(|name| program.ends_with(name))
        && words.next() == Some(sub))
    .then_some(pid)
}

/// stop [ID|ID-prefix] [--list]: SIGTERM one unfinished multi-ai run of this host. Each
/// subcommand then kills its own vendor process groups; nothing else is signalled.
pub(crate) fn stop_command(args: &crate::args::Args) -> anyhow::Result<i32> {
    let open = unfinished()?;
    let wanted = args.positional.first();
    if args.has("list") || wanted.is_none() {
        for event in &open {
            println!(
                "{}\t{}\t{}\t{}",
                event["id"].as_str().unwrap_or("-"),
                if still_running(event).is_some() {
                    "running"
                } else {
                    "gone"
                },
                event["at"].as_str().unwrap_or("-"),
                event["what"].as_str().unwrap_or("-")
            );
        }
        return Ok(0);
    }
    let wanted = wanted.unwrap();
    let matches: Vec<&Value> = open
        .iter()
        .filter(|e| {
            e["id"]
                .as_str()
                .is_some_and(|id| id.starts_with(wanted.as_str()))
        })
        .collect();
    let event = match matches.as_slice() {
        [one] => *one,
        [] => anyhow::bail!("本机没有未结束的 multi-ai 运行 {wanted}；multi-ai stop --list 查看"),
        _ => anyhow::bail!("{wanted} 匹配到 {} 个运行，请给完整编号", matches.len()),
    };
    let Some(pid) = still_running(event) else {
        anyhow::bail!("运行 {wanted} 的进程已不在，账本缺结束记录；无需结束");
    };
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        anyhow::bail!(
            "向 PID {pid} 发送 SIGTERM 失败：{}",
            std::io::Error::last_os_error()
        );
    }
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if still_running(event).is_none() {
            println!("已结束 {} (PID {pid})", event["id"].as_str().unwrap_or("-"));
            return Ok(0);
        }
    }
    anyhow::bail!("已向 PID {pid} 发送 SIGTERM，10 秒后仍在运行")
}

pub(crate) struct Ledger {
    file: PathBuf,
    id: String,
    cwd: PathBuf,
    out: String,
}

fn git(cwd: &PathBuf, args: &[&str]) -> String {
    let mut command = Command::new("git");
    command.current_dir(cwd).args(args);
    // Repository identity must describe cwd, even under a caller's Git environment.
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command
        .output()
        .ok()
        .filter(|v| v.status.success())
        .map(|v| String::from_utf8_lossy(&v.stdout).trim().to_owned())
        .unwrap_or_default()
}

impl Ledger {
    pub(crate) fn start(argv: &[String]) -> Option<Self> {
        let executable = argv
            .first()
            .and_then(|s| std::path::Path::new(s).file_name());
        if executable.is_some_and(|v| v == "llm-run") {
            return None;
        }
        let command = argv.get(1)?;
        if !tracked(command) {
            return None;
        }
        let file = ledger_file()?;
        // Read metadata even when the command will subsequently fail argument validation.
        let option = |name: &str| -> Option<String> {
            let key = format!("--{name}");
            let prefix = format!("{key}=");
            let mut found = None;
            for (i, arg) in argv.iter().enumerate().skip(2) {
                if arg == "--" {
                    break;
                }
                if let Some(value) = arg.strip_prefix(&prefix) {
                    found = Some(value.to_owned());
                } else if arg == &key || (name == "out" && arg == "-o") {
                    found = argv.get(i + 1).filter(|v| !v.starts_with("--")).cloned();
                }
            }
            found
        };
        let cwd = env::current_dir()
            .unwrap_or_default()
            .join(option("cwd").unwrap_or_default());
        let ledger = Self {
            file,
            id: format!(
                "{}-{}-{}",
                chrono::Utc::now().format("%Y%m%dT%H%M%S"),
                std::process::id(),
                uuid::Uuid::new_v4()
            ),
            cwd,
            out: option("out")
                .or_else(|| option("outdir"))
                .unwrap_or_default(),
        };
        let parsed = crate::args::Args::parse(&argv[2..]).unwrap_or_default();
        let task_type = option("task-type").or_else(|| match command.as_str() {
            "delegate"
                if parsed
                    .positional
                    .first()
                    .or_else(|| argv.get(2))
                    .is_some_and(|v| v == "implement") =>
            {
                Some("implement".into())
            }
            "review" => Some("review".into()),
            "verify" => Some("verify".into()),
            _ => None,
        });
        let host = hostname();
        let what = std::iter::once(command.as_str())
            .chain(parsed.positional.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ");
        let mut event = json!({"event":"start", "actor":"multi-ai", "what":what,
            "cwd":ledger.cwd, "out":ledger.out, "pid":std::process::id(), "host":host,
            "branch":git(&ledger.cwd, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "head":git(&ledger.cwd, &["rev-parse", "HEAD"]), "task_type":task_type});
        if let Some(card) = option("card").or_else(|| env::var("AI_LEDGER_CARD").ok()) {
            event["card"] = json!(card);
        }
        ledger.append(event);
        Some(ledger)
    }

    pub(crate) fn finish(self, exit: i32) {
        self.append(json!({"event":"finish", "exit":exit,
            "head":git(&self.cwd, &["rev-parse", "HEAD"]), "out":self.out}));
    }

    fn append(&self, mut event: Value) {
        event["v"] = json!(1);
        event["at"] = json!(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
        event["id"] = json!(self.id);
        let result = (|| -> anyhow::Result<()> {
            if let Some(parent) = self.file.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.file)?;
            let mut line = serde_json::to_vec(&event)?;
            line.push(b'\n');
            // One O_APPEND write per complete event; never interleave a write_all retry.
            if file.write(&line)? != line.len() {
                anyhow::bail!("short ledger write");
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!(
                "multi-ai: warning: ledger append failed ({}): {error:#}",
                self.file.display()
            );
        }
    }
}

//! Best-effort cross-session lifecycle events, compatible with ai-ledger.mjs.
use serde_json::{Value, json};
use std::{env, fs::OpenOptions, io::Write, path::PathBuf, process::Command};

pub(crate) fn tracked(command: &str) -> bool {
    matches!(command, "delegate" | "batch" | "review" | "run" | "verify")
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
        let file = match env::var_os("AI_LEDGER_FILE") {
            Some(v) if v.is_empty() || v == "off" => return None,
            Some(v) => PathBuf::from(v),
            None => PathBuf::from(env::var_os("HOME")?).join(".local/state/ai-ledger/events.jsonl"),
        };
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
        let host = Command::new("hostname")
            .output()
            .ok()
            .filter(|v| v.status.success())
            .map(|v| String::from_utf8_lossy(&v.stdout).trim().to_owned())
            .unwrap_or_default();
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

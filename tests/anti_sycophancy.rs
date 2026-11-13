//! Anti-sycophancy safeguards: reviewer family exclusion, blind review packets,
//! disagreement verification list, decoy canaries, sycophancy rate and dissent seat.
//! Every test uses fake CLI routes under a temporary HOME/state; nothing reaches a network.
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_multi-ai")
}

struct Fixture {
    temp: TempDir,
    config: PathBuf,
    state: PathBuf,
    cwd: PathBuf,
    ledger: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config");
        let state = temp.path().join("state");
        let cwd = temp.path().join("cwd");
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        // Echo the prompt into the report so tests can see exactly what a model received.
        let script = temp.path().join("echo.sh");
        fs::write(&script, "printf 'route %s\\n%s' \"$ROUTE_TAG\" \"$1\"\n").unwrap();
        let route = |id: &str, pool: &str, family: &str, rank: u32| {
            json!({"id":id,"legacy_id":id,"pool":pool,"family":family,"label":id,"agentic":true,"cost":"sub","rank":rank,
                "argv":["/bin/sh",script,"{prompt}","{outfile}"],"extract":"stdout","default_timeout":10})
        };
        let routes = json!([
            route("impl-cli", "pool-a", "openai", 1),
            route("kimi-cli", "pool-b", "kimi", 2),
            route("glm-cli", "pool-c", "glm", 3),
        ]);
        fs::write(
            config.join("routes.json"),
            serde_json::to_vec(&routes).unwrap(),
        )
        .unwrap();
        let ledger = temp.path().join("ledger.jsonl");
        Self {
            temp,
            config,
            state,
            cwd,
            ledger,
        }
    }
    fn command(&self) -> Command {
        let mut c = Command::new(binary());
        c.env_clear()
            .current_dir(&self.cwd)
            .env("HOME", self.temp.path())
            .env("MULTI_AI_CONFIG_DIR", &self.config)
            .env("MULTI_AI_STATE_DIR", &self.state)
            .env("AI_LEDGER_FILE", &self.ledger)
            .env("PATH", "/usr/bin:/bin");
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
    fn write(&self, name: &str, text: &str) -> String {
        let path = self.temp.path().join(name);
        fs::write(&path, text).unwrap();
        path.display().to_string()
    }
    fn path(&self, name: &str) -> PathBuf {
        self.temp.path().join(name)
    }
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}
fn ok(o: &Output) {
    assert!(
        o.status.success(),
        "status={} stderr={}",
        o.status,
        stderr(o)
    );
}
fn failure(o: &Output) {
    assert!(
        !o.status.success(),
        "unexpected success: {}",
        String::from_utf8_lossy(&o.stdout)
    );
}
/// Seats announced by `review` on stderr ("分发（review，目标 N 席）：a, b").
fn seats(o: &Output) -> Vec<String> {
    let text = stderr(o);
    let line = text
        .lines()
        .find(|l| l.starts_with("分发"))
        .unwrap_or_else(|| panic!("no seat line: {text}"));
    line.split('：')
        .nth(1)
        .unwrap()
        .split(", ")
        .map(str::to_owned)
        .collect()
}
fn value(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

// ---- 1. reviewer family exclusion -------------------------------------------------------

#[test]
fn exclude_family_drops_every_route_of_that_family() {
    let f = Fixture::new();
    let input = f.write("material.md", "diff --git a/x b/x\n+fn x() {}\n");
    let base = [
        "review",
        "--input",
        &input,
        "--text-ready",
        "--seats",
        "2",
        "--no-quota",
        "--dry-run",
    ];
    let o = f.run(&base);
    ok(&o);
    assert_eq!(seats(&o), ["impl-cli", "kimi-cli"]);
    let mut args = base.to_vec();
    args.extend(["--exclude-family", "openai"]);
    let o = f.run(&args);
    ok(&o);
    assert_eq!(seats(&o), ["kimi-cli", "glm-cli"]);
    assert!(stderr(&o).contains("排除模型家族 openai"), "{}", stderr(&o));
    // Several families in one flag, and an explicitly selected excluded route has no seat.
    let o = f.run(&[
        "review",
        "--input",
        &input,
        "--text-ready",
        "--no-quota",
        "--dry-run",
        "--only",
        "impl-cli",
        "--exclude-family",
        "openai",
    ]);
    failure(&o);
    assert!(
        stderr(&o).contains("已排除模型家族 openai"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn ledger_implement_record_excludes_implementer_family_by_default() {
    let f = Fixture::new();
    let input = f.write("material.md", "diff\n");
    fs::write(
        &f.ledger,
        json!({"v":1,"event":"start","actor":"multi-ai","what":"delegate implement impl-cli",
            "task_type":"implement","cwd":f.cwd,"id":"x"})
        .to_string()
            + "\n",
    )
    .unwrap();
    let cwd = f.cwd.display().to_string();
    let base = [
        "review",
        "--cwd",
        &cwd,
        "--input",
        &input,
        "--text-ready",
        "--seats",
        "2",
        "--no-quota",
        "--dry-run",
    ];
    let o = f.run(&base);
    ok(&o);
    assert_eq!(seats(&o), ["kimi-cli", "glm-cli"]);
    assert!(stderr(&o).contains("账本显示"), "{}", stderr(&o));
    let mut args = base.to_vec();
    args.push("--allow-same-family");
    let o = f.run(&args);
    ok(&o);
    assert_eq!(seats(&o), ["impl-cli", "kimi-cli"]);
    assert!(stderr(&o).contains("已放开同家族评审"), "{}", stderr(&o));
    // Explicit exclusions are not lifted by --allow-same-family.
    args.extend(["--exclude-family", "openai"]);
    assert_eq!(seats(&f.run(&args)), ["kimi-cli", "glm-cli"]);
}

#[test]
fn author_run_family_is_looked_up_from_recorded_state() {
    let f = Fixture::new();
    let input = f.write("material.md", "diff\n");
    let run = f.state.join("runs/run-0001");
    fs::create_dir_all(&run).unwrap();
    fs::write(
        run.join("manifest.json"),
        json!({"run_id":"run-0001","command":"implement","task_type":"implement","reports":[],
            "current_status":{"route":"impl-cli","family":"openai","status":"ok"},"outputs":{}})
        .to_string(),
    )
    .unwrap();
    let o = f.run(&[
        "review",
        "--input",
        &input,
        "--text-ready",
        "--seats",
        "2",
        "--no-quota",
        "--dry-run",
        "--author-run",
        "run-0001",
    ]);
    ok(&o);
    assert_eq!(seats(&o), ["kimi-cli", "glm-cli"]);
    assert!(
        stderr(&o).contains("--author-run run-0001"),
        "{}",
        stderr(&o)
    );
    // An unknown author run fails before seats are chosen or any model is called.
    let o = f.run(&[
        "review",
        "--input",
        &input,
        "--text-ready",
        "--no-quota",
        "--dry-run",
        "--author-run",
        "missing",
    ]);
    failure(&o);
    assert!(!stderr(&o).contains("分发"), "{}", stderr(&o));
}

#[test]
fn verify_rejects_a_verifier_of_the_excluded_family() {
    let f = Fixture::new();
    let findings = f.write("findings.md", "| src/a.rs:1 | 真问题 |\n");
    let out = f.path("verify.md").display().to_string();
    let o = f.run(&[
        "verify",
        "--findings",
        &findings,
        "--by",
        "impl-cli",
        "--exclude-family",
        "openai",
        "--no-quota",
        "--out",
        &out,
    ]);
    failure(&o);
    assert!(stderr(&o).contains("同家族"), "{}", stderr(&o));
    assert!(!Path::new(&out).exists());
    // Without --by, the verifier is picked outside the excluded family.
    let o = f.run(&[
        "verify",
        "--findings",
        &findings,
        "--exclude-family",
        "openai",
        "--no-quota",
        "--out",
        &out,
    ]);
    ok(&o);
    assert!(stderr(&o).contains("验证者：kimi-cli"), "{}", stderr(&o));
    let _ = value(&PathBuf::from(format!("{out}.status.json")));
}

use multi_ai::{
    config::{Config, Route},
    review::{classify, pick_seats, quota_penalty, quota_schedulable},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_multi-ai")
}
fn make_route(id: &str, pool: &str, rank: u32, script: &Path, extract: &str) -> Value {
    json!({"id":id,"legacy_id":id,"pool":pool,"family":"test","label":id,"agentic":true,"cost":"sub","rank":rank,"argv":["/bin/sh",script,"{prompt}","{outfile}"],"extract":extract,"default_timeout":5})
}
struct Fixture {
    temp: TempDir,
    config: PathBuf,
    state: PathBuf,
    cwd: PathBuf,
}
impl Fixture {
    fn new(routes: Value) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config");
        let state = temp.path().join("state");
        let cwd = temp.path().join("cwd");
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            config.join("routes.json"),
            serde_json::to_vec(&routes).unwrap(),
        )
        .unwrap();
        Self {
            temp,
            config,
            state,
            cwd,
        }
    }
    fn command(&self) -> Command {
        let mut c = Command::new(binary());
        c.env_clear()
            .current_dir(&self.cwd)
            .env("HOME", self.temp.path())
            .env("MULTI_AI_CONFIG_DIR", &self.config)
            .env("MULTI_AI_STATE_DIR", &self.state)
            .env("PATH", "/usr/bin:/bin");
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
}
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, body).unwrap();
    p
}
fn ok(o: &Output) {
    assert!(
        o.status.success(),
        "status={} stderr={}",
        o.status,
        String::from_utf8_lossy(&o.stderr)
    );
}
fn failure(o: &Output) {
    assert!(
        !o.status.success(),
        "unexpected success: {}",
        String::from_utf8_lossy(&o.stdout)
    );
}
fn value(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn classification_keeps_context_and_echo_out_of_cooldown() {
    assert_eq!(
        classify("maximum context length exceeded 429", 1, false, ""),
        "error"
    );
    assert_eq!(
        classify(
            "request input 402 and 429\nunsupported model",
            1,
            false,
            "request input 402 and 429"
        ),
        "error"
    );
    assert_eq!(
        classify("Error [truncated]: exhausted output budget", 1, false, ""),
        "truncated"
    );
    assert_eq!(
        classify(
            "Error [cancelled]: Request cancelled by SIGTERM",
            1,
            false,
            ""
        ),
        "cancelled"
    );
    assert_eq!(classify("Please log in; 401", 1, false, ""), "auth");
    assert_eq!(classify("quota exhausted", 1, false, ""), "quota");
    assert_eq!(classify("connection reset", 1, false, ""), "transport");
    assert_eq!(
        classify(
            r#"[claude-code:unrecognized_model] {"model":"deepseek-v4-pro"}"#,
            1,
            false,
            ""
        ),
        "model"
    );
    assert_eq!(
        classify("The model `kimi-x` does not exist", 1, false, ""),
        "model"
    );
    assert_eq!(classify("file does not exist", 1, false, ""), "error");
    assert_eq!(classify("429", 0, false, ""), "ok");
    assert_eq!(classify("401", 1, true, ""), "timeout");
}
#[test]
fn quota_requires_two_fresh_windows_and_thresholds() {
    let live = |a: f64, b: f64| json!({"source":"live","stale":false,"windows":[{"name":"5h","remaining":a},{"name":"周","remaining":b}]});
    assert!(quota_schedulable(&live(0.8, 0.9)));
    assert!(!quota_schedulable(
        &json!({"source":"record","stale":false,"windows":[{"name":"周","remaining":0.99}]})
    ));
    assert!(!quota_schedulable(
        &json!({"source":"record","stale":true,"windows":[{"name":"5h","remaining":0.9},{"name":"周","remaining":0.9}]})
    ));
    assert_eq!(
        quota_penalty("low", &json!({"low":live(0.3,0.8),"high":live(0.8,0.9)})),
        1
    );
    assert_eq!(
        quota_penalty("low", &json!({"low":live(0.4,0.8),"high":live(0.8,0.9)})),
        0
    );
    assert_eq!(
        quota_penalty("low", &json!({"low":live(0.3,0.8),"high":live(0.7,0.9)})),
        0
    );
}
#[test]
fn selection_obeys_rank_pool_exclusion_and_only_order() {
    let temp = tempfile::tempdir().unwrap();
    let path = script(temp.path(), "ok.sh", "printf ok\n");
    let mut config = Config::load().unwrap();
    config.routes = serde_json::from_value(json!([
        make_route("third", "selection-test-c", 30, &path, "stdout"),
        make_route("first", "selection-test-a", 10, &path, "stdout"),
        make_route("same-wallet", "selection-test-a", 12, &path, "stdout"),
        make_route("second", "selection-test-b", 20, &path, "stdout")
    ]))
    .unwrap();
    let ids = |r: Vec<Route>| r.into_iter().map(|p| p.id).collect::<Vec<_>>();
    assert_eq!(
        ids(pick_seats(
            &config,
            4,
            &[],
            &[],
            false,
            false,
            &json!({}),
            false
        )),
        ["first", "second", "third"]
    );
    assert_eq!(
        ids(pick_seats(
            &config,
            4,
            &[],
            &["same-wallet".into()],
            false,
            false,
            &json!({}),
            false
        )),
        ["second", "third"]
    );
    assert_eq!(
        ids(pick_seats(
            &config,
            4,
            &["third".into(), "same-wallet".into(), "first".into()],
            &[],
            false,
            false,
            &json!({}),
            false
        )),
        ["third", "same-wallet"]
    );
}
#[test]
fn repeated_input_contents_are_passed_not_just_paths() {
    let temp = tempfile::tempdir().unwrap();
    let path = script(temp.path(), "echo.sh", "printf '%s' \"$1\"\n");
    let f = Fixture::new(json!([make_route(
        "echo-cli",
        "echo-test",
        1,
        &path,
        "stdout"
    )]));
    let one = f.cwd.join("one.md");
    let two = f.cwd.join("two.md");
    fs::write(&one, "evidence-one").unwrap();
    fs::write(&two, "evidence-two").unwrap();
    let out = f.run(&[
        "run",
        "echo-cli",
        "task",
        "--input",
        one.to_str().unwrap(),
        "--input",
        two.to_str().unwrap(),
    ]);
    ok(&out);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("evidence-one") && text.contains("evidence-two") && text.contains("task")
    );
}
#[test]
fn jsonl_last_message_with_tool_calls_is_not_a_final_answer() {
    let temp = tempfile::tempdir().unwrap();
    let path = script(
        temp.path(),
        "tool.sh",
        r#"printf '%s\n' '{"role":"assistant","content":"not final"}' '{"role":"assistant","content":"tool phase","tool_calls":[{"name":"read"}]}'
"#,
    );
    let f = Fixture::new(json!([make_route(
        "json-cli",
        "json-test",
        1,
        &path,
        "jsonl-assistant"
    )]));
    let out = f.run(&["run", "json-cli", "task"]);
    failure(&out);
    assert!(String::from_utf8_lossy(&out.stderr).contains("empty"));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("not final"));
}
#[test]
fn file_extractor_keeps_stdout_noise_out_of_report() {
    let temp = tempfile::tempdir().unwrap();
    let path = script(
        temp.path(),
        "file.sh",
        "printf noise\nprintf final > \"$2\"\n",
    );
    let f = Fixture::new(json!([make_route(
        "file-cli",
        "file-test",
        1,
        &path,
        "last-message-file"
    )]));
    let out = f.run(&["run", "file-cli", "task"]);
    ok(&out);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "final");
}
#[test]
fn exact_resume_is_zero_calls_but_changed_prompt_executes() {
    let temp = tempfile::tempdir().unwrap();
    let count = temp.path().join("count");
    let path = script(
        temp.path(),
        "count.sh",
        &format!("echo call >> '{}'\nprintf complete\n", count.display()),
    );
    let f = Fixture::new(json!([make_route(
        "cache-cli",
        "cache-test",
        1,
        &path,
        "stdout"
    )]));
    let out = f.cwd.join("answer.md");
    let base = [
        "run",
        "cache-cli",
        "task",
        "--out",
        out.to_str().unwrap(),
        "--cwd",
        f.cwd.to_str().unwrap(),
        "--source-id",
        "immutable-source",
    ];
    ok(&f.run(&base));
    let mut resume = base.to_vec();
    resume.push("--resume");
    let cached = f.run(&resume);
    ok(&cached);
    assert_eq!(fs::read_to_string(&count).unwrap().lines().count(), 1);
    assert_eq!(
        value(&PathBuf::from(format!("{}.status.json", out.display())))["cached"],
        true
    );
    resume[2] = "changed task";
    ok(&f.run(&resume));
    assert_eq!(fs::read_to_string(&count).unwrap().lines().count(), 2);
    assert_eq!(fs::read_to_string(out).unwrap(), "complete");
    assert!(f.state.join("history").read_dir().unwrap().count() >= 1);
}
#[test]
fn timeout_kills_subprocess_group_including_grandchildren() {
    let temp = tempfile::tempdir().unwrap();
    let orphan = temp.path().join("orphan");
    let pidfile = temp.path().join("descendant-pid");
    let path = script(
        temp.path(),
        "tree.sh",
        &format!(
            "(sleep 20; echo escaped > '{}') &\necho $! > '{}'\nwait\n",
            orphan.display(),
            pidfile.display()
        ),
    );
    let f = Fixture::new(json!([make_route(
        "tree-cli",
        "tree-test",
        1,
        &path,
        "stdout"
    )]));
    let report = f.cwd.join("timeout.md");
    let out = f.run(&[
        "run",
        "tree-cli",
        "task",
        "--timeout",
        "5",
        "--out",
        report.to_str().unwrap(),
    ]);
    failure(&out);
    let status = value(&PathBuf::from(format!("{}.status.json", report.display())));
    assert_eq!(status["status"], "timeout");
    assert!(
        status["response"]["seconds"].as_f64().unwrap() < 5.5,
        "request exceeded its own budget"
    );
    let pid: i32 = fs::read_to_string(pidfile).unwrap().trim().parse().unwrap();
    let start = Instant::now();
    while unsafe { libc::kill(pid, 0) } == 0 && start.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_ne!(
        unsafe { libc::kill(pid, 0) },
        0,
        "grandchild survived timeout"
    );
    assert!(
        !orphan.exists(),
        "grandchild escaped process group cancellation"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("timeout"));
}
#[test]
fn dispatch_immediately_starts_backup_while_other_initial_seat_waits() {
    let temp = tempfile::tempdir().unwrap();
    let fast = script(
        temp.path(),
        "quota.sh",
        "echo '429 quota exhausted' >&2\nexit 1\n",
    );
    let slow_started = temp.path().join("slow-started");
    let slow_finished = temp.path().join("slow-finished");
    let slow = script(
        temp.path(),
        "slow.sh",
        &format!(
            "echo started > '{}'\nsleep 2\necho done > '{}'\nprintf slow-complete\n",
            slow_started.display(),
            slow_finished.display()
        ),
    );
    let marker = temp.path().join("backup-started");
    let backup = script(
        temp.path(),
        "backup.sh",
        &format!(
            "echo started > '{}'\nprintf backup-complete\n",
            marker.display()
        ),
    );
    let f = Fixture::new(json!([
        make_route("fast-fail", "fast-wallet", 1, &fast, "stdout"),
        make_route("slow-cli", "slow-wallet", 2, &slow, "stdout"),
        make_route("backup-cli", "backup-wallet", 3, &backup, "stdout")
    ]));
    let outdir = f.cwd.join("reports");
    let mut c = f.command();
    c.args([
        "review",
        "task",
        "--seats",
        "2",
        "--no-quota",
        "--outdir",
        outdir.to_str().unwrap(),
    ]);
    let mut child = c.spawn().unwrap();
    let start = Instant::now();
    while !marker.exists() && start.elapsed() < Duration::from_secs(8) {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !marker.exists() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("backup did not start");
    }
    assert!(
        !slow_finished.exists(),
        "backup waited for unrelated slow initial seat"
    );
    assert!(child.wait().unwrap().success());
    let status = value(&outdir.join("dispatch.json"));
    assert_eq!(status["completedSeats"], 2);
    assert_eq!(status["status"], "complete");
    assert!(outdir.join("slow-cli.md").exists() && outdir.join("backup-cli.md").exists());
    assert_eq!(
        value(&f.state.join("review/state.json"))["pools"]["fast-wallet"]["kind"],
        "quota"
    );
}
#[test]
fn insufficient_successful_seats_is_nonzero_and_old_report_survives() {
    let temp = tempfile::tempdir().unwrap();
    let path = script(temp.path(), "error.sh", "echo unsupported >&2\nexit 1\n");
    let f = Fixture::new(json!([make_route(
        "error-cli",
        "error-wallet",
        1,
        &path,
        "stdout"
    )]));
    let outdir = f.cwd.join("reports");
    fs::create_dir_all(&outdir).unwrap();
    fs::write(outdir.join("error-cli.md"), "previous complete report").unwrap();
    let out = f.run(&[
        "review",
        "task",
        "--seats",
        "2",
        "--no-quota",
        "--outdir",
        outdir.to_str().unwrap(),
    ]);
    failure(&out);
    assert_eq!(value(&outdir.join("dispatch.json"))["status"], "incomplete");
    assert_eq!(
        fs::read_to_string(outdir.join("error-cli.md")).unwrap(),
        "previous complete report"
    );
}
#[test]
fn verify_attaches_findings_and_evidence_and_excludes_same_wallet() {
    let temp = tempfile::tempdir().unwrap();
    let path = script(temp.path(), "echo.sh", "printf '%s' \"$1\"\n");
    let f = Fixture::new(json!([
        make_route("author-cli", "same-wallet", 1, &path, "stdout"),
        make_route("verify-cli", "same-wallet", 2, &path, "stdout")
    ]));
    let findings = f.cwd.join("findings.md");
    fs::write(&findings, "finding-x").unwrap();
    let out = f.run(&[
        "verify",
        "--findings",
        findings.to_str().unwrap(),
        "--by",
        "verify-cli",
        "--exclude",
        "author-cli",
    ]);
    failure(&out);
    let out = f.run(&[
        "verify",
        "--findings",
        findings.to_str().unwrap(),
        "--by",
        "verify-cli",
    ]);
    ok(&out);
    assert!(String::from_utf8_lossy(&out.stdout).contains("finding-x"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("未声明出具方"));
}

#[test]
fn cancellation_stops_dispatch_without_launching_backup() {
    let temp = tempfile::tempdir().unwrap();
    let started = temp.path().join("started");
    let orphan = temp.path().join("orphan");
    let backup_started = temp.path().join("backup");
    let slow = script(
        temp.path(),
        "slow.sh",
        &format!(
            "echo ready > '{}'\n(sleep 2; echo escaped > '{}') &\nwait\n",
            started.display(),
            orphan.display()
        ),
    );
    let backup = script(
        temp.path(),
        "backup.sh",
        &format!(
            "echo launched > '{}'\nprintf backup\n",
            backup_started.display()
        ),
    );
    let f = Fixture::new(json!([
        make_route("slow-cli", "cancel-wallet", 1, &slow, "stdout"),
        make_route("backup-cli", "backup-wallet", 2, &backup, "stdout")
    ]));
    let outdir = f.cwd.join("reports");
    let mut command = f.command();
    command
        .args([
            "review",
            "task",
            "--seats",
            "1",
            "--no-quota",
            "--outdir",
            outdir.to_str().unwrap(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().unwrap();
    let start = Instant::now();
    while !started.exists() && start.elapsed() < Duration::from_secs(8) {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !started.exists() {
        let _ = child.kill();
        let output = child.wait_with_output().unwrap();
        panic!(
            "CLI failed to start: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let output = child.wait_with_output().unwrap();
    failure(&output);
    assert_eq!(value(&outdir.join("dispatch.json"))["status"], "cancelled");
    assert!(!backup_started.exists());
    std::thread::sleep(Duration::from_millis(2200));
    assert!(!orphan.exists());
}

#[test]
fn parallel_pool_failures_preserve_both_cooldowns() {
    let temp = tempfile::tempdir().unwrap();
    let fail = script(
        temp.path(),
        "fail.sh",
        "echo '429 quota exhausted' >&2\nexit 1\n",
    );
    let f = Fixture::new(json!([
        make_route("first-cli", "first-wallet", 1, &fail, "stdout"),
        make_route("second-cli", "second-wallet", 2, &fail, "stdout")
    ]));
    let out = f.run(&["review", "task", "--seats", "2", "--no-quota"]);
    failure(&out);
    let state = value(&f.state.join("review/state.json"));
    assert_eq!(state["pools"]["first-wallet"]["kind"], "quota");
    assert_eq!(state["pools"]["second-wallet"]["kind"], "quota");
}

#[test]
fn damaged_cache_is_never_reused() {
    let temp = tempfile::tempdir().unwrap();
    let count = temp.path().join("count");
    let path = script(
        temp.path(),
        "count.sh",
        &format!("echo call >> '{}'\nprintf complete\n", count.display()),
    );
    let f = Fixture::new(json!([make_route(
        "cache-cli",
        "cache-test",
        1,
        &path,
        "stdout"
    )]));
    let out = f.cwd.join("answer.md");
    let base = [
        "run",
        "cache-cli",
        "task",
        "--out",
        out.to_str().unwrap(),
        "--source-id",
        "immutable-source",
    ];
    ok(&f.run(&base));
    let cache = PathBuf::from(format!("{}.cache.json", out.display()));
    let mut row = value(&cache);
    row["content"] = json!("tampered");
    fs::write(&cache, serde_json::to_vec(&row).unwrap()).unwrap();
    let mut resume = base.to_vec();
    resume.push("--resume");
    ok(&f.run(&resume));
    assert_eq!(fs::read_to_string(&count).unwrap().lines().count(), 2);
}

#[test]
fn public_cli_failure_never_prints_configured_key() {
    let temp = tempfile::tempdir().unwrap();
    let path = script(
        temp.path(),
        "secret.sh",
        "printf '%s' \"$DEEPSEEK_API_KEY\" >&2\nexit 1\n",
    );
    let f = Fixture::new(json!([make_route(
        "secret-cli",
        "secret-test",
        1,
        &path,
        "stdout"
    )]));
    let key = "private-secret-value-for-test";
    let out = f
        .command()
        .env("DEEPSEEK_API_KEY", key)
        .args(["run", "secret-cli", "task"])
        .output()
        .unwrap();
    failure(&out);
    assert!(!String::from_utf8_lossy(&out.stdout).contains(key));
    assert!(!String::from_utf8_lossy(&out.stderr).contains(key));
    assert!(String::from_utf8_lossy(&out.stderr).contains("[REDACTED]"));
}

#[test]
fn changing_wrapper_implementation_invalidates_same_source_cache() {
    let temp = tempfile::tempdir().unwrap();
    let count = temp.path().join("count");
    let path = script(
        temp.path(),
        "implementation.sh",
        &format!("echo call >> '{}'\nprintf version-one\n", count.display()),
    );
    let f = Fixture::new(json!([make_route(
        "cache-cli",
        "implementation-test",
        1,
        &path,
        "stdout"
    )]));
    let output = f.cwd.join("answer.md");
    let base = [
        "run",
        "cache-cli",
        "task",
        "--out",
        output.to_str().unwrap(),
        "--source-id",
        "immutable-source",
    ];
    ok(&f.run(&base));
    let mut resume = base.to_vec();
    resume.push("--resume");
    ok(&f.run(&resume));
    assert_eq!(fs::read_to_string(&count).unwrap().lines().count(), 1);
    fs::write(
        &path,
        format!("echo call >> '{}'\nprintf version-two\n", count.display()),
    )
    .unwrap();
    ok(&f.run(&resume));
    assert_eq!(fs::read_to_string(&count).unwrap().lines().count(), 2);
    assert_eq!(fs::read_to_string(&output).unwrap(), "version-two");
}

#[test]
fn changed_default_model_config_invalidates_cache_without_recording_secrets() {
    let temp = tempfile::tempdir().unwrap();
    let count = temp.path().join("count");
    let path = script(
        temp.path(),
        "model.sh",
        &format!("echo call >> '{}'\nprintf complete\n", count.display()),
    );
    let mut configured = make_route("codex-fixture", "model-config-test", 1, &path, "stdout");
    configured["legacy_id"] = json!("codex");
    let f = Fixture::new(json!([configured]));
    let directory = f.temp.path().join(".codex");
    fs::create_dir_all(&directory).unwrap();
    let model = directory.join("config.toml");
    let secret = "virtual-config-secret-for-test";
    fs::write(
        &model,
        format!("model = \"first-model\"\napi_key = \"{secret}\"\n"),
    )
    .unwrap();
    let output = f.cwd.join("answer.md");
    let base = [
        "run",
        "codex-fixture",
        "task",
        "--out",
        output.to_str().unwrap(),
        "--source-id",
        "immutable-source",
    ];
    ok(&f.run(&base));
    let mut resume = base.to_vec();
    resume.push("--resume");
    ok(&f.run(&resume));
    assert_eq!(fs::read_to_string(&count).unwrap().lines().count(), 1);
    fs::write(
        &model,
        format!("model = \"second-model\"\napi_key = \"{secret}\"\n"),
    )
    .unwrap();
    ok(&f.run(&resume));
    assert_eq!(fs::read_to_string(&count).unwrap().lines().count(), 2);
    let cache = PathBuf::from(format!("{}.cache.json", output.display()));
    assert!(!fs::read_to_string(cache).unwrap().contains(secret));
}

#[test]
fn explicit_k3_identity_can_resume_without_default_model_config() {
    let temp = tempfile::tempdir().unwrap();
    let count = temp.path().join("count");
    let path = script(
        temp.path(),
        "k3.sh",
        &format!("echo call >> '{}'\nprintf complete\n", count.display()),
    );
    let mut configured = make_route("k3-fixture", "k3-cache-test", 1, &path, "stdout");
    configured["legacy_id"] = json!("kimi-k3");
    configured["argv"]
        .as_array_mut()
        .unwrap()
        .extend([json!("-m"), json!("kimi-code/k3")]);
    let f = Fixture::new(json!([configured]));
    let output = f.cwd.join("answer.md");
    let base = [
        "run",
        "k3-fixture",
        "task",
        "--out",
        output.to_str().unwrap(),
        "--source-id",
        "immutable-source",
    ];
    ok(&f.run(&base));
    let mut resume = base.to_vec();
    resume.push("--resume");
    ok(&f.run(&resume));
    assert_eq!(fs::read_to_string(&count).unwrap().lines().count(), 1);
}

#[test]
fn unknown_vendor_default_does_not_reuse_old_report() {
    let temp = tempfile::tempdir().unwrap();
    let count = temp.path().join("count");
    let path = script(
        temp.path(),
        "unknown.sh",
        &format!("echo call >> '{}'\nprintf complete\n", count.display()),
    );
    let mut configured = make_route("codex-fixture", "unknown-cache-test", 1, &path, "stdout");
    configured["legacy_id"] = json!("codex");
    let f = Fixture::new(json!([configured]));
    let output = f.cwd.join("answer.md");
    let base = [
        "run",
        "codex-fixture",
        "task",
        "--out",
        output.to_str().unwrap(),
        "--source-id",
        "immutable-source",
    ];
    ok(&f.run(&base));
    let mut resume = base.to_vec();
    resume.push("--resume");
    let out = f.run(&resume);
    ok(&out);
    assert_eq!(fs::read_to_string(&count).unwrap().lines().count(), 2);
    assert!(String::from_utf8_lossy(&out.stderr).contains("不能可靠确定"));
    assert!(!PathBuf::from(format!("{}.cache.json", output.display())).exists());
}

fn all_files(dir: &Path, files: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                all_files(&path, files);
            } else {
                files.push(path);
            }
        }
    }
}

#[test]
fn known_key_split_across_chunks_is_masked_in_raw_and_partial_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let ready = temp.path().join("ready");
    let path = script(
        temp.path(),
        "split.sh",
        &format!(
            r#"printf '{{"role":"assistant","content":"%s"}}\n' "$DEEPSEEK_API_KEY"
printf '{{"role":"assistant","content":"split-pri'
printf 'split-pri' >&2
echo ready > '{}'
sleep 0.4
printf 'vate-key-for-test-429"}}\n'
printf 'vate-key-for-test-429' >&2
"#,
            ready.display()
        ),
    );
    let f = Fixture::new(json!([make_route(
        "json-cli",
        "raw-redaction-test",
        1,
        &path,
        "jsonl-assistant"
    )]));
    let output = f.cwd.join("answer.md");
    let key = "split-private-key-for-test-429";
    let mut command = f.command();
    command
        .env("DEEPSEEK_API_KEY", key)
        .args([
            "run",
            "json-cli",
            "task",
            "--out",
            output.to_str().unwrap(),
            "--json",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().unwrap();
    let start = Instant::now();
    while !ready.exists() && start.elapsed() < Duration::from_secs(8) {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ready.exists());
    let progress = PathBuf::from(format!("{}.progress", output.display()));
    let mut files = vec![];
    all_files(&progress, &mut files);
    for file in files {
        let bytes = fs::read(&file).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(key),
            "key present in live artifact {}",
            file.display()
        );
    }
    let out = child.wait_with_output().unwrap();
    ok(&out);
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(result["output"].as_str().unwrap().contains("[REDACTED]"));
    assert!(result["run_id"].is_string() && result["attempt_id"].is_string());
    let mut files = vec![];
    all_files(&progress, &mut files);
    let mut checked_partial = false;
    for file in files {
        let bytes = fs::read(&file).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(key),
            "key present in final artifact {}",
            file.display()
        );
        if file.file_name().is_some_and(|n| n == "answer.partial.txt") {
            checked_partial = true;
            assert!(String::from_utf8_lossy(&bytes).contains("[REDACTED]"));
        }
    }
    assert!(checked_partial);
}

#[test]
fn last_message_staging_file_is_masked_after_body_extraction() {
    let temp = tempfile::tempdir().unwrap();
    let path = script(
        temp.path(),
        "file-key.sh",
        "printf '%s' \"$DEEPSEEK_API_KEY\" > \"$2\"\n",
    );
    let f = Fixture::new(json!([make_route(
        "file-cli",
        "file-redaction-test",
        1,
        &path,
        "last-message-file"
    )]));
    let output = f.cwd.join("answer.md");
    let key = "virtual-staged-key-for-test";
    let out = f
        .command()
        .env("DEEPSEEK_API_KEY", key)
        .args(["run", "file-cli", "task", "--out", output.to_str().unwrap()])
        .output()
        .unwrap();
    ok(&out);
    let mut files = vec![];
    all_files(
        &PathBuf::from(format!("{}.progress", output.display())),
        &mut files,
    );
    assert!(
        files
            .iter()
            .any(|p| p.file_name().is_some_and(|n| n == "last-message.txt"))
    );
    for file in files {
        assert!(
            !String::from_utf8_lossy(&fs::read(&file).unwrap()).contains(key),
            "key in {}",
            file.display()
        );
    }
    assert_eq!(fs::read_to_string(output).unwrap(), "[REDACTED]");
}

#[test]
fn kimi_fixed_tail_reports_matching_quota_line_in_status() {
    let tail = include_str!("fixtures/kimi-5h-stderr.txt");
    assert_eq!(
        classify(
            " M request-429-1302.rs\n?? quota exhausted.txt\nunsupported model",
            1,
            false,
            ""
        ),
        "error"
    );
    let reason = "APIStatusError: 403 You've reached your 5-hour usage limit";
    assert_eq!(classify(tail, 1, false, ""), "quota");
    assert_eq!(
        multi_ai::review::failure_reason(tail, "", "quota", ""),
        reason
    );
    assert_eq!(
        classify("429 code 1302 too many requests", 1, false, ""),
        "rate_limit"
    );
    assert_eq!(
        classify(
            r#"{"code":"insufficient_quota","message":"You exceeded your current quota"}"#,
            1,
            false,
            ""
        ),
        "quota"
    );
    assert_eq!(
        classify(
            "Error [quota] is quoted task text\nunsupported model",
            1,
            false,
            "Error [quota] is quoted task text"
        ),
        "error"
    );
    let temp = tempfile::tempdir().unwrap();
    let fixture = temp.path().join("stderr");
    fs::write(&fixture, tail).unwrap();
    let path = script(
        temp.path(),
        "quota.sh",
        &format!("cat '{}' >&2\nexit 1\n", fixture.display()),
    );
    let f = Fixture::new(json!([make_route(
        "kimi-fixture",
        "fixture-pool",
        1,
        &path,
        "stdout"
    )]));
    let out = f.cwd.join("report.md");
    failure(&f.run(&[
        "run",
        "kimi-fixture",
        "task",
        "--out",
        out.to_str().unwrap(),
    ]));
    let status = value(&PathBuf::from(format!("{}.status.json", out.display())));
    assert_eq!(status["status"], "quota");
    assert_eq!(status["error"], reason);
    assert!(!out.exists());
}

#[test]
fn auth_tokens_require_boundaries() {
    for text in ["x401y", "1403", "unforbidden", "authentication_notes"] {
        assert_eq!(classify(text, 1, false, ""), "error", "{text}");
    }
    for text in ["HTTP 401", "403 Forbidden", "authentication failed"] {
        assert_eq!(classify(text, 1, false, ""), "auth", "{text}");
    }
}

#[test]
fn capacity_diagnostics_have_distinct_status() {
    for text in [
        "at capacity",
        "overloaded",
        "server is busy",
        "HTTP 503",
        "try a different model",
        "temporarily unavailable",
    ] {
        assert_eq!(classify(text, 1, false, ""), "capacity", "{text}");
    }
}

#[test]
fn vendor_diagnostics_exclude_tools_and_assistant_text() {
    for (id, stdout, expected) in [
        (
            "other",
            "{\"type\":\"unknown\",\"message\":{\"role\":\"tool\",\"content\":\"401 Unauthorized\"},\"error\":{\"message\":\"ordinary failure\"}}",
            "error",
        ),
        (
            "other",
            "{\"type\":\"vendor.failure\",\"message\":\"401 Unauthorized\"}",
            "auth",
        ),
        (
            "codex-cli",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\",\"aggregated_output\":\"<tool_result>\"}}\n{\"type\":\"error\",\"message\":\"401 Unauthorized\"}",
            "auth",
        ),
        (
            "codex-cli",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"401 Unauthorized\"}}\n{\"type\":\"turn.failed\",\"error\":{\"message\":\"ordinary failure\"}}",
            "error",
        ),
        (
            "grok-cli",
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"content\":\"401 Unauthorized\"}]}}\n{\"type\":\"result\",\"is_error\":true,\"errors\":[\"overloaded\"]}",
            "capacity",
        ),
        (
            "kimi-k3-cli",
            "{\"role\":\"tool\",\"content\":\"401 Unauthorized\"}\n{\"type\":\"error\",\"error\":{\"message\":\"ordinary failure\"}}",
            "error",
        ),
        (
            "other",
            "<tool_result>\n{\"type\":\"error\",\"message\":\"401 Unauthorized\"}\n</tool_result>\nordinary failure",
            "error",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let p = script(
            temp.path(),
            "fake.sh",
            &format!("cat <<'FIXTURE'\n{stdout}\nFIXTURE\nexit 1\n"),
        );
        let f = Fixture::new(json!([make_route(id, "pool", 1, &p, "stdout")]));
        failure(&f.run(&[
            "run",
            id,
            "task",
            "--out",
            "answer.md",
            "--rate-limit-retries",
            "0",
        ]));
        let result = value(&f.cwd.join("answer.md.status.json"));
        assert_eq!(result["status"], expected, "{id}");
        assert!(
            expected == "auth" || !result["error"].as_str().unwrap().contains("401"),
            "{id}: {result}"
        );
    }
}

fn shell_route(script: &Path, record: &Path) -> Value {
    let mut route = make_route("shell-cli", "deepseek-api", 1, script, "stdout");
    route["argv"] = json!(["/bin/sh", script, record]);
    route["anthropic_provider"] = json!("deepseek");
    route
}

#[test]
fn claude_shell_route_gets_only_its_provider_endpoint_and_key() {
    let temp = tempfile::tempdir().unwrap();
    let record = temp.path().join("env.txt");
    let p = script(temp.path(), "claude.sh", "env > \"$1\"\necho done\n");
    let f = Fixture::new(json!([shell_route(&p, &record)]));
    let key = "deepseek-key-for-shell-test";
    ok(&f
        .command()
        .env("DEEPSEEK_API_KEY", key)
        .env("ZHIPU_API_KEY", "other-provider-key")
        .env("ANTHROPIC_API_KEY", "host-anthropic-key")
        .env("CLAUDECODE", "1")
        .env("CLAUDE_CODE_ENTRYPOINT", "host")
        .args(["run", "shell-cli", "task", "--out", "answer.md"])
        .output()
        .unwrap());
    let env = fs::read_to_string(&record).unwrap();
    for line in [
        "ANTHROPIC_BASE_URL=https://api.deepseek.com/anthropic",
        &format!("ANTHROPIC_AUTH_TOKEN={key}"),
        "ANTHROPIC_MODEL=deepseek-v4-pro",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL=deepseek-flash",
        "CLAUDE_CODE_SUBAGENT_MODEL=deepseek-v4-pro",
    ] {
        assert!(env.lines().any(|l| l == line), "missing {line}");
    }
    for name in [
        "DEEPSEEK_API_KEY=",
        "ZHIPU_API_KEY=",
        "ANTHROPIC_API_KEY=",
        "CLAUDECODE=",
        "CLAUDE_CODE_ENTRYPOINT=",
    ] {
        assert!(!env.lines().any(|l| l.starts_with(name)), "leaked {name}");
    }
    let dir = env
        .lines()
        .find_map(|l| l.strip_prefix("CLAUDE_CONFIG_DIR="))
        .unwrap();
    assert!(Path::new(dir).starts_with(&f.state), "{dir}");
    assert!(
        !Path::new(dir).exists(),
        "private Claude config must be removed: {dir}"
    );
    let routes: Value = serde_json::from_slice(&f.run(&["routes", "--json"]).stdout).unwrap();
    assert_eq!(routes[0]["model"], "deepseek-v4-pro");
    assert_eq!(routes[0]["model_source"], "provider:deepseek");
}

#[test]
fn claude_shell_route_without_key_never_launches() {
    let temp = tempfile::tempdir().unwrap();
    let record = temp.path().join("env.txt");
    let p = script(temp.path(), "claude.sh", "env > \"$1\"\n");
    let f = Fixture::new(json!([shell_route(&p, &record)]));
    failure(&f.run(&["run", "shell-cli", "task", "--out", "answer.md"]));
    assert!(!record.exists());
    let result = value(&f.cwd.join("answer.md.status.json"));
    assert_eq!(result["status"], "missing");
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("DEEPSEEK_API_KEY")
    );
}

#[test]
fn arrears_cool_down_until_reset_and_status_says_recharge() {
    let temp = tempfile::tempdir().unwrap();
    let p = script(
        temp.path(),
        "arrears.sh",
        "echo 'HTTP 429: Your account org-x is suspended due to insufficient balance, please recharge your account' >&2\nexit 1\n",
    );
    let f = Fixture::new(json!([make_route("owed", "owed-pool", 1, &p, "stdout")]));
    failure(&f.run(&["run", "owed", "task", "--out", "answer.md"]));
    assert_eq!(
        value(&f.cwd.join("answer.md.status.json"))["status"],
        "quota"
    );
    let state = value(&f.state.join("review/state.json"));
    let pool = &state["pools"]["owed-pool"];
    assert_eq!(pool["billing"], true);
    let days = (pool["until"].as_f64().unwrap() - pool_now()) / 86400.0;
    assert!(days > 6.0, "{pool}");
    let status = String::from_utf8_lossy(&f.run(&["status"]).stdout).into_owned();
    assert!(
        status.contains("欠费停用") && status.contains("multi-ai reset owed-pool"),
        "{status}"
    );
    ok(&f.run(&["reset", "owed-pool"]));
    let status = String::from_utf8_lossy(&f.run(&["status"]).stdout).into_owned();
    assert!(status.contains("owed-pool 未冷却"), "{status}");
}

#[test]
fn ordinary_quota_keeps_short_cooldown() {
    let temp = tempfile::tempdir().unwrap();
    let p = script(
        temp.path(),
        "quota.sh",
        "echo 'quota exceeded' >&2\nexit 1\n",
    );
    let f = Fixture::new(json!([make_route("q", "q-pool", 1, &p, "stdout")]));
    failure(&f.run(&["run", "q", "task", "--out", "answer.md"]));
    let pool = &value(&f.state.join("review/state.json"))["pools"]["q-pool"];
    assert_eq!(pool["billing"], false);
    assert!(
        pool["until"].as_f64().unwrap() - pool_now() < 2.0 * 3600.0,
        "{pool}"
    );
}

fn pool_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

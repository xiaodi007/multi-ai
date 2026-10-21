use serde_json::{Value, json};
use std::{
    fs,
    os::fd::AsRawFd,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn hold_state_lock(root: &std::path::Path) -> fs::File {
    fs::create_dir_all(root.join("state/review")).unwrap();
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("state/review/state.lock"))
        .unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    lock
}

fn wait_for(path: &std::path::Path) {
    let started = Instant::now();
    while !path.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "missing fixture marker"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Fixture {
    root: tempfile::TempDir,
}
impl Fixture {
    fn new(scripts: &[(&str, &str, u64)]) -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("config")).unwrap();
        let routes=scripts.iter().map(|(id,body,timeout)| {
            let script=root.path().join(format!("{id}.sh"));fs::write(&script,body).unwrap();
            json!({"id":id,"legacy_id":id,"pool":id,"family":"test","label":id,"agentic":true,"cost":"sub","rank":1,"argv":["/bin/sh",script,"{prompt}"],"extract":"stdout","default_timeout":timeout})
        }).collect::<Vec<_>>();
        fs::write(
            root.path().join("config/routes.json"),
            serde_json::to_vec(&routes).unwrap(),
        )
        .unwrap();
        Self { root }
    }
    fn cmd(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_multi-ai"));
        c.env_clear()
            .env("HOME", self.root.path())
            .env("PATH", "/usr/bin:/bin")
            .env("MULTI_AI_CONFIG_DIR", self.root.path().join("config"))
            .env("MULTI_AI_STATE_DIR", self.root.path().join("state"))
            .current_dir(self.root.path());
        c
    }
    fn status(&self) -> Value {
        serde_json::from_slice(&fs::read(self.root.path().join("answer.md.status.json")).unwrap())
            .unwrap()
    }
}
#[test]
fn fallback_shares_seconds_budget_and_records_real_timeout() {
    let f = Fixture::new(&[
        (
            "quota",
            "sleep 3\necho '403 5-hour usage limit' >&2\nexit 1\n",
            10,
        ),
        ("backup", "sleep 4.5\nprintf complete\n", 30),
    ]);
    let start = Instant::now();
    let out = f
        .cmd()
        .args([
            "run",
            "quota",
            "task",
            "--fallback",
            "backup",
            "--timeout",
            "6",
            "--out",
            "answer.md",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let status = f.status();
    assert_eq!(status["status"], "timeout", "{status}");
    assert_eq!(status["route_attempts"].as_array().unwrap().len(), 2);
    assert!(start.elapsed() < Duration::from_millis(8500));
    assert!(!f.root.path().join("answer.md").exists());
}
#[test]
fn sigterm_during_backoff_stops_without_second_call() {
    let f = Fixture::new(&[(
        "rate",
        "echo call >> calls\necho '429 code 1302' >&2\nexit 1\n",
        30,
    )]);
    let mut child = f
        .cmd()
        .args([
            "run",
            "rate",
            "task",
            "--out",
            "answer.md",
            "--retry-delay-ms",
            "10000",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !f.root.path().join("calls").exists() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(400));
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let start = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if start.elapsed() > Duration::from_secs(2) {
            child.kill().unwrap();
            panic!("backoff ignored SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(f.status()["status"], "cancelled");
    assert_eq!(
        fs::read_to_string(f.root.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}
#[test]
fn backup_cache_restores_with_different_default_timeouts() {
    let f = Fixture::new(&[
        ("quota", "echo 'quota exhausted' >&2\nexit 1\n", 10),
        ("backup", "echo call >> calls\nprintf complete\n", 30),
    ]);
    let args = [
        "run",
        "quota",
        "task",
        "--fallback",
        "backup",
        "--out",
        "answer.md",
        "--source-id",
        "fixed",
    ];
    let out = f.cmd().args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = f.cmd().args(args).arg("--resume").output().unwrap();
    assert!(out.status.success());
    assert_eq!(f.status()["cached"], true);
    assert_eq!(
        fs::read_to_string(f.root.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}
#[test]
fn batch_preflights_all_outputs_before_first_adapter_call() {
    let f = Fixture::new(&[("fake", "echo call >> calls\nprintf complete\n", 10)]);
    fs::write(f.root.path().join("p"), "fixture").unwrap();
    fs::create_dir(f.root.path().join("out")).unwrap();
    let target = f.root.path().join("foreign");
    fs::write(&target, "preserve").unwrap();
    std::os::unix::fs::symlink(&target, f.root.path().join("out/b.md")).unwrap();
    fs::write(f.root.path().join("jobs.json"),serde_json::to_vec(&json!({"jobs":[{"id":"a","route":"fake","cwd":".","prompt_file":"p"},{"id":"b","route":"fake","cwd":".","prompt_file":"p"}]})).unwrap()).unwrap();
    let out = f
        .cmd()
        .args(["batch", "--jobs", "jobs.json", "--outdir", "out"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!f.root.path().join("calls").exists());
    assert_eq!(fs::read_to_string(target).unwrap(), "preserve");
    let out = f
        .cmd()
        .args([
            "batch",
            "--jobs",
            "jobs.json",
            "--outdir",
            "out",
            "--pool-limit",
            "fake=18446744073709551615",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("semaphore capacity"));
    assert!(!f.root.path().join("calls").exists());
}

#[test]
fn review_and_verify_retry_rate_limit_before_success() {
    for command in ["review", "verify"] {
        let f = Fixture::new(&[(
            "rate",
            "n=$(cat calls 2>/dev/null || echo 0); n=$((n+1)); echo \"$n\" > calls\nif [ \"$n\" -eq 1 ]; then echo '429 code 1302 too many requests' >&2; exit 1; fi\nprintf complete\n",
            10,
        )]);
        let mut cmd = f.cmd();
        if command == "review" {
            cmd.args([
                "review",
                "task",
                "--only",
                "rate",
                "--seats",
                "1",
                "--no-quota",
                "--outdir",
                "reviews",
            ]);
        } else {
            fs::write(f.root.path().join("findings.md"), "A fictional finding.").unwrap();
            cmd.args([
                "verify",
                "--findings",
                "findings.md",
                "--by",
                "rate",
                "--no-quota",
                "--out",
                "answer.md",
            ]);
        }
        let out = cmd
            .args(["--rate-limit-retries", "2", "--retry-delay-ms", "10"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            fs::read_to_string(f.root.path().join("calls"))
                .unwrap()
                .trim(),
            "2",
            "{command}"
        );
        let result = if command == "review" {
            let status: Value = serde_json::from_slice(
                &fs::read(f.root.path().join("reviews/dispatch.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(status["status"], "complete");
            status["results"][0].clone()
        } else {
            let body = fs::read_to_string(f.root.path().join("answer.md")).unwrap();
            assert!(body.starts_with("<!-- 验证者 rate · "));
            assert!(body.trim().ends_with("complete"));
            f.status()
        };
        assert_eq!(result["status"], "ok", "{command}");
        assert_eq!(result["attempts"].as_array().unwrap().len(), 2, "{command}");
        assert!(
            !f.root.path().join("state/review/state.json").exists(),
            "{command}"
        );
    }
}

#[test]
fn expired_run_budget_does_not_launch_fallback_after_slow_finalization() {
    let f = Fixture::new(&[
        (
            "quota",
            "echo call > quota-calls\necho 'quota exhausted' >&2\nexit 1\n",
            10,
        ),
        ("backup", "echo call > backup-calls\nprintf complete\n", 10),
    ]);
    let lock = hold_state_lock(f.root.path());
    let child = f
        .cmd()
        .args([
            "run",
            "quota",
            "task",
            "--fallback",
            "backup",
            "--timeout",
            "3",
            "--out",
            "answer.md",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for(&f.root.path().join("quota-calls"));
    // The completed quota response is waiting to persist cooldown, beyond its task deadline.
    std::thread::sleep(Duration::from_millis(3300));
    drop(lock);
    assert!(!child.wait_with_output().unwrap().status.success());
    assert_eq!(f.status()["status"], "timeout");
    assert!(!f.root.path().join("backup-calls").exists());
}

#[test]
fn batch_fallback_pool_wait_uses_started_tasks_remaining_budget() {
    let f = Fixture::new(&[
        (
            "primary",
            "echo call > primary-calls\necho 'quota exhausted' >&2\nexit 1\n",
            10,
        ),
        (
            "backup",
            "echo call >> backup-calls\nsleep 10\nprintf complete\n",
            10,
        ),
    ]);
    fs::write(f.root.path().join("p"), "fictional task").unwrap();
    fs::write(
        f.root.path().join("jobs.json"),
        serde_json::to_vec(&json!({
            "jobs":[
                {"id":"waiting","route":"primary","fallback":["backup"],"cwd":".","prompt_file":"p"},
                {"id":"occupy","route":"backup","cwd":".","prompt_file":"p"}
            ],"pool_limits":{"backup":1,"primary":1}
        }))
        .unwrap(),
    )
    .unwrap();
    let result = f
        .cmd()
        .args([
            "batch",
            "--jobs",
            "jobs.json",
            "--outdir",
            "out",
            "--timeout",
            "3",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(
        fs::read_to_string(f.root.path().join("backup-calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert_eq!(
        fs::read_to_string(f.root.path().join("primary-calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    let waiting: Value = serde_json::from_slice(
        &fs::read(f.root.path().join("out/waiting.md.status.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(waiting["status"], "timeout");
    assert_eq!(waiting["model_calls"], 0);
}

#[test]
fn invalid_task_budgets_never_invoke_adapters() {
    let f = Fixture::new(&[("fake", "echo call >> calls\nprintf complete\n", 10)]);
    fs::write(f.root.path().join("p"), "fictional task").unwrap();
    fs::write(
        f.root.path().join("jobs.json"),
        serde_json::to_vec(
            &json!({"jobs":[{"id":"one","route":"fake","cwd":".","prompt_file":"p"}]}),
        )
        .unwrap(),
    )
    .unwrap();
    for timeout in ["0", "-1", "NaN", "inf"] {
        for command in ["run", "batch"] {
            let mut cmd = f.cmd();
            if command == "run" {
                cmd.args(["run", "fake", "task", "--out", "answer.md"]);
            } else {
                cmd.args(["batch", "--jobs", "jobs.json", "--outdir", "out"]);
            }
            assert!(
                !cmd.args(["--timeout", timeout])
                    .output()
                    .unwrap()
                    .status
                    .success(),
                "{command} {timeout}"
            );
            assert!(!f.root.path().join("calls").exists(), "{command} {timeout}");
        }
    }
    assert!(
        !f.cmd()
            .args([
                "batch",
                "--jobs",
                "jobs.json",
                "--outdir",
                "out",
                "--timeout",
                "1e300"
            ])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!f.root.path().join("calls").exists());
}

#[test]
fn batch_sigterm_cancels_pool_queue_and_preserves_previous_reports() {
    let f = Fixture::new(&[(
        "fake",
        "echo call >> calls\necho $$ > cli-pid\ntouch started\nsleep 30\nprintf replacement\n",
        60,
    )]);
    fs::write(f.root.path().join("p"), "fictional task").unwrap();
    fs::create_dir(f.root.path().join("out")).unwrap();
    for id in ["j0", "j1"] {
        fs::write(
            f.root.path().join(format!("out/{id}.md")),
            format!("previous complete {id}\n"),
        )
        .unwrap();
    }
    fs::write(f.root.path().join("jobs.json"), serde_json::to_vec(&json!({
        "jobs":(0..6).map(|n|json!({"id":format!("j{n}"),"route":"fake","cwd":".","prompt_file":"p"})).collect::<Vec<_>>(),
        "pool_limits":{"fake":1}
    })).unwrap()).unwrap();
    let mut child = f
        .cmd()
        .args(["batch", "--jobs", "jobs.json", "--outdir", "out"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for(&f.root.path().join("started"));
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(8) {
            let cli_pid: i32 = fs::read_to_string(f.root.path().join("cli-pid"))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            unsafe {
                libc::kill(-cli_pid, libc::SIGKILL);
            }
            child.kill().unwrap();
            panic!("cancelled batch continued queued work");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["status"], "cancelled");
    assert_eq!(summary["results"].as_array().unwrap().len(), 6);
    assert!(
        summary["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|result| result["status"] == "cancelled")
    );
    assert_eq!(
        fs::read_to_string(f.root.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    for n in 0..6 {
        let status: Value = serde_json::from_slice(
            &fs::read(f.root.path().join(format!("out/j{n}.md.status.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(status["status"], "cancelled");
    }
    for id in ["j0", "j1"] {
        assert_eq!(
            fs::read_to_string(f.root.path().join(format!("out/{id}.md"))).unwrap(),
            format!("previous complete {id}\n")
        );
    }
}

#[test]
fn capacity_retries_then_uses_explicit_same_pool_fallback() {
    let f = Fixture::new(&[
        (
            "busy",
            "echo call >> calls\necho 'server is busy' >&2\nexit 1\n",
            10,
        ),
        ("backup", "printf complete\n", 10),
    ]);
    let config = f.root.path().join("config/routes.json");
    let mut routes: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    routes[1]["pool"] = json!("busy");
    fs::write(config, serde_json::to_vec(&routes).unwrap()).unwrap();
    let output = f
        .cmd()
        .args([
            "run",
            "busy",
            "task",
            "--fallback",
            "backup",
            "--out",
            "answer.md",
            "--rate-limit-retries",
            "1",
            "--retry-delay-ms",
            "1",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(f.root.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert_eq!(
        f.status()["route_attempts"][0]["attempts"][0]["status"],
        "capacity"
    );
    assert!(!f.root.path().join("state/review/state.json").exists());
}

#[test]
fn auth_cooldown_requires_consecutive_failures_and_reset_clears_history() {
    let f = Fixture::new(&[("auth", "echo '401 Unauthorized' >&2\nexit 1\n", 10)]);
    let invoke = || {
        let out = f
            .cmd()
            .args([
                "run",
                "auth",
                "task",
                "--out",
                "answer.md",
                "--ignore-cooldown",
            ])
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert_eq!(f.status()["status"], "auth");
        let state: Value = serde_json::from_slice(
            &fs::read(f.root.path().join("state/review/state.json")).unwrap(),
        )
        .unwrap();
        state["pools"]["auth"]["until"].as_f64().unwrap()
            - std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs_f64()
    };
    assert!((590.0..=600.0).contains(&invoke()));
    assert!((14390.0..=14400.0).contains(&invoke()));
    assert!(
        f.cmd()
            .args(["reset", "auth"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!((590.0..=600.0).contains(&invoke()));
    fs::write(f.root.path().join("auth.sh"), "printf complete\n").unwrap();
    assert!(
        f.cmd()
            .args([
                "run",
                "auth",
                "task",
                "--out",
                "answer.md",
                "--ignore-cooldown"
            ])
            .output()
            .unwrap()
            .status
            .success()
    );
    fs::write(
        f.root.path().join("auth.sh"),
        "echo '401 Unauthorized' >&2\nexit 1\n",
    )
    .unwrap();
    assert!((590.0..=600.0).contains(&invoke()));
}

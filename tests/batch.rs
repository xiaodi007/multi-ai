use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
fn run(root: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_multi-ai"))
        .env_clear()
        .env("HOME", root)
        .env("PATH", "/usr/bin:/bin")
        .env("MULTI_AI_CONFIG_DIR", root.join("config"))
        .env("MULTI_AI_STATE_DIR", root.join("state"))
        .current_dir(root)
        .args([
            "batch",
            "--jobs",
            "jobs.json",
            "--outdir",
            "out",
            "--source-id",
            "fixture",
        ])
        .args(extra)
        .output()
        .unwrap()
}
#[test]
fn twenty_jobs_pool_limit_failure_isolation_and_resume() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    fs::create_dir(root.join("config")).unwrap();
    let script = root.join("fake.sh");
    fs::write(
        &script,
        r#"cd "$2"
while ! mkdir lock 2>/dev/null; do sleep 0.01; done
n=$(cat active 2>/dev/null || echo 0); n=$((n+1)); echo "$n" > active
m=$(cat peak 2>/dev/null || echo 0); if [ "$n" -gt "$m" ]; then echo "$n" > peak; fi
echo "$1" >> calls
rmdir lock
sleep 0.05
while ! mkdir lock 2>/dev/null; do sleep 0.01; done
n=$(cat active); echo "$((n-1))" > active
rmdir lock
if [ "$1" = job-7 ] && [ ! -f failed ]; then touch failed; echo 'fixture failure' >&2; exit 1; fi
printf 'answer %s' "$1"
"#,
    )
    .unwrap();
    fs::write(root.join("config/routes.json"),serde_json::to_vec(&json!([{"id":"fake","legacy_id":"fake","pool":"test","family":"test","label":"fake","agentic":true,"cost":"sub","rank":1,"argv":["/bin/sh",script,"{prompt}",root],"extract":"stdout","default_timeout":10}])).unwrap()).unwrap();
    let jobs = (0..20)
        .map(|n| {
            fs::write(root.join(format!("p{n}")), format!("job-{n}")).unwrap();
            json!({"id":format!("j{n}"),"prompt_file":format!("p{n}"),"cwd":".","route":"fake"})
        })
        .collect::<Vec<_>>();
    fs::write(
        root.join("jobs.json"),
        serde_json::to_vec(&json!({"jobs":jobs,"pool_limits":{"test":2}})).unwrap(),
    )
    .unwrap();
    let first = run(root, &[]);
    assert!(
        !first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(
        first["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["status"] == "ok")
            .count(),
        19
    );
    assert_eq!(fs::read_to_string(root.join("peak")).unwrap().trim(), "2");
    assert_eq!(
        fs::read_to_string(root.join("calls"))
            .unwrap()
            .lines()
            .count(),
        20
    );
    let second = run(root, &["--resume"]);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join("calls"))
            .unwrap()
            .lines()
            .count(),
        21
    );
    let second: Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(
        second["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["cached"] == true)
            .count(),
        19
    );
}

#[test]
fn quota_switches_pool_and_queued_jobs_skip_exhausted_pool() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    fs::create_dir(root.join("config")).unwrap();
    fs::write(root.join("quota.sh"), "echo call >> quota-calls\necho \"403 You've reached your 5-hour usage limit\" >&2\nexit 1\n").unwrap();
    fs::write(
        root.join("ok.sh"),
        "echo call >> backup-calls\nprintf 'complete %s' \"$1\"\n",
    )
    .unwrap();
    let route = |id: &str, pool: &str, script: &str| json!({"id":id,"legacy_id":id,"pool":pool,"family":"test","label":id,"agentic":true,"cost":"sub","rank":1,"argv":["/bin/sh",root.join(script),"{prompt}"],"extract":"stdout","default_timeout":10});
    fs::write(
        root.join("config/routes.json"),
        serde_json::to_vec(&json!([
            route("quota", "window", "quota.sh"),
            route("backup", "other", "ok.sh")
        ]))
        .unwrap(),
    )
    .unwrap();
    fs::write(root.join("p"), "fixture").unwrap();
    fs::write(root.join("jobs.json"),serde_json::to_vec(&json!({"jobs":(0..4).map(|n|json!({"id":format!("j{n}"),"prompt_file":"p","cwd":".","route":"quota","fallback":["backup"]})).collect::<Vec<_>>(),"pool_limits":{"window":1,"other":2}})).unwrap()).unwrap();
    let first = run(root, &[]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let summary = String::from_utf8_lossy(&first.stderr);
    assert!(
        summary
            .lines()
            .any(|l| l.contains("回落：quota:quota(") && l.ends_with("→ backup")),
        "{summary}"
    );
    assert_eq!(
        fs::read_to_string(root.join("quota-calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert_eq!(
        fs::read_to_string(root.join("backup-calls"))
            .unwrap()
            .lines()
            .count(),
        4
    );
    let second = run(root, &["--resume"]);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join("backup-calls"))
            .unwrap()
            .lines()
            .count(),
        4
    );
    let v: Value = serde_json::from_slice(&second.stdout).unwrap();
    assert!(
        v["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["cached"] == true)
    );
}

#[test]
fn rate_limit_retries_without_pool_cooldown_and_remains_resumable() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    fs::create_dir(root.join("config")).unwrap();
    fs::write(
        root.join("rate.sh"),
        r#"n=$(cat calls 2>/dev/null || echo 0); n=$((n+1)); echo "$n" > calls
if [ "$n" -le 2 ]; then echo '429 code 1302 too many requests' >&2; exit 1; fi
printf complete
"#,
    )
    .unwrap();
    fs::write(root.join("config/routes.json"),serde_json::to_vec(&json!([{"id":"rate","legacy_id":"rate","pool":"zhipu","family":"test","label":"rate","agentic":true,"cost":"sub","rank":1,"argv":["/bin/sh",root.join("rate.sh")],"extract":"stdout","default_timeout":10}])).unwrap()).unwrap();
    fs::write(root.join("p"), "fixture").unwrap();
    fs::write(
        root.join("jobs.json"),
        serde_json::to_vec(
            &json!({"jobs":[{"id":"j","prompt_file":"p","cwd":".","route":"rate"}]}),
        )
        .unwrap(),
    )
    .unwrap();
    let output = run(
        root,
        &["--rate-limit-retries", "2", "--retry-delay-ms", "10"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(root.join("calls")).unwrap().trim(), "3");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result["results"][0]["route_attempts"][0]["attempts"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let output = run(
        root,
        &[
            "--resume",
            "--rate-limit-retries",
            "2",
            "--retry-delay-ms",
            "10",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(root.join("calls")).unwrap().trim(), "3");
    assert!(!root.join("state/review/state.json").exists());
}

#[test]
fn http_quota_and_rate_limit_are_distinct() {
    use multi_ai::api::http_error_code;
    assert_eq!(
        http_error_code(403, "You've reached your 5-hour usage limit"),
        "quota"
    );
    assert_eq!(http_error_code(429, "insufficient balance"), "quota");
    assert_eq!(
        http_error_code(429, r#"{"error":{"code":"1302"}}"#),
        "rate_limit"
    );
    assert_eq!(http_error_code(403, "forbidden"), "auth");
}

#[test]
fn preflight_skips_a_route_whose_model_is_rejected_before_any_job_runs() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    fs::create_dir(root.join("config")).unwrap();
    fs::write(
        root.join("dead.sh"),
        format!("echo call >> '{}/dead-calls'\necho '[claude-code:unrecognized_model] {{\"model\":\"gone\"}}' >&2\nexit 1\n", root.display()),
    )
    .unwrap();
    fs::write(
        root.join("ok.sh"),
        format!(
            "echo call >> '{}/ok-calls'\nprintf 'complete %s' \"$1\"\n",
            root.display()
        ),
    )
    .unwrap();
    let route = |id: &str, script: &str, rank: u32| json!({"id":id,"legacy_id":id,"pool":id,"family":"test","label":id,"agentic":true,"cost":"sub","rank":rank,"argv":["/bin/sh",root.join(script),"{prompt}"],"extract":"stdout","default_timeout":10});
    fs::write(
        root.join("config/routes.json"),
        serde_json::to_vec(&json!([
            route("dead", "dead.sh", 1),
            route("ok", "ok.sh", 2)
        ]))
        .unwrap(),
    )
    .unwrap();
    let jobs = (0..3)
        .map(|n| {
            fs::write(root.join(format!("p{n}")), format!("job-{n}")).unwrap();
            json!({"id":format!("j{n}"),"prompt_file":format!("p{n}"),"cwd":".","route":"dead","fallback":["ok"]})
        })
        .collect::<Vec<_>>();
    fs::write(
        root.join("jobs.json"),
        serde_json::to_vec(&json!({"jobs":jobs})).unwrap(),
    )
    .unwrap();
    let out = run(root, &["--preflight"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let calls = |name: &str| {
        fs::read_to_string(root.join(name))
            .unwrap_or_default()
            .lines()
            .count()
    };
    // One preflight call per route; jobs never invoke the dead route.
    assert_eq!(calls("dead-calls"), 1);
    assert_eq!(calls("ok-calls"), 4);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    for row in report["results"].as_array().unwrap() {
        assert_eq!(row["status"], "ok", "{row}");
        assert_eq!(row["route"], "ok", "{row}");
        assert_eq!(row["route_attempts"][0]["status"], "model", "{row}");
        assert_eq!(row["route_attempts"][0]["preflight"], true, "{row}");
    }
    assert!(String::from_utf8_lossy(&out.stderr).contains("preflight: skip dead"));
    // Without a fallback, a job on a dead route reports the preflight failure, not a call.
    let jobs = json!({"jobs":[{"id":"solo","prompt_file":"p0","cwd":".","route":"dead"}]});
    fs::write(root.join("jobs.json"), serde_json::to_vec(&jobs).unwrap()).unwrap();
    fs::remove_dir_all(root.join("out")).unwrap();
    let out = run(root, &["--preflight"]);
    assert!(!out.status.success());
    assert_eq!(calls("dead-calls"), 2);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["results"][0]["status"], "model");
    assert_eq!(report["results"][0]["model_calls"], 0);
}

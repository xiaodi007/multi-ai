use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
fn write(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}
fn command(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_multi-ai"))
        .env_clear()
        .env("HOME", root.join("home"))
        .env("PATH", "/usr/bin:/bin")
        .env("MULTI_AI_CONFIG_DIR", root.join("config"))
        .env("MULTI_AI_STATE_DIR", root.join("state"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}
fn value(out: Output) -> Value {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
fn setup(root: &Path) {
    fs::write(
        root.join("fake.sh"),
        "printf '| /fixture/src/a.rs:13 | 真问题 |\\n| src/b.rs:20 | 误报 |'\n",
    )
    .unwrap();
    let route = |id: &str, pool: &str| json!({"id":id,"legacy_id":id,"pool":pool,"family":"test","label":id,"agentic":true,"cost":"sub","rank":1,"argv":["/bin/sh",root.join("fake.sh")],"extract":"stdout","default_timeout":10});
    write(
        &root.join("config/routes.json"),
        &json!([
            route("good", "p1"),
            route("explore", "p2"),
            route("cooled", "p3")
        ]),
    );
    fs::write(
        root.join("prompt"),
        "fictional evidence; classify each location",
    )
    .unwrap();
    write(
        &root.join("canaries.json"),
        &json!([{"match":"src/a.rs:10","expect":"真问题"},{"match":"src/b.rs:20","expect":"真问题"}]),
    );
}
#[test]
fn run_canaries_bind_feedback_and_validate_before_invocation() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    setup(root);
    let result = value(command(
        root,
        &[
            "run",
            "good",
            "--task-type",
            "triage",
            "--prompt-file",
            "prompt",
            "--canary-file",
            "canaries.json",
            "--out",
            "one.md",
            "--source-id",
            "v1",
            "--json",
        ],
    ));
    assert_eq!(result["canary_hits"], 1);
    assert_eq!(result["canary_total"], 2);
    let status: Value =
        serde_json::from_slice(&fs::read(root.join("one.md.status.json")).unwrap()).unwrap();
    assert_eq!(status["canary_hits"], 1);
    assert_eq!(status["task_type"], "triage");
    let dir = root
        .join("state/runs")
        .join(result["run_id"].as_str().unwrap());
    let feedback: Value = serde_json::from_str(
        fs::read_to_string(dir.join("feedback.jsonl"))
            .unwrap()
            .trim(),
    )
    .unwrap();
    assert_eq!(feedback["source"], "canary");
    assert!(feedback["counts"]["accepted"].is_null());
    let stats = value(command(root, &["stats", "--json"]));
    assert_eq!(stats["task_types"][0]["task_type"], "triage");
    let score = value(command(
        root,
        &["scorecard", "--json", "--task-type", "triage"],
    ));
    assert_eq!(score["cells"][0]["canary_hits"], 1);
    assert_eq!(score["cells"][0]["assessment"], "样本不足");
    let cached = value(command(
        root,
        &[
            "run",
            "good",
            "--task-type",
            "triage",
            "--prompt-file",
            "prompt",
            "--canary-file",
            "canaries.json",
            "--out",
            "one.md",
            "--source-id",
            "v1",
            "--resume",
            "--json",
        ],
    ));
    assert_eq!(cached["cached"], true);
    let score = value(command(root, &["scorecard", "--json"]));
    assert_eq!(score["cells"][0]["sample_count"], 1);
    assert_eq!(score["cells"][0]["canary_total"], 2);
    value(command(
        root,
        &[
            "feedback",
            "--run",
            result["run_id"].as_str().unwrap(),
            "--report",
            result["attempt_id"].as_str().unwrap(),
            "--accepted",
            "5",
            "--false-positive",
            "2",
            "--evidence",
            "host verified original",
        ],
    ));
    value(command(
        root,
        &[
            "feedback",
            "--run",
            cached["run_id"].as_str().unwrap(),
            "--report",
            cached["attempt_id"].as_str().unwrap(),
            "--accepted",
            "3",
            "--evidence",
            "host corrected cached report",
        ],
    ));
    let revised = value(command(root, &["scorecard", "--json"]));
    assert_eq!(revised["cells"][0]["sample_count"], 1);
    assert_eq!(revised["cells"][0]["accepted"], 3);
    assert_eq!(revised["cells"][0]["false_positive"], 2);
    assert_eq!(revised["cells"][0]["precision"], 0.6);
    assert_eq!(revised["cells"][0]["canary_total"], 2);
    fs::write(
        root.join("bad.json"),
        r#"[{"match":"[","expect":"真问题"}]"#,
    )
    .unwrap();
    let before = fs::read_dir(root.join("state/runs")).unwrap().count();
    assert!(
        !command(
            root,
            &[
                "run",
                "good",
                "--prompt",
                "fixture",
                "--canary-file",
                "bad.json"
            ]
        )
        .status
        .success()
    );
    assert_eq!(
        fs::read_dir(root.join("state/runs")).unwrap().count(),
        before
    );
}
#[test]
fn batch_feedback_preflight_corrections_recommend_and_auto() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    setup(root);
    write(
        &root.join("jobs.json"),
        &json!({"jobs":(0..5).map(|i|json!({"id":format!("j{i}"),"prompt_file":"prompt","cwd":".","route":"good","task_type":"triage","canaries":[{"match":"src/a.rs:10","expect":"真问题"}]})).collect::<Vec<_>>()}),
    );
    let out = command(
        root,
        &[
            "batch",
            "--jobs",
            "jobs.json",
            "--outdir",
            "out",
            "--source-id",
            "fixture",
        ],
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("run_id\tattempt_id\tcanary"));
    let result = value(out);
    for job in result["results"].as_array().unwrap() {
        assert_eq!(job["canary_hits"], 1);
        assert!(job["attempt_id"].is_string());
    }
    let header = "id\taccepted\tfalse_positive\tuncertain\tevidence\tscreen_min\n";
    fs::write(
        root.join("verdicts.tsv"),
        format!("{header}j0\t1\t0\t\thost inspected fixture\t2\nbad\t1\t0\t\tunknown task\t1\n"),
    )
    .unwrap();
    assert!(
        !command(
            root,
            &["feedback", "--batch", "out", "--from", "verdicts.tsv"]
        )
        .status
        .success()
    );
    let event_path = root
        .join("state/runs")
        .join(result["run_id"].as_str().unwrap())
        .join("feedback.jsonl");
    assert_eq!(fs::read_to_string(&event_path).unwrap().lines().count(), 5);
    fs::write(
        root.join("verdicts.tsv"),
        format!(
            "{header}{}",
            (0..5)
                .map(|i| format!("j{i}\t2\t0\t\thost inspected fixture\t4\n"))
                .collect::<String>()
        ),
    )
    .unwrap();
    assert_eq!(
        value(command(
            root,
            &["feedback", "--batch", "out", "--from", "verdicts.tsv"]
        ))["written"],
        5
    );
    let score = value(command(
        root,
        &["scorecard", "--task-type", "triage", "--json"],
    ));
    let row = &score["cells"][0];
    assert_eq!(row["sample_count"], 5);
    assert_eq!(row["precision"], 1.0);
    assert_eq!(row["canary_recall"], 1.0);
    assert_eq!(row["host_screen_minutes_per_accepted"], 2.0);
    assert_eq!(row["assessment"], "可比较");
    write(
        &root.join("state/review/state.json"),
        &json!({"pools":{"p3":{"until":4102444800.0,"kind":"quota","detail":"fixture"}}}),
    );
    let recommended = value(command(
        root,
        &[
            "recommend",
            "--task-type",
            "triage",
            "--jobs",
            "10",
            "--json",
        ],
    ));
    assert_eq!(recommended["primary"], "good");
    assert_eq!(recommended["routes"].as_array().unwrap().len(), 2);
    assert_eq!(recommended["routes"][0]["jobs"], 8);
    assert_eq!(recommended["routes"][1]["jobs"], 2);
    write(
        &root.join("auto.json"),
        &json!({"jobs":(0..10).map(|i|json!({"id":format!("a{i}"),"prompt_file":"prompt","cwd":".","route":"auto","task_type":"triage"})).collect::<Vec<_>>()}),
    );
    let auto = value(command(
        root,
        &[
            "batch",
            "--jobs",
            "auto.json",
            "--outdir",
            "auto-out",
            "--source-id",
            "v1",
        ],
    ));
    assert_eq!(
        auto["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["route"] == "good")
            .count(),
        8
    );
    assert_eq!(
        auto["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["route"] == "explore")
            .count(),
        2
    );
    // Changing the CLI wrapper invalidates old quality evidence for route recommendations.
    fs::write(root.join("fake.sh"), "printf 'new version'\n").unwrap();
    let changed = value(command(
        root,
        &[
            "recommend",
            "--task-type",
            "triage",
            "--jobs",
            "10",
            "--json",
        ],
    ));
    assert!(changed["primary"].is_null());
    assert_eq!(changed["routes"][0]["jobs"], 5);
    // Correction events are retained, but not summed twice.
    fs::write(root.join("correction.tsv"),"id\taccepted\tfalse_positive\tuncertain\tevidence\nj0\t0\t2\t\thost corrected two claims\n").unwrap();
    value(command(
        root,
        &["feedback", "--batch", "out", "--from", "correction.tsv"],
    ));
    let score = value(command(
        root,
        &["scorecard", "--task-type", "triage", "--json"],
    ));
    assert_eq!(
        score["cells"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["route"] == "good")
            .unwrap()["accepted"],
        8
    );
}

fn historical(root: &Path, run: &str, records: &[Value]) -> PathBuf {
    let directory = root.join("state/runs").join(run);
    let progress = root.join("progress").join(run);
    let mut reports = Vec::new();
    let mut events = Vec::new();
    for v in records {
        let id = v["attempt_id"].as_str().unwrap();
        write(&progress.join(id).join("metadata.json"), v);
        if v["status"] == "complete" {
            let path = progress.join(id).join("report.md");
            fs::write(&path, "verified fixture").unwrap();
            let hash = multi_ai::runtime::sha256(b"verified fixture");
            reports.push(json!({"attempt_id":id,"path":path,"report_sha256":hash,"result":v}));
            events.push(json!({"event_id":id,"run_id":run,"attempt_id":id,"report_sha256":hash,"source":"agent","counts":{"accepted":2,"false-positive":1},"timing":{"screen-min":4},"created_at":"2026-10-18T10:00:00Z"}));
            events.push(json!({"event_id":format!("c{id}"),"run_id":run,"attempt_id":id,"report_sha256":hash,"source":"canary","canary_hits":1,"canary_total":2,"created_at":"2026-10-18T10:00:01Z"}));
        }
    }
    write(
        &directory.join("manifest.json"),
        &json!({"run_id":run,"started_at":"2026-10-18T00:00:00Z","command":"batch","outputs":{"progress":progress},"reports":reports}),
    );
    fs::write(
        directory.join("feedback.jsonl"),
        events.iter().map(|e| format!("{e}\n")).collect::<String>(),
    )
    .unwrap();
    directory
}
#[test]
fn scorecard_percentiles_failure_cost_coverage_since_and_corruption() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    let records=(0..8).map(|i|json!({"attempt_id":format!("a{i}"),"route":"good","task_type":"review","status":match i {5=>"quota",6=>"rate_limit",7=>"timeout",_=>"complete"},"seconds":(i+1)*10})).collect::<Vec<_>>();
    let dir = historical(root, "fixture", &records);
    let report = value(command(
        root,
        &["scorecard", "--json", "--since", "2026-10-18"],
    ));
    let r = &report["cells"][0];
    assert_eq!(r["sample_count"], 8);
    assert_eq!(r["median_seconds"], 45.0);
    assert_eq!(r["p90_seconds"], 80.0);
    assert_eq!(r["precision"], 2.0 / 3.0);
    assert_eq!(r["canary_recall"], 0.5);
    assert_eq!(r["miss_rate"], 0.5);
    assert_eq!(r["failure_rate"], 3.0 / 8.0);
    assert_eq!(r["failure_rates"]["quota"], 1.0 / 8.0);
    assert_eq!(r["external_seconds_per_accepted"], 36.0);
    assert_eq!(r["host_screen_minutes_per_accepted"], 2.0);
    let filtered = value(command(
        root,
        &["scorecard", "--json", "--since", "2026-10-19"],
    ));
    assert!(filtered["cells"].as_array().unwrap().is_empty());
    assert!(
        !command(root, &["scorecard", "--since", "nonsense"])
            .status
            .success()
    );
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    manifest["reports"][0]["report_sha256"] = json!("0".repeat(64));
    write(&dir.join("manifest.json"), &manifest);
    let corrupt = value(command(root, &["scorecard", "--json"]));
    assert_eq!(corrupt["invalid_feedback"], 2);
    assert_eq!(corrupt["cells"][0]["accepted"], 8);
}
#[test]
fn unknown_counts_do_not_become_zero_or_a_quality_conclusion() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    let records=(0..5).map(|i|json!({"attempt_id":format!("a{i}"),"route":"good","status":"complete","seconds":10})).collect::<Vec<_>>();
    let dir = historical(root, "fixture", &records);
    let text = fs::read_to_string(dir.join("feedback.jsonl")).unwrap();
    let events = text
        .lines()
        .map(|line| {
            let mut e: Value = serde_json::from_str(line).unwrap();
            e["counts"]["false-positive"] = Value::Null;
            e.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(dir.join("feedback.jsonl"), events).unwrap();
    let score = value(command(root, &["scorecard", "--json"]));
    let r = &score["cells"][0];
    assert!(r["precision"].is_null());
    assert_eq!(r["assessment"], "样本不足");
    assert_eq!(r["verified_samples"], 0);
    assert!(r["task_type"].is_null());
}
#[test]
fn routes_broken_pipe_is_successful() {
    use std::process::Stdio;
    let t = tempfile::tempdir().unwrap();
    setup(t.path());
    let mut child = Command::new(env!("CARGO_BIN_EXE_multi-ai"))
        .env_clear()
        .env("HOME", t.path())
        .env("MULTI_AI_CONFIG_DIR", t.path().join("config"))
        .current_dir(t.path())
        .args(["routes", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!String::from_utf8_lossy(&out.stderr).contains("panic"));
}

#[test]
fn auto_uses_each_job_cwd_and_single_job_exploration_is_not_starved() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    setup(root);
    fs::create_dir(root.join("work")).unwrap();
    fs::write(
        root.join("work/fake.sh"),
        "# distinct worker version\nprintf '| src/a.rs:10 | 真问题 |'\n",
    )
    .unwrap();
    let mut routes: Value =
        serde_json::from_slice(&fs::read(root.join("config/routes.json")).unwrap()).unwrap();
    for route in routes.as_array_mut().unwrap() {
        route["argv"] = json!(["/bin/sh", "fake.sh"]);
    }
    routes.as_array_mut().unwrap().push(json!({"id":"missing","legacy_id":"missing","pool":"missing","family":"test","label":"missing","agentic":true,"cost":"sub","rank":1,"argv":["/nonexistent/fixture-cli"],"extract":"stdout","default_timeout":10}));
    write(&root.join("config/routes.json"), &routes);
    write(
        &root.join("jobs.json"),
        &json!({"jobs":(0..5).map(|i|json!({"id":format!("j{i}"),"prompt_file":"prompt","cwd":"work","route":"good","task_type":"triage","canaries":[{"match":"src/a.rs:10","expect":"真问题"}]})).collect::<Vec<_>>()}),
    );
    value(command(
        root,
        &[
            "batch",
            "--jobs",
            "jobs.json",
            "--outdir",
            "out",
            "--source-id",
            "v1",
        ],
    ));
    fs::write(
        root.join("v.tsv"),
        format!(
            "id\taccepted\tfalse_positive\tuncertain\tevidence\n{}",
            (0..5)
                .map(|i| format!("j{i}\t1\t0\t\tverified fictional fixture\n"))
                .collect::<String>()
        ),
    )
    .unwrap();
    value(command(
        root,
        &["feedback", "--batch", "out", "--from", "v.tsv"],
    ));
    let root_scope = value(command(
        root,
        &["recommend", "--task-type", "triage", "--json"],
    ));
    assert!(root_scope["primary"].is_null());
    let work_scope = value(command(
        root,
        &[
            "recommend",
            "--task-type",
            "triage",
            "--cwd",
            "work",
            "--only",
            "good",
            "explore",
            "--json",
        ],
    ));
    assert_eq!(work_scope["primary"], "good");
    write(
        &root.join("auto.json"),
        &json!({"jobs":(0..10).map(|i|json!({"id":format!("a{i}"),"prompt_file":"prompt","cwd":"work","route":"auto","task_type":"triage"})).collect::<Vec<_>>()}),
    );
    let auto = value(command(
        root,
        &[
            "batch",
            "--jobs",
            "auto.json",
            "--outdir",
            "auto-out",
            "--source-id",
            "v1",
            "--only",
            "good",
            "explore",
        ],
    ));
    assert_eq!(
        auto["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["route"] == "good")
            .count(),
        8
    );
    // No quality evidence in this task: repeated one-job batches explore the least sampled route.
    let first = value(command(
        root,
        &[
            "recommend",
            "--task-type",
            "extract",
            "--jobs",
            "1",
            "--json",
        ],
    ));
    assert!(
        !first["routes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["route"] == "missing")
    );
    let first_route = first["routes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["jobs"] == 1)
        .unwrap()["route"]
        .as_str()
        .unwrap();
    value(command(
        root,
        &[
            "run",
            first_route,
            "--prompt",
            "fixture",
            "--out",
            "single.md",
            "--task-type",
            "extract",
            "--source-id",
            "v1",
            "--json",
        ],
    ));
    let next = value(command(
        root,
        &[
            "recommend",
            "--task-type",
            "extract",
            "--jobs",
            "1",
            "--json",
        ],
    ));
    assert_ne!(
        next["routes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["jobs"] == 1)
            .unwrap()["route"],
        first_route
    );
}

#[test]
fn auto_resume_reuses_original_route_before_spending_exploration_quota() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    setup(root);
    write(
        &root.join("jobs.json"),
        &json!({"jobs":[{"id":"one","prompt_file":"prompt","cwd":".","route":"auto","task_type":"walkthrough"}]}),
    );
    let args = [
        "batch",
        "--jobs",
        "jobs.json",
        "--outdir",
        "out",
        "--source-id",
        "v1",
    ];
    let first = value(command(root, &args));
    let mut resume = args.to_vec();
    resume.push("--resume");
    write(
        &root.join("state/review/state.json"),
        &json!({"pools": (["p1","p2","p3"].into_iter().map(|p|(p.to_owned(),json!({"until":4102444800.0,"kind":"quota","detail":"fixture"}))).collect::<serde_json::Map<_,_>>())}),
    );
    let restored = value(command(root, &resume));
    assert_eq!(
        restored["results"][0]["route"],
        first["results"][0]["route"]
    );
    assert_eq!(restored["results"][0]["cached"], true);
    let scores = value(command(
        root,
        &["scorecard", "--task-type", "walkthrough", "--json"],
    ));
    assert_eq!(
        scores["cells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["sample_count"].as_u64().unwrap())
            .sum::<u64>(),
        1
    );
    fs::remove_file(root.join("state/review/state.json")).unwrap();
    // Damaging the cached body still forces a real call; auto does not trust the hint.
    let path = root.join("out/one.md.cache.json");
    let mut cache: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    cache["content"] = json!("corrupt body");
    write(&path, &cache);
    let rerun = value(command(root, &resume));
    assert_ne!(rerun["results"][0]["cached"], true);
}

#[test]
fn implement_recommendation_uses_merge_rate_then_rework_without_precision_or_canaries() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    setup(root);
    write(
        &root.join("jobs.json"),
        &json!({"jobs":(0..10).map(|i|json!({"id":format!("i{i}"),"route":if i<5 {"good"} else {"explore"},"task_type":"implement","prompt_file":"prompt","cwd":"."})).collect::<Vec<_>>()}),
    );
    value(command(
        root,
        &[
            "batch",
            "--jobs",
            "jobs.json",
            "--outdir",
            "out",
            "--source-id",
            "fixture",
        ],
    ));
    fs::write(root.join("v.tsv"), format!("id\taccepted\tfalse_positive\tuncertain\tevidence\toutcome\trework_items\trework_rounds\n{}", (0..10).map(|i|format!("i{i}\t\t\t\tverified fixture\t{}\t{}\t0\n", if i==9 {"discarded"} else {"merged"}, if i<5 {2} else {0})).collect::<String>())).unwrap();
    value(command(
        root,
        &["feedback", "--batch", "out", "--from", "v.tsv"],
    ));
    let report = value(command(
        root,
        &["scorecard", "--task-type", "implement", "--json"],
    ));
    assert!(
        report["cells"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c.get("precision").is_none() && c["assessment"] == "可比较")
    );
    let recommendation = || {
        value(command(
            root,
            &["recommend", "--task-type", "implement", "--json"],
        ))
    };
    assert_eq!(recommendation()["primary"], "good");
    // Equal merge rate: lower rework wins. Missing fields stay unknown, never zero.
    fs::write(root.join("correction.tsv"), "id\taccepted\tfalse_positive\tuncertain\tevidence\toutcome\ni9\t\t\t\tcorrected fixture verdict\tmerged\n").unwrap();
    value(command(
        root,
        &["feedback", "--batch", "out", "--from", "correction.tsv"],
    ));
    assert_eq!(recommendation()["primary"], "explore");
}

#[test]
fn scorecard_separates_recorded_models_and_counts_execution_failure_under_boundary() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    let records = vec![
        json!({"attempt_id":"a","route":"codex-cli","model":"fixture-one","task_type":"implement","status":"complete","seconds":1}),
        json!({"attempt_id":"b","route":"codex-cli","model":"fixture-two","task_type":"implement","status":"complete","seconds":3}),
        json!({"attempt_id":"c","route":"codex-cli","model":"fixture-two","task_type":"implement","status":"boundary_violation","execution_status":"quota","response":{"status":"quota","complete":false,"exitCode":1},"attributed_boundary_count":1,"seconds":5}),
    ];
    historical(root, "fixture", &records);
    let report = value(command(root, &["scorecard", "--json"]));
    let cells = report["cells"].as_array().unwrap();
    assert_eq!(cells.len(), 2);
    assert_eq!(cells[0]["model"], "fixture-one");
    assert_eq!(cells[1]["model"], "fixture-two");
    assert_eq!(cells[1]["sample_count"], 2);
    assert_eq!(cells[1]["failures"], json!({"quota":1}));
    assert_eq!(cells[1]["failure_rate"], 0.5);
    assert_eq!(cells[1]["attributed_boundary_count"], 1);
}

#[test]
fn scorecard_distinguishes_capacity_from_rate_limit() {
    let t = tempfile::tempdir().unwrap();
    historical(
        t.path(),
        "fixture",
        &[
            json!({"attempt_id":"busy","route":"good","task_type":"review","status":"capacity","seconds":1}),
            json!({"attempt_id":"rate","route":"good","task_type":"review","status":"rate_limit","seconds":1}),
        ],
    );
    let report = value(command(t.path(), &["scorecard", "--json"]));
    let cell = &report["cells"][0];
    assert_eq!(cell["failures"]["capacity"], 1);
    assert_eq!(cell["failure_rates"]["capacity"], 0.5);
    assert_eq!(cell["failure_rates"]["rate_limit"], 0.5);
}

#[test]
fn recommend_exposes_capacity_failure_rate_for_current_identity() {
    let t = tempfile::tempdir().unwrap();
    setup(t.path());
    fs::write(t.path().join("fake.sh"), "echo 'overloaded' >&2\nexit 1\n").unwrap();
    let out = command(
        t.path(),
        &[
            "run",
            "good",
            "task",
            "--task-type",
            "triage",
            "--out",
            "answer.md",
            "--rate-limit-retries",
            "0",
        ],
    );
    assert!(!out.status.success());
    let report = value(command(
        t.path(),
        &[
            "recommend",
            "--task-type",
            "triage",
            "--only",
            "good",
            "--json",
        ],
    ));
    assert_eq!(report["routes"][0]["failure_rates"]["capacity"], 1.0);
}

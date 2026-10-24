use serde_json::{Value, json};
use std::{fs, os::unix::fs::symlink, path::Path, process::Command};

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn run_record(state: &Path, id: &str, command: &str, records: &[Value]) -> Value {
    let progress = state.join("progress").join(id);
    fs::create_dir_all(&progress).unwrap();
    for (index, value) in records.iter().enumerate() {
        let name = value["attempt_id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("attempt-{index}"));
        write_json(&progress.join(name).join("metadata.json"), value);
    }
    let manifest = json!({"schemaVersion":1,"run_id":id,"command":command,"outputs":{"progress":progress},"reports":[]});
    write_json(
        &state.join("runs").join(id).join("manifest.json"),
        &manifest,
    );
    manifest
}

fn invoke(state: &Path, json_output: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_multi-ai"));
    command
        .env_clear()
        .env("HOME", state.join("unavailable-home"))
        .env("MULTI_AI_STATE_DIR", state.join("wrong-state"))
        .env("MULTI_AI_CONFIG_DIR", state.join("unavailable-config"))
        .args(["stats", "--state-dir"])
        .arg(state);
    if json_output {
        command.arg("--json");
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn stats(state: &Path) -> Value {
    serde_json::from_slice(&invoke(state, true).stdout).unwrap()
}

fn row<'a>(report: &'a Value, route: &str) -> &'a Value {
    report["routes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["route"] == route)
        .unwrap()
}

#[test]
fn summarizes_calls_success_percentiles_failures_and_utf8_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let records = [100, 200, 300, 400, 1000]
        .into_iter()
        .enumerate()
        .map(|(index, milliseconds)| {
            json!({"attempt_id":format!("a{index}"),"route":"codex-cli","provider":"irrelevant",
                "status":if index == 4 {"quota"} else {"complete"},"complete":index != 4,
                "totalMs":milliseconds,"inputBytes":10,"output":"中文"})
        })
        .collect::<Vec<_>>();
    run_record(temp.path(), "one", "run", &records);
    let report = stats(temp.path());
    let actual = row(&report, "codex-cli");
    assert_eq!(report["runs"], 1);
    assert_eq!(report["attempts"], 5);
    assert_eq!(actual["calls"], 5);
    assert_eq!(actual["successful"], 4);
    assert_eq!(actual["success_rate_pct"], 80.0);
    assert_eq!(actual["median_ms"], 300.0);
    assert_eq!(actual["p90_ms"], 1000.0);
    assert_eq!(actual["failures"]["quota"], 1);
    assert_eq!(actual["input_bytes"], 50);
    assert_eq!(actual["output_bytes"], 30);
    assert_eq!(actual["input_bytes_samples"], 5);
    assert_eq!(actual["output_bytes_samples"], 5);
}

#[test]
fn even_median_and_singleton_use_available_elapsed_metrics() {
    let temp = tempfile::tempdir().unwrap();
    run_record(
        temp.path(),
        "one",
        "run",
        &[
            json!({"attempt_id":"a","route":"even","status":"ok","timings":{"totalMs":100}}),
            json!({"attempt_id":"b","route":"even","status":"ok","seconds":0.3}),
            json!({"attempt_id":"c","route":"single","status":"ok","totalMs":0}),
        ],
    );
    let report = stats(temp.path());
    assert_eq!(row(&report, "even")["median_ms"], 200.0);
    assert_eq!(row(&report, "even")["p90_ms"], 300.0);
    assert_eq!(row(&report, "single")["median_ms"], 0.0);
    assert_eq!(row(&report, "single")["p90_ms"], 0.0);
}

#[test]
fn cached_skipped_and_running_are_outside_call_metrics() {
    let temp = tempfile::tempdir().unwrap();
    let records = ["complete", "cooldown", "missing_input", "cache_miss", "missing", "running"]
        .into_iter()
        .enumerate()
        .map(|(index, status)| json!({"attempt_id":format!("a{index}"),"route":"fake","status":status,"cached":index==0,"inputBytes":10,"outputBytes":20,"totalMs":100}))
        .collect::<Vec<_>>();
    run_record(temp.path(), "one", "batch", &records);
    let report = stats(temp.path());
    let actual = row(&report, "fake");
    assert_eq!(actual["attempts"], 6);
    assert_eq!(actual["calls"], 0);
    assert_eq!(actual["cached"], 1);
    assert_eq!(actual["skipped"], 4);
    assert_eq!(actual["in_progress"], 1);
    for key in [
        "success_rate_pct",
        "median_ms",
        "p90_ms",
        "input_bytes",
        "output_bytes",
    ] {
        assert!(actual[key].is_null(), "{key}");
    }
    assert_eq!(actual["timing_samples"], 0);
}

#[test]
fn metadata_overrides_fallbacks_without_double_counting() {
    let temp = tempfile::tempdir().unwrap();
    let mut manifest = run_record(
        temp.path(),
        "one",
        "run",
        &[
            json!({"attempt_id":"a","label":"codex-cli","status":"complete","complete":true,"totalMs":100,"inputBytes":10,"outputBytes":4}),
        ],
    );
    manifest["reports"] = json!([{"attempt_id":"a","model_calls":1,"result":{"route":"codex-cli","status":"ok","complete":true,"totalMs":9000,"content":"fallback"}}]);
    manifest["current_status"] = json!({"result":{"attempt_id":"a","status":"error","complete":false,"inputBytes":5},"results":[{"attemptId":"b","route":"other","status":"timeout","seconds":1.2}]});
    write_json(&temp.path().join("runs/one/manifest.json"), &manifest);
    let report = stats(temp.path());
    assert_eq!(report["attempts"], 2);
    let actual = row(&report, "codex-cli");
    assert_eq!(actual["calls"], 1);
    assert_eq!(actual["successful"], 1);
    assert_eq!(actual["median_ms"], 100.0);
    assert_eq!(actual["input_bytes"], 10);
    assert_eq!(actual["output_bytes"], 4);
    assert_eq!(row(&report, "other")["median_ms"], 1200.0);
}

#[test]
fn deleted_progress_recovers_direct_status_and_report_results() {
    let temp = tempfile::tempdir().unwrap();
    let mut manifest = run_record(temp.path(), "one", "text:run", &[]);
    fs::remove_dir(temp.path().join("progress/one")).unwrap();
    manifest["reports"] = json!([{"attempt_id":"a","cached":true,"model_calls":0,"path":"/never/read/report.md","result":{"provider":"zhipu","model":"old-model","complete":true,"content":"stored"}}]);
    manifest["current_status"] =
        json!({"attempt_id":"b","route":"grok-cli","status":"quota","seconds":2.0});
    write_json(&temp.path().join("runs/one/manifest.json"), &manifest);
    let report = stats(temp.path());
    assert_eq!(report["missing_progress_runs"], 1);
    assert_eq!(report["attempts"], 2);
    assert_eq!(row(&report, "text:zhipu/old-model")["cached"], 1);
    assert_eq!(row(&report, "grok-cli")["failures"]["quota"], 1);
}

#[test]
fn deleted_progress_recovers_nested_failover_and_retry_attempts_once() {
    let temp = tempfile::tempdir().unwrap();
    let mut manifest = run_record(temp.path(), "one", "batch", &[]);
    fs::remove_dir(temp.path().join("progress/one")).unwrap();
    let quota = json!({"attempt_id":"a","route":"primary","status":"quota","seconds":1});
    let limited = json!({"attempt_id":"b","route":"backup","status":"rate_limit","seconds":2});
    let success =
        json!({"attempt_id":"c","route":"backup","status":"ok","complete":true,"seconds":3});
    let mut backup = success.clone();
    backup["attempts"] = json!([limited, success]);
    let mut result = backup.clone();
    result["route_attempts"] = json!([quota, backup]);
    manifest["current_status"] = json!({"results":[{"result":result}]});
    write_json(&temp.path().join("runs/one/manifest.json"), &manifest);
    let report = stats(temp.path());
    assert_eq!(report["attempts"], 3);
    assert_eq!(row(&report, "primary")["calls"], 1);
    assert_eq!(row(&report, "primary")["failures"]["quota"], 1);
    assert_eq!(row(&report, "backup")["calls"], 2);
    assert_eq!(row(&report, "backup")["successful"], 1);
    assert_eq!(row(&report, "backup")["failures"]["rate_limit"], 1);
    assert_eq!(row(&report, "backup")["median_ms"], 2500.0);
}

#[test]
fn unknown_metrics_stay_null_and_text_aliases_are_historical() {
    let temp = tempfile::tempdir().unwrap();
    run_record(
        temp.path(),
        "one",
        "text:glm",
        &[
            json!({"attempt_id":"a","label":"glm","provider":"zhipu","status":"truncated","bytes":500,"stdoutBytes":999,"totalMs":-1,"inputBytes":-1}),
        ],
    );
    let report = stats(temp.path());
    let actual = row(&report, "text:glm");
    for key in ["median_ms", "p90_ms", "input_bytes", "output_bytes"] {
        assert!(actual[key].is_null(), "{key}");
    }
    assert_eq!(actual["input_bytes_samples"], 0);
    assert_eq!(actual["output_bytes_samples"], 0);
    assert_eq!(actual["failures"]["truncated"], 1);
}

#[test]
fn bad_records_and_metadata_symlinks_are_reported_without_aborting() {
    let temp = tempfile::tempdir().unwrap();
    run_record(
        temp.path(),
        "one",
        "run",
        &[json!({"attempt_id":"valid","route":"fake","status":"complete","complete":true})],
    );
    let malformed = temp.path().join("progress/one/broken/metadata.json");
    fs::create_dir_all(malformed.parent().unwrap()).unwrap();
    fs::write(malformed, b"{broken").unwrap();
    fs::create_dir_all(temp.path().join("progress/one/link")).unwrap();
    write_json(
        &temp.path().join("private.json"),
        &json!({"attempt_id":"private","route":"private","status":"complete"}),
    );
    symlink(
        temp.path().join("private.json"),
        temp.path().join("progress/one/link/metadata.json"),
    )
    .unwrap();
    fs::create_dir_all(temp.path().join("runs/broken")).unwrap();
    fs::write(temp.path().join("runs/broken/manifest.json"), "not json").unwrap();
    let report = stats(temp.path());
    assert_eq!(report["runs"], 1);
    assert_eq!(report["attempts"], 1);
    assert_eq!(report["invalid_records"], 3);
    assert_eq!(report["routes"].as_array().unwrap().len(), 1);
}

#[test]
fn table_and_json_never_emit_body_errors_or_require_configuration() {
    let temp = tempfile::tempdir().unwrap();
    run_record(
        temp.path(),
        "one",
        "run",
        &[
            json!({"attempt_id":"a","route":"fake","status":"rate_limit","error":"private-error-marker","output":"private-body-marker","inputBytes":7,"totalMs":100}),
        ],
    );
    let table = String::from_utf8(invoke(temp.path(), false).stdout).unwrap();
    assert!(table.contains("ROUTE"));
    assert!(table.contains("rate_limit=1"));
    assert!(table.contains("fake"));
    let serialized = serde_json::to_string(&stats(temp.path())).unwrap();
    for output in [&table, &serialized] {
        assert!(!output.contains("private-body-marker"));
        assert!(!output.contains("private-error-marker"));
    }
}

#[test]
fn default_state_and_empty_history_are_offline() {
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_multi-ai"))
        .env_clear()
        .env("HOME", temp.path().join("unavailable-home"))
        .env("MULTI_AI_STATE_DIR", temp.path().join("no-history"))
        .args(["stats", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["runs"], 0);
    assert_eq!(report["attempts"], 0);
    assert!(report["routes"].as_array().unwrap().is_empty());
}

#[test]
fn attempt_ids_are_only_deduplicated_within_their_run() {
    let temp = tempfile::tempdir().unwrap();
    for id in ["one", "two"] {
        run_record(
            temp.path(),
            id,
            "run",
            &[json!({"attempt_id":"same","route":"fake","status":"ok","outputBytes":0})],
        );
    }
    let report = stats(temp.path());
    assert_eq!(report["runs"], 2);
    assert_eq!(report["attempts"], 2);
    assert_eq!(row(&report, "fake")["calls"], 2);
    assert_eq!(row(&report, "fake")["output_bytes"], 0);
    assert_eq!(row(&report, "fake")["output_bytes_samples"], 2);
}

#[test]
fn unfinished_runs_whose_process_is_gone_count_as_lost() {
    use std::os::fd::AsRawFd;
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path();
    let attempt =
        |id: &str| json!({"attempt_id":id,"route":"fake","status":"running","model_calls":1});
    let unfinished = |id: &str, started: &str| {
        let mut manifest = run_record(state, id, "run", &[attempt(id)]);
        manifest["status"] = json!("running");
        manifest["started_at"] = json!(started);
        write_json(
            &state.join("runs").join(id).join("manifest.json"),
            &manifest,
        );
    };
    let now = chrono::Utc::now().to_rfc3339();
    // Lock held: the process is alive.
    unfinished("held", &now);
    let held = fs::File::create(state.join("runs/held/alive.lock")).unwrap();
    assert_eq!(
        unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    // Lock file left behind but free: the process died without finishing.
    unfinished("released", &now);
    fs::File::create(state.join("runs/released/alive.lock")).unwrap();
    // Older engines wrote no lock: lost only after a day.
    unfinished("legacy-recent", &now);
    unfinished("legacy-old", "2026-01-01T00:00:00+00:00");
    let report = stats(state);
    let fake = row(&report, "fake");
    assert_eq!(fake["in_progress"], 2, "{fake}");
    assert_eq!(fake["failures"]["lost"], 2, "{fake}");
    drop(held);
}

#[test]
fn subcommand_help_lists_the_options_validation_accepts() {
    let help = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_multi-ai"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert!(help(&["stats", "--help"]).contains("stats 可用参数：--state-dir --task-type"));
    let delegate = help(&["delegate", "--help"]);
    assert!(delegate.contains("delegate implement 可用参数："));
    assert!(delegate.contains("--preflight"));
    assert!(help(&["help", "batch"]).contains("--preflight-timeout"));
}

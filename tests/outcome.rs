use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn git(repo: &Path, args: &[&str], date: &str) -> String {
    let out = Command::new("/usr/bin/git")
        .args(args)
        .current_dir(repo)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
fn write(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}
fn fixture(root: &Path, kind: &str) -> String {
    let run = uuid::Uuid::new_v4().to_string();
    let repo = root.join(kind);
    fs::create_dir_all(&repo).unwrap();
    let old = "2000-01-01T00:00:00Z";
    let new = "2021-01-01T00:00:00Z";
    git(&repo, &["init", "-b", "main"], old);
    fs::write(repo.join("a.txt"), "base").unwrap();
    git(&repo, &["add", "."], old);
    git(&repo, &["commit", "-m", "base"], old);
    let base = git(&repo, &["rev-parse", "HEAD"], old);
    git(&repo, &["checkout", "-b", "implementation"], old);
    if kind != "discarded" && kind != "unchanged" {
        fs::write(repo.join("a.txt"), "implemented").unwrap();
        git(&repo, &["commit", "-am", "implement"], new);
    }
    git(&repo, &["checkout", "main"], new);
    if kind != "discarded" && kind != "unchanged" {
        git(&repo, &["merge", "--ff-only", "implementation"], new);
    }
    if kind == "extra" {
        git(&repo, &["checkout", "implementation"], new);
        fs::write(repo.join("a.txt"), "extra work").unwrap();
        git(&repo, &["commit", "-am", "later"], "2022-01-01T00:00:00Z");
    }
    if kind == "discarded" || kind == "merged-deleted" {
        git(&repo, &["branch", "-D", "implementation"], new);
    }
    let dir = root.join("state/runs").join(&run);
    let report = dir.join("report.md");
    fs::create_dir_all(&dir).unwrap();
    fs::write(&report, "fixture report").unwrap();
    let hash = multi_ai::runtime::sha256(b"fixture report");
    write(
        &dir.join("manifest.json"),
        &json!({"run_id":run,"command":"implement",
        "started_at":"2020-01-01T00:00:00Z", "reports":[{"run_id":run,"attempt_id":"attempt",
        "path":report,"report_sha256":hash,"result":{"route":"codex-cli","complete":true,
        "cwd":repo,"git_before":{"head":base,"branch":"implementation"},"git_after":{"head":base},"changed_files":["a.txt"]}}]}),
    );
    run
}
fn suggest(root: &Path, args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_multi-ai"))
        .env_clear()
        .env("HOME", root.join("home"))
        .env("PATH", "/usr/bin:/bin")
        .env("MULTI_AI_STATE_DIR", root.join("state"))
        .env("MULTI_AI_CONFIG_DIR", root.join("config"))
        .env("AI_LEDGER_FILE", "off")
        .current_dir(root)
        .args(["outcome", "suggest", "--into", "main", "--json"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
#[test]
fn git_suggestions_apply_only_certain_and_are_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let runs: Vec<_> = ["merged", "discarded", "extra"]
        .iter()
        .map(|k| fixture(root, k))
        .collect();
    for (run, expected) in runs.iter().zip(["merged", "discarded", "unknown"]) {
        let rows = suggest(root, &["--run", run, "--report", "attempt"]);
        assert_eq!(rows[0]["outcome"], expected, "{rows}");
        assert!(
            !root
                .join("state/runs")
                .join(run)
                .join("feedback.jsonl")
                .exists()
        );
    }
    let all = suggest(root, &["--all", "--apply"]);
    assert_eq!(
        all.as_array()
            .unwrap()
            .iter()
            .filter(|r| r["applied"] == true)
            .count(),
        2
    );
    let again = suggest(root, &["--all", "--apply"]);
    assert_eq!(again.as_array().unwrap().len(), 1);
    assert_eq!(again[0]["outcome"], "unknown");
    for run in &runs[..2] {
        let text =
            fs::read_to_string(root.join("state/runs").join(run).join("feedback.jsonl")).unwrap();
        assert_eq!(text.lines().count(), 1);
        let event: Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(event["source"], "agent");
        assert!(event["counts"]["accepted"].is_null());
        assert!(
            event["evidence"]
                .as_str()
                .unwrap()
                .contains("target_commit")
        );
    }
    assert!(
        !root
            .join("state/runs")
            .join(&runs[2])
            .join("feedback.jsonl")
            .exists()
    );
}
#[test]
fn uncommitted_or_deleted_merged_branch_missing_repo_or_corrupt_report_is_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // A live branch with nothing committed may still be under host review.
    let run = fixture(root, "unchanged");
    assert_eq!(
        suggest(root, &["--run", &run, "--report", "attempt"])[0]["outcome"],
        "unknown"
    );
    // Lanes delete merged branches; reaching the target must not read as a discard.
    let deleted = fixture(root, "merged-deleted");
    assert_eq!(
        suggest(root, &["--run", &deleted, "--report", "attempt"])[0]["outcome"],
        "unknown"
    );
    fs::remove_dir_all(root.join("unchanged")).unwrap();
    assert_eq!(
        suggest(root, &["--run", &run, "--report", "attempt", "--apply"])[0]["outcome"],
        "unknown"
    );
    let run = fixture(root, "merged");
    fs::write(
        root.join("state/runs").join(&run).join("report.md"),
        "tampered",
    )
    .unwrap();
    assert_eq!(
        suggest(root, &["--run", &run, "--report", "attempt", "--apply"])[0]["outcome"],
        "unknown"
    );
}

#[test]
fn old_branchless_status_invalid_ref_and_partial_records_stay_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let run = fixture(root, "merged");
    let path = root.join("state/runs").join(&run).join("manifest.json");
    let original: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let mut old = original.clone();
    old["reports"][0]["result"]["git_before"]["branch"] = Value::Null;
    write(&path, &old);
    assert_eq!(
        suggest(root, &["--run", &run, "--report", "attempt", "--apply"])[0]["outcome"],
        "unknown"
    );
    let mut partial = original.clone();
    partial["reports"][0]["result"]["complete"] = json!(false);
    write(&path, &partial);
    assert_eq!(
        suggest(root, &["--run", &run, "--report", "attempt", "--apply"])[0]["outcome"],
        "unknown"
    );
    write(&path, &original);
    assert_eq!(
        suggest(
            root,
            &[
                "--run",
                &run,
                "--report",
                "attempt",
                "--into",
                "missing-ref",
                "--apply"
            ]
        )[0]["outcome"],
        "unknown"
    );
    assert!(!path.parent().unwrap().join("feedback.jsonl").exists());
}

#[test]
fn failed_attempt_without_registered_report_is_included_as_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let run = fixture(root, "merged");
    let path = root.join("state/runs").join(&run).join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let mut status = manifest["reports"][0]["result"].clone();
    status["attempt_id"] = json!("attempt");
    status["complete"] = json!(false);
    manifest["current_status"] = status;
    manifest["reports"] = json!([]);
    write(&path, &manifest);
    let rows = suggest(root, &["--all", "--apply"]);
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["outcome"], "unknown");
    assert_eq!(
        suggest(root, &["--run", &run, "--report", "attempt"])[0]["outcome"],
        "unknown"
    );
    assert!(!path.parent().unwrap().join("feedback.jsonl").exists());
}

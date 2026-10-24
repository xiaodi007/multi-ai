use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
fn attempt(
    state: &Path,
    run: &str,
    cwd: &str,
    started: &str,
    model: Option<&str>,
) -> std::path::PathBuf {
    let progress = state.join("progress").join(run);
    let mut metadata = json!({"route":"codex-cli","cwd":cwd,"started_at":started,"status":"ok"});
    if let Some(model) = model {
        metadata["model"] = json!(model);
    }
    let file = progress.join("codex-cli-a").join("metadata.json");
    write(&file, &metadata.to_string());
    write(
        &state.join("runs").join(run).join("manifest.json"),
        &json!({"command":"implement","started_at":started,"outputs":{"progress":progress}})
            .to_string(),
    );
    file
}
fn session(root: &Path, name: &str, cwd: &str, at: &str, model: &str) {
    let meta = json!({"type":"session_meta","payload":{"timestamp":at,"cwd":cwd,"originator":"codex_exec"}});
    let turn = json!({"type":"turn_context","payload":{"model":model}});
    write(
        &root.join(format!("rollout-{name}.jsonl")),
        &format!("{meta}\n{turn}\n"),
    );
}

#[test]
fn backfills_only_unique_session_matches_and_keeps_a_backup() {
    let temp = tempfile::tempdir().unwrap();
    let (state, logs) = (temp.path().join("state"), temp.path().join("sessions"));
    let unique = attempt(&state, "unique", "/w/a", "2026-10-20T01:00:00+00:00", None);
    let ambiguous = attempt(
        &state,
        "ambiguous",
        "/w/b",
        "2026-10-20T02:00:00+00:00",
        None,
    );
    let known = attempt(
        &state,
        "known",
        "/w/c",
        "2026-10-20T03:00:00+00:00",
        Some("kept"),
    );
    session(&logs, "1", "/w/a", "2026-10-20T01:00:04Z", "gpt-x");
    session(&logs, "2", "/w/a", "2026-10-20T05:00:00Z", "other");
    session(&logs, "3", "/w/b", "2026-10-20T02:00:01Z", "gpt-x");
    session(&logs, "4", "/w/b", "2026-10-20T02:00:09Z", "gpt-y");
    session(&logs, "5", "/w/c", "2026-10-20T03:00:01Z", "gpt-x");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/backfill-codex-model.py");
    let run = |apply: bool| {
        let mut cmd = Command::new("/usr/bin/python3");
        cmd.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .arg(&script)
            .arg("--state-dir")
            .arg(&state)
            .arg("--codex-sessions")
            .arg(&logs);
        if apply {
            cmd.arg("--apply");
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let read = |p: &Path| serde_json::from_slice::<Value>(&fs::read(p).unwrap()).unwrap();
    let original = fs::read(&unique).unwrap();
    assert!(run(false).contains("\"backfilled\": 1, \"no_match\": 0, \"ambiguous\": 1"));
    assert_eq!(
        fs::read(&unique).unwrap(),
        original,
        "dry-run must not write"
    );
    run(true);
    let filled = read(&unique);
    assert_eq!(filled["model"], "gpt-x");
    assert_eq!(filled["model_source"], "codex-session-log");
    assert_eq!(
        fs::read(
            unique
                .parent()
                .unwrap()
                .join("metadata.before-model-backfill.json")
        )
        .unwrap(),
        original
    );
    assert!(read(&ambiguous)["model"].is_null());
    assert_eq!(read(&known)["model"], "kept");
}

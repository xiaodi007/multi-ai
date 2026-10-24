//! All commands are offline failures/help; hooks are local shell fixtures only.
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;

fn fixture() -> TempDir {
    tempfile::tempdir().unwrap()
}
fn command(root: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_multi-ai"));
    cmd.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.join("home"))
        .env("MULTI_AI_CONFIG_DIR", root.join("config"))
        .env("MULTI_AI_STATE_DIR", root.join("state"))
        .env("AI_LEDGER_FILE", root.join("events.jsonl"))
        .current_dir(root);
    cmd
}
fn events(root: &Path) -> Vec<Value> {
    fs::read_to_string(root.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn pair(root: &Path, output: &Output) -> Vec<Value> {
    let entries = events(root);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["event"], "start");
    assert_eq!(entries[1]["event"], "finish");
    assert_eq!(entries[0]["id"], entries[1]["id"]);
    assert_eq!(entries[1]["exit"], output.status.code().unwrap());
    for event in &entries {
        assert_eq!(event["v"], 1);
        chrono::DateTime::parse_from_rfc3339(event["at"].as_str().unwrap()).unwrap();
        assert!(event["head"].is_string());
        assert!(event["out"].is_string());
    }
    entries
}

#[test]
fn schema_inference_and_explicit_task_type() {
    for (args, inferred) in [
        (vec!["delegate", "implement"], json!("implement")),
        (vec!["delegate", "extract"], Value::Null),
        (vec!["batch"], Value::Null),
        (vec!["review"], json!("review")),
        (vec!["run"], Value::Null),
        (vec!["verify"], json!("verify")),
    ] {
        for explicit in [false, true] {
            let temp = fixture();
            let mut cmd = command(temp.path());
            cmd.args(&args).args(["--out=report.md"]);
            if explicit {
                cmd.args(["--task-type", "custom"]);
            }
            let output = cmd.output().unwrap();
            let entries = pair(temp.path(), &output);
            let start = &entries[0];
            assert_eq!(
                start["task_type"],
                if explicit {
                    json!("custom")
                } else {
                    inferred.clone()
                }
            );
            assert_eq!(start["actor"], "multi-ai");
            assert!(start["what"].as_str().unwrap().starts_with(args[0]));
            assert_eq!(
                Path::new(start["cwd"].as_str().unwrap())
                    .canonicalize()
                    .unwrap(),
                temp.path().canonicalize().unwrap()
            );
            assert!(start["pid"].as_u64().unwrap() > 0);
            assert!(start["host"].is_string());
            assert!(start["branch"].is_string());
            assert!(start.get("card").is_none());
            assert_eq!(entries[1]["out"], "report.md");
        }
    }
}

#[test]
fn card_precedence_and_valid_options() {
    for explicit in [false, true] {
        let temp = fixture();
        let mut cmd = command(temp.path());
        cmd.args(["verify", "--findings", "missing.md", "--task-type", "audit"])
            .env("AI_LEDGER_CARD", "env-card");
        if explicit {
            cmd.arg("--card=cli-card");
        }
        let output = cmd.output().unwrap();
        let entries = pair(temp.path(), &output);
        assert_eq!(
            entries[0]["card"],
            if explicit { "cli-card" } else { "env-card" }
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("Unknown argument"));
    }
}

#[test]
fn disabled_default_and_write_failures_preserve_exit() {
    for value in ["", "off"] {
        let temp = fixture();
        let output = command(temp.path())
            .env("AI_LEDGER_FILE", value)
            .args(["run", "--help"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(!temp.path().join("events.jsonl").exists());
        assert!(!temp.path().join("home/.local").exists());
    }
    let temp = fixture();
    let output = command(temp.path())
        .env_remove("AI_LEDGER_FILE")
        .args(["run", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        temp.path()
            .join("home/.local/state/ai-ledger/events.jsonl")
            .exists()
    );
    for args in [vec!["run", "--help"], vec!["run"]] {
        let baseline = command(temp.path())
            .env("AI_LEDGER_FILE", "off")
            .args(&args)
            .output()
            .unwrap();
        let output = command(temp.path())
            .env("AI_LEDGER_FILE", temp.path())
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), baseline.status.code());
        assert!(String::from_utf8_lossy(&output.stderr).contains("ledger append failed"));
    }
}

#[test]
fn metadata_cwd_outdir_escaping_and_parse_error() {
    let temp = fixture();
    let cwd = temp.path().join("target-dir");
    fs::create_dir(&cwd).unwrap();
    let output = command(temp.path())
        .args([
            "batch",
            "--cwd",
            cwd.to_str().unwrap(),
            "--outdir",
            "quoted\"\npath",
            "--card",
            "卡\n片",
            "--timeout",
        ])
        .output()
        .unwrap();
    let entries = pair(temp.path(), &output);
    assert_eq!(entries[0]["cwd"], cwd.to_str().unwrap());
    assert_eq!(entries[0]["out"], "quoted\"\npath");
    assert_eq!(entries[0]["card"], "卡\n片");
    assert_eq!(entries[1]["exit"], 2);
}

#[test]
fn concurrent_appends_are_complete_lines() {
    let temp = fixture();
    let children: Vec<_> = (0..24)
        .map(|_| {
            command(temp.path())
                .args(["run", "--help"])
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    let entries = events(temp.path());
    assert_eq!(entries.len(), 48);
    let ids: std::collections::HashSet<_> =
        entries.iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), 24);
    for id in ids {
        let matched: Vec<_> = entries.iter().filter(|e| e["id"] == id).collect();
        assert_eq!(matched.len(), 2);
        assert_eq!(matched[0]["event"], "start");
        assert_eq!(matched[1]["event"], "finish");
    }
}

fn hook_config(root: &Path, value: Value) {
    fs::create_dir_all(root.join(".multi-ai")).unwrap();
    fs::write(root.join(".multi-ai/hooks.json"), value.to_string()).unwrap();
}
fn implement(root: &Path) -> Output {
    command(root)
        .args([
            "delegate",
            "implement",
            "invalid-route",
            "--cwd",
            root.to_str().unwrap(),
        ])
        .output()
        .unwrap()
}
#[test]
fn absent_hooks_skip_and_required_failure_refuses_dispatch() {
    let temp = fixture();
    let baseline = implement(temp.path());
    assert!(!String::from_utf8_lossy(&baseline.stderr).contains("hook"));
    fs::remove_file(temp.path().join("events.jsonl")).unwrap();
    hook_config(
        temp.path(),
        json!({"pre_delegate_implement":[{"cmd":["/bin/sh", "-c", "echo checked > marker; exit 7"],"required":true}]}),
    );
    let output = implement(temp.path());
    assert!(!output.status.success());
    assert!(temp.path().join("marker").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("dispatch refused") && stderr.contains('7'));
    pair(temp.path(), &output);
}
#[test]
fn optional_failures_warn_and_successful_hooks_continue_in_order() {
    let temp = fixture();
    let baseline = implement(temp.path());
    fs::remove_file(temp.path().join("events.jsonl")).unwrap();
    hook_config(
        temp.path(),
        json!({"pre_delegate_implement":[
        {"cmd":["/bin/sh", "-c", "echo hook-banner; echo first > marker; exit 9"],"required":false},
        {"cmd":["/bin/sh", "-c", "echo second >> marker"],"required":true}]}),
    );
    let output = implement(temp.path());
    assert_eq!(output.status.code(), baseline.status.code());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning: optional"));
    assert!(stderr.contains("hook-banner"));
    assert!(output.stdout.is_empty());
    assert!(stderr.contains(String::from_utf8_lossy(&baseline.stderr).trim()));
    assert_eq!(
        fs::read_to_string(temp.path().join("marker")).unwrap(),
        "first\nsecond\n"
    );
    pair(temp.path(), &output);
}
#[test]
fn malformed_hooks_fail_closed_and_other_commands_ignore_hooks() {
    let temp = fixture();
    hook_config(
        temp.path(),
        json!({"pre_delegate_implement":[{"cmd":[],"required":true}]}),
    );
    assert!(String::from_utf8_lossy(&implement(temp.path()).stderr).contains("empty hook cmd"));
    fs::write(temp.path().join(".multi-ai/hooks.json"), "broken json").unwrap();
    assert!(
        String::from_utf8_lossy(&implement(temp.path()).stderr).contains("invalid pre-hook config")
    );
    let output = command(temp.path())
        .args(["run", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
}

#[test]
fn reordered_options_infer_implement_and_short_output_alias_is_recorded() {
    let temp = fixture();
    let output = command(temp.path())
        .args([
            "delegate",
            "--card",
            "C1",
            "implement",
            "invalid-route",
            "-o",
            "summary.md",
        ])
        .output()
        .unwrap();
    let entries = pair(temp.path(), &output);
    assert_eq!(entries[0]["task_type"], "implement");
    assert_eq!(entries[0]["out"], "summary.md");
    assert_eq!(entries[0]["card"], "C1");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Unknown argument"));
}

#[test]
fn cwd_git_identity_matches_the_selected_repository() {
    let temp = fixture();
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = command(temp.path())
        .args(["run", "--cwd", repo.to_str().unwrap()])
        .output()
        .unwrap();
    let entries = pair(temp.path(), &output);
    for (field, args) in [
        ("head", vec!["rev-parse", "HEAD"]),
        ("branch", vec!["rev-parse", "--abbrev-ref", "HEAD"]),
    ] {
        let expected = Command::new("/usr/bin/git")
            .env_clear()
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(expected.status.success());
        assert_eq!(
            entries[0][field],
            String::from_utf8(expected.stdout).unwrap().trim()
        );
    }
}
#[test]
fn hooks_see_allow_dirty_switch() {
    let temp = fixture();
    hook_config(
        temp.path(),
        json!({"pre_delegate_implement":[{"cmd":["/bin/sh", "-c", "echo $MULTI_AI_ALLOW_DIRTY >> marker"],"required":true}]}),
    );
    implement(temp.path());
    let root = temp.path().to_str().unwrap();
    command(temp.path())
        .args([
            "delegate",
            "implement",
            "invalid-route",
            "--cwd",
            root,
            "--allow-dirty",
        ])
        .output()
        .unwrap();
    assert_eq!(
        fs::read_to_string(temp.path().join("marker")).unwrap(),
        "0\n1\n"
    );
}

fn host() -> String {
    String::from_utf8(Command::new("hostname").output().unwrap().stdout)
        .unwrap()
        .trim()
        .to_owned()
}
fn alive(pid: &str) -> bool {
    Command::new("/bin/ps")
        .args(["-p", pid, "-o", "pid="])
        .output()
        .unwrap()
        .stdout
        .iter()
        .any(u8::is_ascii_digit)
}
fn wait_for(mut done: impl FnMut() -> bool) {
    for _ in 0..100 {
        if done() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("condition not reached");
}

#[test]
fn stop_ends_only_the_named_run_and_its_vendor_group() {
    let temp = fixture();
    let root = temp.path();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(root.join("slow.sh"), "echo $$ > child.pid\nsleep 30\n").unwrap();
    fs::write(
        root.join("config/routes.json"),
        serde_json::to_vec(&json!([{"id":"slow","legacy_id":"slow","pool":"p","family":"test","label":"slow","agentic":true,"cost":"sub","rank":1,"argv":["/bin/sh",root.join("slow.sh"),"{prompt}"],"extract":"stdout","default_timeout":60}])).unwrap(),
    )
    .unwrap();
    let mut run = command(root)
        .args(["run", "slow", "task", "--out", "answer.md"])
        .spawn()
        .unwrap();
    wait_for(|| root.join("child.pid").exists() && root.join("events.jsonl").exists());
    let child = fs::read_to_string(root.join("child.pid"))
        .unwrap()
        .trim()
        .to_owned();
    let id = events(root)[0]["id"].as_str().unwrap().to_owned();
    let list = command(root).args(["stop", "--list"]).output().unwrap();
    let listed = String::from_utf8_lossy(&list.stdout);
    assert!(
        listed.contains(&id) && listed.contains("running"),
        "{listed}"
    );
    let stop = command(root).args(["stop", &id[..15]]).output().unwrap();
    assert!(
        stop.status.success(),
        "{}",
        String::from_utf8_lossy(&stop.stderr)
    );
    assert!(!run.wait().unwrap().success());
    wait_for(|| !alive(&child));
    assert_eq!(events(root).last().unwrap()["event"], "finish");
    let again = command(root).args(["stop", &id]).output().unwrap();
    assert!(!again.status.success());
}

#[test]
fn stop_never_signals_a_reused_pid() {
    let temp = fixture();
    let root = temp.path();
    let mut stranger = Command::new("/bin/sleep").arg("30").spawn().unwrap();
    let pid = stranger.id();
    fs::write(
        root.join("events.jsonl"),
        format!(
            "{}\n",
            json!({"v":1,"event":"start","actor":"multi-ai","id":"20990101T000000-1-x","pid":pid,"host":host(),"what":"run slow","at":"2099-01-01T00:00:00Z"})
        ),
    )
    .unwrap();
    let stop = command(root).args(["stop", "20990101"]).output().unwrap();
    assert!(!stop.status.success());
    assert!(String::from_utf8_lossy(&stop.stderr).contains("进程已不在"));
    assert!(
        stranger.try_wait().unwrap().is_none(),
        "unrelated process was signalled"
    );
    stranger.kill().unwrap();
    let _ = stranger.wait();
}

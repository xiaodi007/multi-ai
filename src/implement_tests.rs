use super::*;
use std::{os::unix::fs::symlink, process::Stdio, time::Duration};

fn init_repo(root: &Path) {
    fs::create_dir_all(root).unwrap();
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.email", "fixture@example.invalid"],
        vec!["config", "user.name", "Fixture"],
    ] {
        git(root, &args).unwrap();
    }
    fs::write(root.join("tracked.txt"), "original\n").unwrap();
    git(root, &["add", "tracked.txt"]).unwrap();
    git(
        root,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
    )
    .unwrap();
}
fn fixture(scenario: &str) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for directory in ["config", "bin", "home"] {
        fs::create_dir(root.join(directory)).unwrap();
    }
    init_repo(&root.join("repo"));
    fs::write(
        root.join("task.md"),
        "Change the fixture and report the actual checks.",
    )
    .unwrap();
    fs::write(root.join("summary.md"), "previous complete summary").unwrap();
    let mut routes: Value = serde_json::from_str(include_str!("../config/routes.json")).unwrap();
    let route = routes
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|r| r["id"] == "codex-cli")
        .unwrap();
    route["argv"] = json!([
        "never-execute-this-wrapper",
        "--dangerously-bypass-approvals-and-sandbox",
        "--add-dir",
        "/",
        "--model=fixture-model"
    ]);
    route["pool"] = json!(format!("fixture-{scenario}"));
    fs::write(
        root.join("config/routes.json"),
        serde_json::to_vec(&json!([route])).unwrap(),
    )
    .unwrap();
    let body = match scenario {
        "quota" => {
            "echo ' M unrelated.rs' >&2\necho \"403 You've reached your 5-hour usage limit\" >&2\nexit 1\n"
        }
        "timeout" | "cancel" => {
            "(sleep 10; touch \"$FIXTURE_ROOT/descendant-survived\") &\necho $! > \"$FIXTURE_ROOT/descendant-pid\"\nsleep 30\n"
        }
        "empty" => "exit 0\n",
        "after-error" => {
            "rm -rf .git\nprintf 'gitdir: /nonexistent/fixture' > .git\necho \"403 You've reached your 5-hour usage limit\" >&2\nexit 1\n"
        }
        _ => {
            "printf changed > tracked.txt\nprintf new > 'new file.txt'\nprintf 'Implemented fixture; tests: fixture check passed.' > \"$last\"\n"
        }
    };
    let executable = root.join("bin/codex");
    fs::write(&executable, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$FIXTURE_ROOT/argv\"\nprintf '%s' \"$PATH\" > \"$FIXTURE_ROOT/child-path\"\ntouch \"$FIXTURE_ROOT/started\"\nlast=\nwhile [ \"$#\" -gt 0 ]; do\nif [ \"$1\" = -o ]; then shift; last=$1; fi\nshift\ndone\n{body}")).unwrap();
    fs::set_permissions(executable, fs::Permissions::from_mode(0o700)).unwrap();
    if scenario == "dirty" || scenario == "allow-dirty" {
        fs::write(root.join("repo/tracked.txt"), "preexisting\n").unwrap();
        fs::write(root.join("repo/preexisting.txt"), "preserve me\n").unwrap();
    }
    temp
}
fn run_scenario(scenario: &str) {
    let temp = fixture(scenario);
    let root = temp.path();
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "implement::tests::isolated_worker",
            "--nocapture",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.join("home"))
        .env("MULTI_AI_CONFIG_DIR", root.join("config"))
        .env("MULTI_AI_STATE_DIR", root.join("state"))
        .env("FIXTURE_ROOT", root)
        .env("FIXTURE_SCENARIO", scenario)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if scenario == "cancel" {
        let start = Instant::now();
        while !root.join("descendant-pid").exists() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            root.join("descendant-pid").exists(),
            "fixture did not start"
        );
        unsafe {
            libc::kill(child.id() as i32, libc::SIGTERM);
        }
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "scenario {scenario}: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if scenario == "timeout" || scenario == "cancel" {
        let pid = fs::read_to_string(root.join("descendant-pid"))
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        let start = Instant::now();
        while unsafe { libc::kill(pid, 0) } == 0 && start.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_ne!(
            unsafe { libc::kill(pid, 0) },
            0,
            "descendant survived timeout/cancellation"
        );
        assert!(!root.join("descendant-survived").exists());
    }
}
#[test]
fn isolated_worker() {
    let Ok(scenario) = std::env::var("FIXTURE_SCENARIO") else {
        return;
    };
    let root = PathBuf::from(std::env::var("FIXTURE_ROOT").unwrap());
    let mut args = Args {
        positional: vec!["implement".into(), "codex-cli".into()],
        ..Args::default()
    };
    args.set("cwd", root.join("repo").to_string_lossy());
    args.set("out", root.join("summary.md").to_string_lossy());
    args.set("prompt-file", root.join("task.md").to_string_lossy());
    args.set("path-prepend", root.join("bin").to_string_lossy());
    args.set("timeout", if scenario == "timeout" { "5" } else { "10" });
    if scenario == "allow-dirty" {
        args.flags.insert("allow-dirty".into());
    }
    if scenario == "subdirectory" {
        fs::create_dir(root.join("repo/sub")).unwrap();
        args.set("cwd", root.join("repo/sub").to_string_lossy());
    }
    if scenario == "output-inside" {
        args.set("out", root.join("repo/summary.md").to_string_lossy());
    }
    if scenario == "overlap" {
        let _lock = OutputLock::acquire(&[(root.join("repo"), true)]).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert!(
            rt.block_on(command(&Config::load().unwrap(), &args))
                .is_err()
        );
        assert!(!root.join("started").exists());
        return;
    }
    let config = Config::load().unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let result = runtime.block_on(command(&config, &args));
    if scenario == "output-inside" {
        assert!(result.is_err());
        assert!(!root.join("started").exists());
        return;
    }
    let rc = result.unwrap();
    let status: Value =
        serde_json::from_slice(&fs::read(root.join("summary.md.status.json")).unwrap()).unwrap();
    match scenario.as_str() {
        "dirty" | "subdirectory" => {
            assert_eq!(rc, 1);
            assert!(!root.join("started").exists());
            assert_eq!(
                status["status"],
                if scenario == "dirty" {
                    "dirty"
                } else {
                    "error"
                }
            );
        }
        "quota" | "timeout" | "cancel" | "empty" | "after-error" => {
            assert_eq!(rc, 1);
            assert_eq!(
                status["status"],
                match scenario.as_str() {
                    "after-error" => "quota",
                    "cancel" => "cancelled",
                    other => other,
                },
                "{status}"
            );
            assert_eq!(
                fs::read_to_string(root.join("summary.md")).unwrap(),
                "previous complete summary"
            );
            if scenario == "quota" || scenario == "after-error" {
                assert!(
                    status["error"]
                        .as_str()
                        .unwrap()
                        .contains("5-hour usage limit")
                );
                assert!(root.join("state/review/state.json").is_file());
            }
            if scenario == "after-error" {
                assert!(status["git_after_error"].is_string());
            }
        }
        _ => {
            assert_eq!(rc, 0, "{status}");
            assert_eq!(status["status"], "ok");
            assert_eq!(status["git_before"]["head"], status["git_after"]["head"]);
            assert_eq!(
                status["changed_files"],
                json!(["new file.txt", "tracked.txt"])
            );
            assert!(
                fs::read_to_string(root.join("summary.md"))
                    .unwrap()
                    .contains("Implemented fixture")
            );
            if scenario == "allow-dirty" {
                assert_eq!(status["changed_preexisting_files"], json!(["tracked.txt"]));
                assert_eq!(
                    fs::read_to_string(root.join("repo/preexisting.txt")).unwrap(),
                    "preserve me\n"
                );
            } else {
                assert_eq!(status["preexisting_files"], json!([]));
            }
            let argv = fs::read_to_string(root.join("argv")).unwrap();
            for expected in [
                "-C",
                "-s",
                "workspace-write",
                "--skip-git-repo-check",
                "-o",
                "fixture-model",
            ] {
                assert!(argv.contains(expected), "missing {expected}: {argv}");
            }
            assert!(!argv.contains("sandbox-exec"));
            assert!(!argv.contains("network_access"));
            assert!(!argv.contains("--config"));
            assert!(!argv.contains("--ignore-user-config"));
            assert!(!argv.contains("--add-dir"));
            assert!(
                !Path::new(status["artifacts"].as_str().unwrap())
                    .join("sandbox.sb")
                    .exists()
            );
            assert!(!argv.contains("dangerously"));
            let child_path = fs::read_to_string(root.join("child-path")).unwrap();
            assert_eq!(
                std::env::split_paths(&child_path).next().unwrap(),
                fs::canonicalize(root.join("bin")).unwrap()
            );
            assert_eq!(std::env::var("PATH").unwrap(), "/usr/bin:/bin");
        }
    }
    assert_eq!(status["automatic_commit"], false);
}
#[test]
fn clean_tree_records_changes_and_prepends_child_path() {
    run_scenario("clean");
}
#[test]
fn dirty_tree_refuses_before_starting_codex() {
    run_scenario("dirty");
}
#[test]
fn allow_dirty_distinguishes_preexisting_changes() {
    run_scenario("allow-dirty");
}
#[test]
fn quota_preserves_summary_and_cools_pool() {
    run_scenario("quota");
}
#[test]
fn timeout_kills_descendants_and_preserves_summary() {
    run_scenario("timeout");
}
#[test]
fn cancellation_kills_descendants() {
    run_scenario("cancel");
}
#[test]
fn empty_response_preserves_summary() {
    run_scenario("empty");
}
#[test]
fn git_after_failure_preserves_quota_reason() {
    run_scenario("after-error");
}
#[test]
fn implementation_requires_git_root() {
    run_scenario("subdirectory");
}
#[test]
fn output_cannot_overlap_workspace() {
    run_scenario("output-inside");
}
#[test]
fn exclusive_workspace_lock_refuses_overlap() {
    run_scenario("overlap");
}

#[test]
fn snapshot_does_not_follow_external_parent_symlinks() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    init_repo(&root);
    fs::create_dir(root.join("nested")).unwrap();
    fs::write(root.join("nested/source"), "fixture").unwrap();
    git(&root, &["add", "nested/source"]).unwrap();
    fs::remove_dir_all(root.join("nested")).unwrap();
    let external = temp.path().join("external");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("source"), "must not read").unwrap();
    symlink(external, root.join("nested")).unwrap();
    let failure = snapshot(&root).err().unwrap();
    assert!(failure.to_string().contains("outside implementation cwd"));
}
#[test]
fn model_parser_is_shared_with_display_and_controlled_execution() {
    // Deserialization uses embedded profiles only; this test never loads user configuration.
    let mut route: Route = serde_json::from_value(json!({"id":"codex-cli","legacy_id":"codex","pool":"test","family":"test","label":"fixture","agentic":true,"cost":"sub","rank":1,"argv":["codex","-m","first","--model=last"],"extract":"last-message-file","default_timeout":10})).unwrap();
    assert_eq!(route.explicit_model().as_deref(), Some("last"));
    let controlled = controlled_route(
        &route,
        Path::new("/fixture/codex"),
        Path::new("/fixture/worktree"),
    );
    assert_eq!(controlled.explicit_model(), route.explicit_model());
    route.argv = vec!["codex".into(), "--model".into(), "{prompt}".into()];
    assert_eq!(route.explicit_model(), None);
}

#[test]
fn boundary_scan_records_internal_symlink_without_following_target() {
    let temp = tempfile::tempdir().unwrap();
    let monitored = temp.path().join("monitored");
    let target = temp.path().join("external-target");
    let cwd = temp.path().join("worktree");
    fs::create_dir(&monitored).unwrap();
    fs::create_dir(&target).unwrap();
    fs::write(target.join("private.txt"), "fixture target contents").unwrap();
    let link = monitored.join("linked-directory");
    symlink(&target, &link).unwrap();
    let new_snapshot = || BoundarySnapshot {
        roots: BTreeSet::from([monitored.clone()]),
        entries: BTreeMap::new(),
        errors: Vec::new(),
        excluded_artifacts: temp.path().join("artifacts"),
    };

    let mut before = new_snapshot();
    scan_mtimes(&monitored, &cwd, &mut before);
    assert!(before.errors.is_empty());
    assert_eq!(before.entries.len(), 2);
    assert_eq!(before.entries[&link].kind, "symlink");
    assert!(!before.entries.contains_key(&link.join("private.txt")));
    assert!(!before.entries.contains_key(&target));

    fs::write(
        target.join("private.txt"),
        "changed fixture target contents",
    )
    .unwrap();
    let mut after = new_snapshot();
    scan_mtimes(&monitored, &cwd, &mut after);
    assert!(after.errors.is_empty());
    assert!(boundary_changes(&before, &after).is_empty());
}

#[test]
fn boundary_scan_error_does_not_turn_unknown_entries_into_deletions() {
    let temp = tempfile::tempdir().unwrap();
    let monitored = temp.path().join("monitored");
    let unreadable = monitored.join("partially-unreadable");
    let unknown = unreadable.join("not-scanned.txt");
    let observed = unreadable.join("observed.txt");
    let entry = |seconds| MtimeEntry {
        kind: "file",
        mtime_seconds: seconds,
        mtime_nanoseconds: 0,
        length: 7,
    };
    let before = BoundarySnapshot {
        roots: BTreeSet::from([monitored.clone()]),
        entries: BTreeMap::from([(unknown, entry(10)), (observed.clone(), entry(10))]),
        errors: Vec::new(),
        excluded_artifacts: temp.path().join("artifacts"),
    };
    let mut after = BoundarySnapshot {
        roots: BTreeSet::from([monitored]),
        entries: BTreeMap::from([(observed.clone(), entry(10))]),
        errors: vec![BoundaryError {
            path: unreadable,
            error: "fixture read_dir interrupted after a partial scan".into(),
        }],
        excluded_artifacts: temp.path().join("artifacts"),
    };
    assert!(boundary_changes(&before, &after).is_empty());

    // A known change remains evidence even when another entry could not be inspected.
    after.entries.insert(observed.clone(), entry(11));
    assert_eq!(boundary_changes(&before, &after), vec![observed]);
}

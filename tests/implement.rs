use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, SystemTime},
};

struct Fixture {
    temp: tempfile::TempDir,
    cwd: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("worktree");
        for directory in ["worktree", "sibling", "bin", "config", "home"] {
            fs::create_dir(temp.path().join(directory)).unwrap();
        }
        let f = Self { temp, cwd };
        f.seed("worktree/tracked.txt", "original");
        f.seed("sibling/nested/keep", "before");
        f.seed("home/.claude/skills/custom/keep", "before");
        f.seed("home/.agents/nested/remove", "delete me");
        std::os::unix::fs::symlink(f.path("sibling"), f.cwd.join("outside-link")).unwrap();
        for worktree in [&f.cwd, &f.path("sibling")] {
            f.git_at(worktree, &["init", "--quiet", "--template="]);
            f.git_at(worktree, &["add", "."]);
            f.git_at(
                worktree,
                &[
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "--quiet",
                    "-m",
                    "fixture",
                ],
            );
        }
        fs::write(
            f.path("brief.md"),
            "Modify only the fictional fixture; report verification.",
        )
        .unwrap();
        f.fake("printf changed > tracked.txt\nprintf created > changed.txt\nprintf 'fixture implementation complete' > \"$out\"\n");
        f
    }
    fn path(&self, relative: &str) -> PathBuf {
        fs::canonicalize(self.temp.path()).unwrap().join(relative)
    }
    fn seed(&self, relative: &str, content: &str) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        // Same-length rewrites still differ deterministically from this old timestamp.
        fs::File::open(path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(946_684_800))
            .unwrap();
    }
    fn fake(&self, body: &str) {
        let fake = self.path("bin/codex");
        fs::write(
            &fake,
            format!(
                r#"#!/bin/sh
printf '%s\0' "$@" > "$FAKE_ROOT/argv"
out=''
while [ "$#" -gt 0 ]; do
    if [ "$1" = -o ] || [ "$1" = --output-last-message ]; then shift; out=$1; fi
    shift
done
{body}"#
            ),
        )
        .unwrap();
        fs::set_permissions(fake, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn git_at(&self, cwd: &Path, args: &[&str]) -> Output {
        let out = Command::new("/usr/bin/git")
            .env_clear()
            .env("HOME", self.path("home"))
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }
    fn git(&self, args: &[&str]) -> Output {
        self.git_at(&self.cwd, args)
    }
    fn cmd(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_multi-ai"));
        cmd.env_clear()
            .env("HOME", self.path("home"))
            .env("PATH", "/usr/bin:/bin")
            .env("MULTI_AI_CONFIG_DIR", self.path("config"))
            .env("MULTI_AI_STATE_DIR", self.path("state"))
            .env("FAKE_ROOT", self.temp.path())
            .current_dir(self.temp.path());
        cmd
    }
    fn run(&self, extra: &[&str]) -> Output {
        self.cmd()
            .args(["delegate", "implement", "codex-cli", "--cwd"])
            .arg(&self.cwd)
            .arg("--prompt-file")
            .arg(self.path("brief.md"))
            .arg("--out")
            .arg(self.path("summary.md"))
            .arg("--path-prepend")
            .arg(self.path("bin"))
            .args(extra)
            .output()
            .unwrap()
    }
    fn status(&self) -> Value {
        serde_json::from_slice(&fs::read(self.path("summary.md.status.json")).unwrap()).unwrap()
    }
    fn preserve_summary(&self) {
        fs::write(self.path("summary.md"), "previous complete summary").unwrap();
    }
    fn assert_boundary(&self, out: &Output, paths: &[&str]) -> Value {
        assert!(!out.status.success());
        let status = self.status();
        assert_eq!(status["status"], "boundary_violation", "{status}");
        assert_eq!(status["complete"], false);
        assert_eq!(status["automatic_commit"], false);
        let violations = status["boundary_violations"].as_array().unwrap();
        assert!(
            violations
                .iter()
                .all(|path| Path::new(path.as_str().unwrap()).is_absolute())
        );
        for path in paths {
            let expected = self.path(path);
            assert!(
                violations.iter().any(|path| {
                    path.as_str()
                        .is_some_and(|path| Path::new(path) == expected)
                }),
                "missing {}: {status}",
                expected.display()
            );
        }
        assert!(status["boundary_before"]["roots"].is_array());
        assert!(status["boundary_after"]["roots"].is_array());
        let artifacts = Path::new(status["artifacts"].as_str().unwrap());
        for file in ["boundary-before.json", "boundary-after.json"] {
            let snapshot: Value =
                serde_json::from_slice(&fs::read(artifacts.join(file)).unwrap()).unwrap();
            assert!(snapshot.is_object());
        }
        let summary = fs::read_to_string(self.path("summary.md")).unwrap();
        if status["execution_status"] == "ok" && !status["git_after_error"].is_string() {
            assert!(summary.starts_with("# 警告："), "{summary}");
            assert_eq!(status["summary_published"], true);
            assert!(summary.contains("implementation finished"));
        } else {
            assert_eq!(summary, "previous complete summary");
        }
        status
    }
}

#[test]
fn public_implementation_help_and_argument_boundaries_are_offline() {
    let f = Fixture::new();
    let help = f
        .cmd()
        .args(["delegate", "implement", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--allow-dirty"));
    assert!(String::from_utf8_lossy(&help.stdout).contains("--network"));
    let out = f.run(&["--network=false"]);
    assert!(!out.status.success());
    assert!(!f.path("argv").exists());
    let out = f.run(&["--resume"]);
    assert!(!out.status.success());
    assert!(!f.path("argv").exists());
    let out = f
        .cmd()
        .args(["delegate", "implement", "grok-cli", "--cwd"])
        .arg(&f.cwd)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let out = f.run(&["--out", f.cwd.join("inside.md").to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(!f.path("argv").exists());
}

#[test]
fn public_dirty_worktree_refuses_before_start_and_records_head() {
    let f = Fixture::new();
    fs::write(f.cwd.join("tracked.txt"), "preexisting edit").unwrap();
    let before = f.git(&["rev-parse", "HEAD"]).stdout;
    let out = f.run(&["--network"]);
    assert!(!out.status.success());
    let status = f.status();
    assert_eq!(status["network"], true);
    assert_eq!(status["status"], "dirty");
    assert_eq!(
        status["git_before"]["head"].as_str().unwrap(),
        String::from_utf8(before).unwrap().trim()
    );
    assert!(
        status["git_before"]["status_porcelain"]
            .as_str()
            .unwrap()
            .contains("tracked.txt")
    );
    assert_eq!(status["preexisting_files"][0], "tracked.txt");
    assert!(!f.path("argv").exists());
}

#[test]
fn public_fake_codex_receives_native_workspace_write_arguments_and_changes_cwd() {
    for network in [false, true] {
        let f = Fixture::new();
        let branch = String::from_utf8(f.git(&["symbolic-ref", "--short", "HEAD"]).stdout).unwrap();
        let branch = branch.trim();
        f.git(&["tag", branch]);
        let extra = if network {
            vec!["--network", "--timeout", "10"]
        } else {
            vec!["--timeout", "10"]
        };
        let out = f.run(&extra);
        let status = f.status();
        assert!(out.status.success(), "{status}");
        assert_eq!(status["status"], "ok");
        assert_eq!(status["network"], network);
        assert_eq!(
            status["changed_files"],
            json!(["changed.txt", "tracked.txt"])
        );
        assert_eq!(status["git_before"]["head"], status["git_after"]["head"]);
        assert_eq!(status["git_before"]["branch"], branch);
        assert_eq!(status["git_after"]["branch"], branch);
        assert_eq!(status["git_before"]["status_porcelain"], "");
        assert!(
            status["git_after"]["status_porcelain"]
                .as_str()
                .unwrap()
                .contains("tracked.txt")
        );
        assert_eq!(status["boundary_violations"], json!([]));
        assert!(
            fs::read_to_string(f.path("summary.md"))
                .unwrap()
                .contains("fixture implementation complete")
        );
        let args = fs::read(f.path("argv"))
            .unwrap()
            .split(|byte| *byte == 0)
            .filter(|arg| !arg.is_empty())
            .map(|arg| String::from_utf8(arg.to_vec()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(args[0], "exec");
        for (option, expected) in [
            (
                "-C",
                fs::canonicalize(&f.cwd)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("-s", "workspace-write".into()),
        ] {
            let index = args.iter().position(|arg| arg == option).unwrap();
            assert_eq!(args[index + 1], expected);
        }
        assert!(args.iter().any(|arg| arg == "--skip-git-repo-check"));
        let output_index = args.iter().position(|arg| arg == "-o").unwrap();
        let output = Path::new(&args[output_index + 1]);
        assert!(output.is_absolute());
        assert!(output.ends_with("last-message.txt"));
        for forbidden in [
            "sandbox-exec",
            "ignore-user-config",
            "ignore-rules",
            "--add-dir",
            "dangerously",
        ] {
            assert!(
                args.iter().all(|arg| !arg.contains(forbidden)),
                "unexpected {forbidden}: {args:?}"
            );
        }
        let overrides = args
            .windows(2)
            .filter(|pair| pair[0] == "-c")
            .map(|pair| pair[1].as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            overrides,
            if network {
                vec!["sandbox_workspace_write.network_access=true"]
            } else {
                vec![]
            }
        );
        assert!(!args.iter().any(|arg| arg.contains("danger-full-access")));
    }
}

#[test]
fn public_postcheck_reports_recursive_rewrites_creation_and_deletion_without_rollback() {
    let f = Fixture::new();
    f.preserve_summary();
    f.fake(
        r#"printf '%s\n' '{"type":"item.completed","item":{"type":"file_change","changes":[{"path":"../sibling/nested/keep","kind":"update"}],"status":"completed"}}'
printf mutate > outside-link/nested/keep
printf mutate > "$HOME/.claude/skills/custom/keep"
printf created > "$HOME/.claude/skills/custom/created"
rm "$HOME/.agents/nested/remove"
printf 'implementation finished' > "$out"
"#,
    );
    let out = f.run(&[]);
    f.assert_boundary(
        &out,
        &[
            "sibling/nested/keep",
            "home/.claude/skills/custom/keep",
            "home/.claude/skills/custom/created",
            "home/.agents/nested/remove",
        ],
    );
    assert_eq!(
        fs::read_to_string(f.path("sibling/nested/keep")).unwrap(),
        "mutate"
    );
    assert_eq!(
        fs::read_to_string(f.path("home/.claude/skills/custom/keep")).unwrap(),
        "mutate"
    );
    assert!(f.path("home/.claude/skills/custom/created").is_file());
    assert!(!f.path("home/.agents/nested/remove").exists());
}

#[test]
fn public_postcheck_detects_sensitive_directories_created_after_start() {
    let f = Fixture::new();
    f.preserve_summary();
    fs::remove_dir_all(f.path("home/.claude")).unwrap();
    fs::remove_dir_all(f.path("home/.agents")).unwrap();
    f.fake(
        r#"mkdir -p "$HOME/.claude/skills/custom" "$HOME/.agents/nested"
printf created > "$HOME/.claude/skills/custom/created"
printf created > "$HOME/.agents/nested/created"
printf 'implementation finished' > "$out"
"#,
    );
    let out = f.run(&[]);
    f.assert_boundary(
        &out,
        &[
            "home/.claude/skills/custom/created",
            "home/.agents/nested/created",
        ],
    );
    assert!(f.path("home/.claude/skills/custom/created").is_file());
    assert!(f.path("home/.agents/nested/created").is_file());
}

#[test]
fn public_postcheck_runs_after_codex_failure_and_retains_response_reason() {
    let f = Fixture::new();
    f.preserve_summary();
    f.fake(
        r#"printf '%s\n' '{"type":"item.completed","item":{"type":"file_change","changes":[{"path":"../sibling/nested/keep","kind":"update"}],"status":"completed"}}'
printf mutate > "$FAKE_ROOT/sibling/nested/keep"
echo 'fixture execution failed' >&2
exit 7
"#,
    );
    let out = f.run(&[]);
    let status = f.assert_boundary(&out, &["sibling/nested/keep"]);
    assert_eq!(status["response"]["status"], "error");
    assert!(
        status["response"]["error"]
            .as_str()
            .unwrap()
            .contains("fixture execution failed")
    );
    assert_eq!(
        fs::read_to_string(f.path("sibling/nested/keep")).unwrap(),
        "mutate"
    );
}

#[test]
fn public_postcheck_reports_boundary_even_when_git_after_snapshot_fails() {
    let f = Fixture::new();
    f.preserve_summary();
    f.fake(
        r#"printf '%s\n' '{"type":"item.completed","item":{"type":"file_change","changes":[{"path":"../sibling/nested/keep","kind":"update"}],"status":"completed"}}'
printf mutate > "$FAKE_ROOT/sibling/nested/keep"
rm -rf .git
printf 'gitdir: /nonexistent/fixture' > .git
echo 'fixture execution failed' >&2
exit 7
"#,
    );
    let out = f.run(&[]);
    let status = f.assert_boundary(&out, &["sibling/nested/keep"]);
    assert!(status["git_after_error"].is_string());
    assert_eq!(status["response"]["status"], "error");
    assert_eq!(
        fs::read_to_string(f.path("sibling/nested/keep")).unwrap(),
        "mutate"
    );
    assert_eq!(
        fs::read_to_string(f.cwd.join(".git")).unwrap(),
        "gitdir: /nonexistent/fixture"
    );
}

#[test]
fn public_postcheck_retains_quota_response_and_cooldown_after_boundary_violation() {
    let f = Fixture::new();
    f.preserve_summary();
    f.fake(
        r#"printf '%s\n' '{"type":"item.completed","item":{"type":"file_change","changes":[{"path":"../sibling/nested/keep","kind":"update"}],"status":"completed"}}'
printf mutate > "$FAKE_ROOT/sibling/nested/keep"
echo "403 You've reached your 5-hour usage limit" >&2
exit 1
"#,
    );
    let out = f.run(&[]);
    let status = f.assert_boundary(&out, &["sibling/nested/keep"]);
    assert_eq!(status["response"]["status"], "quota");
    assert!(
        status["response"]["error"]
            .as_str()
            .unwrap()
            .contains("5-hour usage limit")
    );
    assert!(f.path("state/review/state.json").is_file());
}

#[test]
fn unrelated_new_sibling_repository_is_not_scanned() {
    let f = Fixture::new();
    f.preserve_summary();
    f.fake(
        r#"/usr/bin/git init --quiet --template= "$FAKE_ROOT/new-worktree"
printf created > "$FAKE_ROOT/new-worktree/created"
printf 'implementation finished' > "$out"
"#,
    );
    let out = f.run(&[]);
    assert!(out.status.success());
    assert!(
        !f.status()["boundary_after"]["roots"]
            .as_array()
            .unwrap()
            .contains(&json!(f.path("new-worktree")))
    );
    assert!(f.path("new-worktree/created").is_file());
}

#[test]
fn public_summary_inside_monitored_sibling_excludes_only_current_attempt_artifacts() {
    let f = Fixture::new();
    let summary = f.path("sibling/summary.md");
    let out = f.run(&["--out", summary.to_str().unwrap()]);
    let status: Value =
        serde_json::from_slice(&fs::read(f.path("sibling/summary.md.status.json")).unwrap())
            .unwrap();
    assert!(out.status.success(), "{status}");
    assert_eq!(status["status"], "ok");
    assert_eq!(status["boundary_violations"], json!([]));
    for snapshot in ["boundary_before", "boundary_after"] {
        let excluded = status[snapshot]["excluded_artifacts"].as_array().unwrap();
        assert!(excluded.contains(&status["artifacts"]));
    }
    assert!(
        fs::read_to_string(summary)
            .unwrap()
            .contains("fixture implementation complete")
    );
    assert_eq!(
        fs::read_to_string(f.path("sibling/nested/keep")).unwrap(),
        "before"
    );
}

#[test]
fn host_sessions_and_build_caches_do_not_trigger_boundary_but_settings_do() {
    let f = Fixture::new();
    f.fake(r#"mkdir -p "$HOME/.claude/projects" "$HOME/.claude/sessions" "$HOME/.claude/skills/synced" "$FAKE_ROOT/sibling/node_modules" "$FAKE_ROOT/sibling/target" "$FAKE_ROOT/sibling/dist" "$FAKE_ROOT/sibling/.git/objects/cache"
printf x > "$HOME/.claude/projects/log.jsonl"
printf x > "$HOME/.claude/sessions/log"
printf x > "$HOME/.claude/skills/synced/skill"
printf x > "$FAKE_ROOT/sibling/node_modules/large"
printf x > "$FAKE_ROOT/sibling/target/large"
printf x > "$FAKE_ROOT/sibling/dist/large"
printf x > "$FAKE_ROOT/sibling/.git/objects/cache/large"
printf 'implementation finished' > "$out"
"#);
    let out = f.run(&[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(f.status()["status"], "ok");
    f.fake(
        "printf x > \"$HOME/.claude/settings.json\"\nprintf 'implementation finished' > \"$out\"\n",
    );
    let out = f.run(&[]);
    f.assert_boundary(&out, &["home/.claude/settings.json"]);
}
#[test]
fn concurrent_implementations_exclude_overlapping_intervals_even_after_peer_finishes() {
    use std::{process::Stdio, time::Instant};
    let f = Fixture::new();
    f.fake(
        r#"name=$(basename "$PWD")
touch "$FAKE_ROOT/$name.ready"
while [ ! -f "$FAKE_ROOT/release" ]; do sleep 0.02; done
if [ "$name" = worktree ]; then
  while [ ! -f "$FAKE_ROOT/peer-returned" ]; do sleep 0.02; done
fi
printf changed > nested-change
printf 'implementation finished' > "$out"
"#,
    );
    let spawn = |cwd: &Path, output: PathBuf| {
        f.cmd()
            .args(["delegate", "implement", "codex-cli", "--cwd"])
            .arg(cwd)
            .arg("--out")
            .arg(output)
            .arg("--prompt-file")
            .arg(f.path("brief.md"))
            .arg("--path-prepend")
            .arg(f.path("bin"))
            .arg("--timeout")
            .arg("15")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let first = spawn(&f.cwd, f.path("summary.md"));
    let peer = spawn(&f.path("sibling"), f.path("peer.md"));
    let start = Instant::now();
    while !(f.path("worktree.ready").exists() && f.path("sibling.ready").exists()) {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "fake workers never became ready"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    fs::write(f.path("release"), "go").unwrap();
    let peer = peer.wait_with_output().unwrap();
    assert!(
        peer.status.success(),
        "{}",
        String::from_utf8_lossy(&peer.stderr)
    );
    fs::write(f.path("peer-returned"), "go").unwrap();
    let first = first.wait_with_output().unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let status = f.status();
    assert_eq!(status["boundary_violations"], json!([]));
    assert!(
        status["boundary_after"]["excluded_worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p.as_str() == f.path("sibling").to_str())
    );
}

fn feedback(f: &Fixture, status: &Value, extra: &[&str]) -> Output {
    f.cmd()
        .args([
            "feedback",
            "--run",
            status["run_id"].as_str().unwrap(),
            "--report",
            status["attempt_id"].as_str().unwrap(),
            "--evidence",
            "synthetic host verification",
        ])
        .args(extra)
        .output()
        .unwrap()
}
fn score(f: &Fixture) -> Value {
    let out = f
        .cmd()
        .args(["scorecard", "--task-type", "implement", "--json"])
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
fn attributed_patch_registers_diagnostic_report_and_implementation_feedback() {
    let f = Fixture::new();
    f.fake(r#"printf '%s\n' '{"type":"thread.started","thread_id":"fixture"}' '{"type":"turn.started","model":"fixture-model"}' '{"type":"item.completed","item":{"type":"file_change","status":"completed","changes":[{"path":"../sibling/nested/keep","kind":"update"}]}}' '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}'
printf 'implementation finished' > "$out"
"#);
    let out = f.run(&[]);
    let status = f.assert_boundary(&out, &["sibling/nested/keep"]);
    assert_eq!(status["attribution"], "available");
    assert_eq!(status["attributed_boundary_count"], 1);
    assert_eq!(status["task_type"], "implement");
    assert_eq!(status["model"], "fixture-model");
    assert_eq!(status["model_source"], "codex-event");
    assert_eq!(status["summary_kind"], "diagnostic");
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .lines()
            .last()
            .unwrap()
            .starts_with("multi-ai: status=boundary_violation exit=1 run=")
    );
    let out = feedback(
        &f,
        &status,
        &[
            "--outcome",
            "merged-after-rework",
            "--rework-items",
            "2",
            "--rework-rounds",
            "1",
            "--defect-tags",
            "criterion-duplicated,id-collision",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let first = score(&f);
    let cell = &first["cells"][0];
    assert_eq!(cell["failure_rate"], 0.0);
    assert_eq!(cell["attributed_boundary_count"], 1);
    assert_eq!(cell["merge_rate"], 1.0);
    assert_eq!(cell["mean_rework_items"], 2.0);
    assert_eq!(cell["mean_rework_rounds"], 1.0);
    assert!(cell.get("precision").is_none());
    // Corrections replace supplied fields only, without duplicating the report sample.
    assert!(
        feedback(
            &f,
            &status,
            &["--rework-items", "3", "--defect-tags", "browser-only"]
        )
        .status
        .success()
    );
    let second = score(&f);
    assert_eq!(second["cells"][0]["sample_count"], 1);
    assert_eq!(second["cells"][0]["mean_rework_items"], 3.0);
    assert_eq!(second["cells"][0]["merge_rate"], 1.0);
    assert_eq!(second["cells"][0]["defect_tags"], json!({"browser-only":1}));
    fs::write(
        Path::new(status["artifacts"].as_str().unwrap()).join("report.md"),
        "corrupt",
    )
    .unwrap();
    assert!(
        !feedback(&f, &status, &["--outcome", "merged"])
            .status
            .success()
    );
}
#[test]
fn inside_edits_and_registered_worktree_ignored_and_git_index_changes_are_warnings() {
    let f = Fixture::new();
    let peer = f.path("registered");
    f.git(&["worktree", "add", "--detach", peer.to_str().unwrap()]);
    fs::write(peer.join(".gitignore"), "cache/\n").unwrap();
    f.fake(r#"printf changed > tracked.txt
mkdir -p "$FAKE_ROOT/registered/cache"
i=0
while [ "$i" -lt 27 ]; do printf ignored > "$FAKE_ROOT/registered/cache/$i"; i=$((i+1)); done
printf changed > "$FAKE_ROOT/registered/tracked.txt"
touch .git/index
printf '%s\n' '{"type":"thread.started","thread_id":"fixture"}' '{"type":"item.completed","item":{"type":"file_change","status":"completed","changes":[{"path":"tracked.txt","kind":"update"}]}}' '{"type":"turn.completed"}'
printf 'implementation finished' > "$out"
"#);
    let out = f.run(&[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let status = f.status();
    assert_eq!(status["status"], "ok");
    assert_eq!(status["complete"], true);
    assert_eq!(status["summary_kind"], "final");
    assert_eq!(status["attribution"], "available");
    assert_eq!(status["boundary_warnings"]["git_ignored"]["count"], 27);
    assert_eq!(
        status["boundary_warnings"]["git_ignored"]["samples"]
            .as_array()
            .unwrap()
            .len(),
        20
    );
    assert!(
        status["boundary_warnings"]["git_internal"]["count"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(
        status["boundary_warnings"]["sibling_worktree_tracked"]["count"],
        1
    );
    let details: Value =
        serde_json::from_slice(&fs::read(status["boundary_details"].as_str().unwrap()).unwrap())
            .unwrap();
    assert_eq!(
        details["buckets"]["git_ignored"].as_array().unwrap().len(),
        27
    );
    assert!(status["boundary_after"]["elapsed_ms"].is_number());
    assert!(status["model"].is_null());
    assert_eq!(status["model_source"], "cli-default-unknown");
    assert!(
        feedback(&f, &status, &["--outcome", "merged", "--rework-items", "0"])
            .status
            .success()
    );
    let scores = score(&f);
    assert_eq!(scores["cells"][0]["failure_rate"], 0.0);
    assert_eq!(scores["cells"][0]["boundary_warning_count"], 1);
    assert_eq!(scores["cells"][0]["merge_rate"], 1.0);
    // Config model resolution is shared by display and execution.
    fs::create_dir_all(f.path("home/.codex")).unwrap();
    fs::write(
        f.path("home/.codex/config.toml"),
        "model = 'fixture-config-model'\n",
    )
    .unwrap();
    let routes = f
        .cmd()
        .env("PATH", format!("{}:/usr/bin:/bin", f.path("bin").display()))
        .args(["routes", "--json"])
        .output()
        .unwrap();
    let routes: Value = serde_json::from_slice(&routes.stdout).unwrap();
    let codex = routes
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "codex-cli")
        .unwrap();
    assert_eq!(codex["model"], "fixture-config-model");
    let out = f.run(&["--allow-dirty"]);
    assert!(out.status.success());
    assert_eq!(f.status()["model"], codex["model"]);
}
#[test]
fn explicit_watch_is_warning_without_events_and_batch_implementation_feedback_works() {
    let f = Fixture::new();
    f.fake("printf changed > \"$FAKE_ROOT/sibling/nested/keep\"\nprintf 'implementation finished' > \"$out\"\n");
    assert!(
        f.run(&["--watch", f.path("sibling").to_str().unwrap()])
            .status
            .success()
    );
    let status = f.status();
    assert_eq!(status["attribution"], "unavailable");
    assert_eq!(
        status["boundary_warnings"]["sibling_worktree_tracked"]["count"],
        1
    );
    fs::write(f.path("verdicts.tsv"), "id\taccepted\tfalse_positive\tuncertain\tevidence\toutcome\trework_items\trework_rounds\tdefect_tags\nsummary\t\t\t\tsynthetic checks\tmerged\t0\t0\tsort-tiebreak\n").unwrap();
    let out = f
        .cmd()
        .args(["feedback", "--batch"])
        .arg(f.temp.path())
        .args(["--from"])
        .arg(f.path("verdicts.tsv"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        score(&f)["cells"][0]["defect_tags"],
        json!({"sort-tiebreak":1})
    );
}

#[test]
fn legacy_offline_reclassification_registers_report_and_preserves_evidence() {
    let f = Fixture::new();
    f.fake("printf changed > \"$HOME/.claude/skills/custom/keep\"\nprintf 'implementation finished' > \"$out\"\n");
    assert!(!f.run(&[]).status.success());
    let mut status = f.status();
    let run = status["run_id"].as_str().unwrap().to_owned();
    let attempt = status["attempt_id"].as_str().unwrap().to_owned();
    let artifacts = PathBuf::from(status["artifacts"].as_str().unwrap());
    for field in [
        "attribution",
        "summary_kind",
        "attributed_boundary_count",
        "boundary_warnings",
        "boundary_buckets",
        "boundary_details",
    ] {
        status.as_object_mut().unwrap().remove(field);
    }
    status["task_type"] = Value::Null;
    let write_json =
        |path: &Path, v: &Value| fs::write(path, serde_json::to_vec_pretty(v).unwrap()).unwrap();
    write_json(&f.path("summary.md.status.json"), &status);
    write_json(&artifacts.join("metadata.json"), &status);
    let manifest_path = f.path(&format!("state/runs/{run}/manifest.json"));
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["reports"] = json!([]);
    manifest["current_status"] = status.clone();
    manifest["task_type"] = Value::Null;
    write_json(&manifest_path, &manifest);
    assert!(
        !feedback(&f, &status, &["--outcome", "merged"])
            .status
            .success()
    );
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/reclassify-implement.py");
    let repair = |apply: bool| {
        let mut cmd = Command::new("/usr/bin/python3");
        cmd.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", f.path("home"))
            .env("TMPDIR", f.temp.path())
            .arg(&script)
            .arg("--state-dir")
            .arg(f.path("state"))
            .arg("--status")
            .arg(f.path("summary.md.status.json"));
        if apply {
            cmd.arg("--apply");
        }
        cmd.output().unwrap()
    };
    let original = fs::read(&manifest_path).unwrap();
    assert!(repair(false).status.success());
    assert_eq!(fs::read(&manifest_path).unwrap(), original);
    // Reject execution failure and damaged summaries, rather than guessing success.
    status["response"]["exitCode"] = json!(1);
    write_json(&f.path("summary.md.status.json"), &status);
    assert!(!repair(true).status.success());
    assert_eq!(fs::read(&manifest_path).unwrap(), original);
    status["response"]["exitCode"] = json!(0);
    write_json(&f.path("summary.md.status.json"), &status);
    let summary = fs::read(f.path("summary.md")).unwrap();
    fs::write(f.path("summary.md"), "truncated").unwrap();
    assert!(!repair(true).status.success());
    fs::write(f.path("summary.md"), &summary).unwrap();
    let out = repair(true);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let updated = f.status();
    assert_eq!(updated["status"], "ok");
    assert_eq!(updated["task_type"], "implement");
    assert_eq!(updated["summary_kind"], "diagnostic");
    assert_eq!(updated["run_id"], run);
    assert_eq!(updated["attempt_id"], attempt);
    assert_eq!(fs::read(f.path("summary.md")).unwrap(), summary);
    assert!(
        artifacts
            .join("legacy-before-reclassification.json")
            .exists()
    );
    assert!(
        feedback(
            &f,
            &updated,
            &[
                "--outcome",
                "merged-after-rework",
                "--rework-items",
                "1",
                "--rework-rounds",
                "0"
            ]
        )
        .status
        .success()
    );
    let report = score(&f);
    assert_eq!(report["cells"][0]["failure_rate"], 0.0);
    assert_eq!(report["cells"][0]["merge_rate"], 1.0);
    assert_eq!(report["cells"][0]["boundary_warning_count"], 1);
    assert!(repair(true).status.success());
}

#[test]
fn nested_worktree_changes_warn_and_linked_claude_configuration_remains_protected() {
    let f = Fixture::new();
    let peer = f.path("registered");
    f.git(&["worktree", "add", "--detach", peer.to_str().unwrap()]);
    let nested = peer.join(".claude/worktrees/nested");
    fs::create_dir_all(&nested).unwrap();
    f.git_at(&nested, &["init", "--quiet", "--template="]);
    fs::write(nested.join("tracked"), "original").unwrap();
    f.git_at(&nested, &["add", "tracked"]);
    f.fake("printf changed > \"$FAKE_ROOT/registered/.claude/worktrees/nested/tracked\"\nprintf 'implementation finished' > \"$out\"\n");
    assert!(f.run(&[]).status.success());
    assert_eq!(
        f.status()["boundary_warnings"]["nested_worktree"]["count"],
        1
    );
    fs::rename(f.path("home/.claude/skills"), f.path("linked-skills")).unwrap();
    std::os::unix::fs::symlink(f.path("linked-skills"), f.path("home/.claude/skills")).unwrap();
    f.fake("printf changed > \"$HOME/.claude/skills/custom/keep\"\nprintf 'implementation finished' > \"$out\"\n");
    let out = f.run(&[]);
    let status = f.assert_boundary(&out, &["linked-skills/custom/keep"]);
    assert_eq!(status["boundary_buckets"]["claude_config"]["count"], 1);
}

// Minimal replay: no repository documents or real credentials are retained.
fn replay_failure(stdout: &str, stderr: &str, expected: &str) -> Fixture {
    let f = Fixture::new();
    fs::write(f.path("stdout.fixture"), stdout).unwrap();
    fs::write(f.path("stderr.fixture"), stderr).unwrap();
    f.fake("cat \"$FAKE_ROOT/stdout.fixture\"\ncat \"$FAKE_ROOT/stderr.fixture\" >&2\nexit 1\n");
    f.preserve_summary();
    let output = f.run(&[]);
    assert!(!output.status.success());
    assert_eq!(f.status()["execution_status"], expected);
    assert_eq!(
        fs::read_to_string(f.path("summary.md")).unwrap(),
        "previous complete summary"
    );
    f
}

#[test]
fn replay_capacity_ignores_forbidden_tool_output_without_cooldown() {
    let f = replay_failure(
        include_str!("fixtures/codex-capacity-stdout.jsonl"),
        include_str!("fixtures/codex-capacity-stderr.txt"),
        "capacity",
    );
    assert!(!f.path("state/review/state.json").exists());
    let output = f.cmd().args(["stats", "--json"]).output().unwrap();
    assert!(output.status.success());
    let stats: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stats["routes"][0]["failures"]["capacity"], 1);
}

#[test]
fn replay_plain_error_ignores_unauthorized_tool_output() {
    let f = replay_failure(
        "{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\",\"aggregated_output\":\"401 Unauthorized\"}}\n{\"type\":\"error\",\"message\":\"ordinary failure\"}\n",
        "",
        "error",
    );
    assert!(!f.path("state/review/state.json").exists());
}

#[test]
fn replay_auth_error_still_auth_with_short_first_cooldown() {
    let f = replay_failure(
        "{\"type\":\"error\",\"message\":\"401 Unauthorized\"}\n",
        "",
        "auth",
    );
    let state: Value =
        serde_json::from_slice(&fs::read(f.path("state/review/state.json")).unwrap()).unwrap();
    let remaining = state["pools"]["openai"]["until"].as_f64().unwrap()
        - SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
    assert!((590.0..=600.0).contains(&remaining), "{remaining}");
}

#[test]
fn claude_shell_implementer_gets_restricted_sandbox_and_git_deny() {
    let f = Fixture::new();
    let claude = f.path("bin/claude");
    fs::write(
        &claude,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$FAKE_ROOT/claude-argv\"\nprintf '%s' \"$ANTHROPIC_BASE_URL\" > \"$FAKE_ROOT/claude-url\"\nprintf changed > tracked.txt\nprintf 'shell implementation complete'\n",
    )
    .unwrap();
    fs::set_permissions(&claude, fs::Permissions::from_mode(0o755)).unwrap();
    let shell = json!({"id":"shell-impl","legacy_id":"shell-impl","pool":"deepseek-api","family":"deepseek","label":"shell","agentic":true,"cost":"metered","rank":1,"argv":["claude","-p","{prompt}","--tools","Read"],"extract":"stdout","default_timeout":60,"anthropic_provider":"deepseek"});
    let plain = json!({"id":"plain-cli","legacy_id":"plain-cli","pool":"x","family":"x","label":"plain","agentic":true,"cost":"sub","rank":2,"argv":["claude","-p","{prompt}"],"extract":"stdout","default_timeout":60});
    fs::write(
        f.path("config/routes.json"),
        serde_json::to_vec(&json!([shell, plain])).unwrap(),
    )
    .unwrap();
    let run = |target: &str, extra: &[&str]| {
        f.cmd()
            .env("DEEPSEEK_API_KEY", "fixture-deepseek-key")
            .args(["delegate", "implement", target, "--cwd"])
            .arg(&f.cwd)
            .arg("--prompt-file")
            .arg(f.path("brief.md"))
            .arg("--out")
            .arg(f.path("summary.md"))
            .arg("--path-prepend")
            .arg(f.path("bin"))
            .args(["--timeout", "20"])
            .args(extra)
            .output()
            .unwrap()
    };
    let out = run("shell-impl", &[]);
    let status = f.status();
    assert!(out.status.success(), "{status}");
    assert_eq!(status["route"], "shell-impl");
    assert_eq!(status["changed_files"], json!(["tracked.txt"]));
    let argv = fs::read_to_string(f.temp.path().join("claude-argv")).unwrap();
    let argv: Vec<&str> = argv.lines().collect();
    for flag in ["--restricted", "--strict-mcp-config", "acceptEdits"] {
        assert!(argv.contains(&flag), "{argv:?}");
    }
    // The configured read-only tool list is replaced by the controlled implementer one.
    assert!(argv.contains(&"Read,Grep,Glob,Edit,Write,Bash"), "{argv:?}");
    let settings: Value =
        serde_json::from_str(argv[argv.iter().position(|a| *a == "--settings").unwrap() + 1])
            .unwrap();
    assert_eq!(settings["sandbox"]["enabled"], true);
    assert_eq!(settings["sandbox"]["allowUnsandboxedCommands"], false);
    assert_eq!(
        settings["permissions"]["deny"],
        json!([format!(
            "Edit(/{}/.git/**)",
            fs::canonicalize(&f.cwd).unwrap().display()
        )])
    );
    assert_eq!(
        fs::read_to_string(f.temp.path().join("claude-url")).unwrap(),
        "https://api.deepseek.com/anthropic"
    );
    f.git(&["checkout", "--", "tracked.txt"]);
    let refused = run("plain-cli", &[]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("anthropic_provider"));
    let network = run("shell-impl", &["--network"]);
    assert!(!network.status.success());
    assert!(String::from_utf8_lossy(&network.stderr).contains("--network is only supported"));
}

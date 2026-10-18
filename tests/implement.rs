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
        f.seed("home/.claude/nested/keep", "before");
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
        assert_eq!(
            fs::read_to_string(self.path("summary.md")).unwrap(),
            "previous complete summary"
        );
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
    let out = f.run(&[]);
    assert!(!out.status.success());
    let status = f.status();
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
    let f = Fixture::new();
    let out = f.run(&["--timeout", "10"]);
    let status = f.status();
    assert!(out.status.success(), "{status}");
    assert_eq!(status["status"], "ok");
    assert_eq!(
        status["changed_files"],
        json!(["changed.txt", "tracked.txt"])
    );
    assert_eq!(status["git_before"]["head"], status["git_after"]["head"]);
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
        "network_access",
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
}

#[test]
fn public_postcheck_reports_recursive_rewrites_creation_and_deletion_without_rollback() {
    let f = Fixture::new();
    f.preserve_summary();
    f.fake(
        r#"printf mutate > outside-link/nested/keep
printf mutate > "$HOME/.claude/nested/keep"
printf created > "$HOME/.claude/nested/created"
rm "$HOME/.agents/nested/remove"
printf 'implementation finished' > "$out"
"#,
    );
    let out = f.run(&[]);
    f.assert_boundary(
        &out,
        &[
            "sibling/nested/keep",
            "home/.claude/nested/keep",
            "home/.claude/nested/created",
            "home/.agents/nested/remove",
        ],
    );
    assert_eq!(
        fs::read_to_string(f.path("sibling/nested/keep")).unwrap(),
        "mutate"
    );
    assert_eq!(
        fs::read_to_string(f.path("home/.claude/nested/keep")).unwrap(),
        "mutate"
    );
    assert!(f.path("home/.claude/nested/created").is_file());
    assert!(!f.path("home/.agents/nested/remove").exists());
}

#[test]
fn public_postcheck_detects_sensitive_directories_created_after_start() {
    let f = Fixture::new();
    f.preserve_summary();
    fs::remove_dir_all(f.path("home/.claude")).unwrap();
    fs::remove_dir_all(f.path("home/.agents")).unwrap();
    f.fake(
        r#"mkdir -p "$HOME/.claude/nested" "$HOME/.agents/nested"
printf created > "$HOME/.claude/nested/created"
printf created > "$HOME/.agents/nested/created"
printf 'implementation finished' > "$out"
"#,
    );
    let out = f.run(&[]);
    f.assert_boundary(
        &out,
        &["home/.claude/nested/created", "home/.agents/nested/created"],
    );
    assert!(f.path("home/.claude/nested/created").is_file());
    assert!(f.path("home/.agents/nested/created").is_file());
}

#[test]
fn public_postcheck_runs_after_codex_failure_and_retains_response_reason() {
    let f = Fixture::new();
    f.preserve_summary();
    f.fake(
        r#"printf mutate > "$FAKE_ROOT/sibling/nested/keep"
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
        r#"printf mutate > "$FAKE_ROOT/sibling/nested/keep"
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
        r#"printf mutate > "$FAKE_ROOT/sibling/nested/keep"
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
fn public_postcheck_discovers_new_sibling_worktree_after_start() {
    let f = Fixture::new();
    f.preserve_summary();
    f.fake(
        r#"/usr/bin/git init --quiet --template= "$FAKE_ROOT/new-worktree"
printf created > "$FAKE_ROOT/new-worktree/created"
printf 'implementation finished' > "$out"
"#,
    );
    let out = f.run(&[]);
    f.assert_boundary(&out, &["new-worktree/created"]);
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

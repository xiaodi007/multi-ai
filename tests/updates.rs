//! Fake vendor CLIs on PATH; no network and no user configuration.
use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn updates_flag_newer_vendor_versions_and_delegation_revalidation() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let fake = |name: &str, body: &str| {
        let path = bin.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    };
    fake("claude", "echo '2.1.310 (Claude Code)'");
    fake(
        "npm",
        "case \"$2\" in @anthropic-ai/claude-code) echo 2.1.310;; *) echo 0.200.0;; esac",
    );
    fake("codex", "echo 'codex-cli 0.164.0'");
    fake(
        "grok",
        "if [ \"$1\" = update ]; then echo '{\"latestVersion\":\"1.0.99\"}'; else echo 'grok 1.0.67 (2fe064390d07) [stable]'; fi",
    );
    let out = Command::new(env!("CARGO_BIN_EXE_multi-ai"))
        .env_clear()
        .env("HOME", temp.path())
        .env("MULTI_AI_CONFIG_DIR", temp.path().join("config"))
        .env("MULTI_AI_STATE_DIR", temp.path().join("state"))
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .current_dir(temp.path())
        .args(["doctor", "--updates", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let row = |name: &str| {
        v["vendors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["vendor"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(row("claude")["update_available"], false);
    assert_eq!(row("codex")["latest"], "0.200.0");
    assert_eq!(row("codex")["upgrade_needs_revalidation"], false);
    assert_eq!(row("grok")["update_available"], true);
    assert_eq!(row("grok")["upgrade_needs_revalidation"], true);
    assert_eq!(row("kimi")["installed"], Value::Null);
}

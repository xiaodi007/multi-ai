use serde_json::Value;
use std::{path::Path, process::Command};

#[test]
fn disagreements_come_first_and_locations_align_by_suffix_and_nearby_lines() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/consensus");
    // consensus reads local sycophancy rates; never let it see a real state directory.
    let state = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_multi-ai"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", state.path())
        .env("MULTI_AI_STATE_DIR", state.path())
        .arg("consensus")
        .args(["grok.md", "kimi.md", "codex.md"].map(|f| fixtures.join(f)))
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let rows = report["rows"].as_array().unwrap();
    let kinds: Vec<_> = rows.iter().map(|r| r["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["结论相反", "仅一家", "全部提到"]);
    assert_eq!(rows[0]["verdicts"]["grok"], true);
    assert_eq!(rows[0]["verdicts"]["kimi"], false);
    assert_eq!(rows[2]["seats"].as_array().unwrap().len(), 3);
    let unaligned = report["unaligned"].as_array().unwrap();
    assert!(
        unaligned
            .iter()
            .any(|u| u["seat"] == "codex" && u["summary"].as_str().unwrap().contains("隐患"))
    );
    assert!(
        !unaligned
            .iter()
            .any(|u| u["summary"].as_str().unwrap().contains("结尾说明"))
    );
    let one = Command::new(env!("CARGO_BIN_EXE_multi-ai"))
        .env_clear()
        .env("HOME", state.path())
        .env("MULTI_AI_STATE_DIR", state.path())
        .arg("consensus")
        .arg(fixtures.join("grok.md"))
        .output()
        .unwrap();
    assert!(!one.status.success());
}

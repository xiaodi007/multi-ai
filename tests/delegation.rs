use multi_ai::{packet, runtime};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    process::Command,
};

struct Fixture {
    url: String,
    calls: Arc<AtomicUsize>,
    answer: Arc<Mutex<String>>,
    status: Arc<AtomicUsize>,
    wires: Arc<Mutex<Vec<Value>>>,
    hold: Arc<AtomicBool>,
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn fixture() -> Fixture {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let answer = Arc::new(Mutex::new(String::new()));
    let status = Arc::new(AtomicUsize::new(200));
    let wires = Arc::new(Mutex::new(Vec::new()));
    let hold = Arc::new(AtomicBool::new(false));
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let (cc, aa, ss, ww, hh, bb, rr) = (
        calls.clone(),
        answer.clone(),
        status.clone(),
        wires.clone(),
        hold.clone(),
        started.clone(),
        release.clone(),
    );
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let (cc, aa, ss, ww, hh, bb, rr) = (
                cc.clone(),
                aa.clone(),
                ss.clone(),
                ww.clone(),
                hh.clone(),
                bb.clone(),
                rr.clone(),
            );
            tokio::spawn(async move {
                let mut data = Vec::new();
                let mut chunk = [0; 4096];
                let end;
                loop {
                    let n = stream.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        return;
                    };
                    data.extend_from_slice(&chunk[..n]);
                    if let Some(i) = data.windows(4).position(|b| b == b"\r\n\r\n") {
                        end = i + 4;
                        break;
                    }
                }
                let size = String::from_utf8_lossy(&data[..end])
                    .lines()
                    .find_map(|l| {
                        l.to_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|s| s.trim().parse::<usize>().ok())
                    })
                    .unwrap();
                while data.len() < end + size {
                    let n = stream.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        return;
                    };
                    data.extend_from_slice(&chunk[..n]);
                }
                ww.lock()
                    .unwrap()
                    .push(serde_json::from_slice(&data[end..end + size]).unwrap());
                cc.fetch_add(1, Ordering::SeqCst);
                bb.notify_one();
                if hh.load(Ordering::SeqCst) {
                    rr.notified().await;
                }
                let status = ss.load(Ordering::SeqCst);
                let payload = if status == 200 {
                    format!(
                        "data: {}\n\ndata: [DONE]\n\n",
                        json!({"model":"fixture-model","choices":[{"delta":{"content":aa.lock().unwrap().clone()},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":20}})
                    )
                } else {
                    "unauthorized".to_owned()
                };
                let headers = format!(
                    "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                let _ = stream.write_all(headers.as_bytes()).await;
                let _ = stream.write_all(payload.as_bytes()).await;
            });
        }
    });
    Fixture {
        url,
        calls,
        answer,
        status,
        wires,
        hold,
        started,
        release,
        task,
    }
}
fn setup(f: &Fixture) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir(root.join("config")).unwrap();
    fs::create_dir(root.join("source")).unwrap();
    fs::create_dir(root.join("empty-path")).unwrap();
    fs::write(
        root.join("source/entry.rs"),
        "const RETRY_COUNT: u32 = 7;\n// fixture source\n",
    )
    .unwrap();
    fs::write(
        root.join("task.txt"),
        "Extract retry count and cite exact source; record coverage limits.",
    )
    .unwrap();
    let mut profiles: Value =
        serde_json::from_str(include_str!("../config/profiles.json")).unwrap();
    profiles["providers"]["glm"]["baseUrl"] = json!(f.url);
    profiles["providers"]["deepseek"]["baseUrl"] = json!(f.url);
    fs::write(
        root.join("config/profiles.json"),
        serde_json::to_vec(&profiles).unwrap(),
    )
    .unwrap();
    let packet = packet::create(
        &root.join("source"),
        &["entry.rs".into()],
        &[],
        &root.join("packet"),
    )
    .unwrap();
    *f.answer.lock().unwrap()=serde_json::to_string(&json!({"schema_version":1,"findings":[{"importance":"info","conclusion":"The retry count is 7.","evidence":[{"path":"entry.rs","start_line":1,"end_line":1,"sha256":packet.manifest.files[0].sha256,"quote":"RETRY_COUNT: u32 = 7"}]}],"checked_scope":["entry.rs"],"uncovered_scope":[],"uncertainties":[]})).unwrap();
    tmp
}
fn cli(root: &Path, target: &str, out: &str, extra: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_multi-ai"));
    cmd.args(["delegate", "extract", target, "--packet"])
        .arg(root.join("packet"))
        .arg("--prompt-file")
        .arg(root.join("task.txt"))
        .arg("--outdir")
        .arg(root.join(out))
        .args(["--timeout", "8", "--json"])
        .args(extra)
        .current_dir(root)
        .env_clear()
        .env("HOME", root)
        .env("PATH", root.join("empty-path"))
        .env("MULTI_AI_CONFIG_DIR", root.join("config"))
        .env("MULTI_AI_STATE_DIR", root.join("state"))
        .env("ZHIPU_API_KEY", "fixture-key")
        .env("DEEPSEEK_API_KEY", "fixture-key");
    cmd
}
async fn result(root: &Path, target: &str, out: &str, extra: &[&str]) -> (bool, Value) {
    let output = cli(root, target, out, extra).output().await.unwrap();
    let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.success(), value)
}
fn pointer(root: &Path, out: &str) -> Value {
    serde_json::from_slice(&fs::read(root.join(out).join("latest.json")).unwrap()).unwrap()
}
#[tokio::test]
async fn native_api_publication_resume_and_feedback_are_bound_to_full_report() {
    let f = fixture().await;
    let tmp = setup(&f);
    let root = tmp.path();
    let (ok, first) = result(root, "glm-flash", "out", &[]).await;
    assert!(ok, "{first}");
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    assert_eq!(first["metrics"]["usage"]["completion_tokens"], 20);
    assert!(first["metrics"]["host_usage"].is_null());
    assert_eq!(first["metrics"]["pool"], first["pool"]);
    assert_eq!(first["metrics"]["run_id"], first["run_id"]);
    assert_eq!(first["metrics"]["material_chars"], 46);
    assert!(
        first["metrics"]["material_to_handoff_char_ratio"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    let p = pointer(root, "out");
    for row in p["artifacts"].as_object().unwrap().values() {
        let path = root.join("out").join(row["path"].as_str().unwrap());
        assert_eq!(runtime::sha256(&fs::read(path).unwrap()), row["sha256"]);
    }
    let (_, second) = result(root, "glm-flash", "out", &["--resume"]).await;
    assert_eq!(second["cached"], true);
    assert_eq!(second["metrics"]["invocations"], 0);
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    assert_ne!(first["run_id"], second["run_id"]);
    let wire = f.wires.lock().unwrap()[0].clone();
    assert!(wire["messages"].to_string().contains("RETRY_COUNT"));
    assert!(
        wire["messages"]
            .to_string()
            .contains("1 | const RETRY_COUNT: u32 = 7;")
    );
    assert!(
        wire["messages"]
            .to_string()
            .contains("2 | // fixture source")
    );
    assert!(!wire.to_string().contains(root.to_str().unwrap()));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_multi-ai"));
    cmd.args([
        "feedback",
        "--run",
        first["run_id"].as_str().unwrap(),
        "--report",
        first["attempt_id"].as_str().unwrap(),
        "--accepted",
        "1",
        "--evidence",
        "Checked fixture source line 1",
    ])
    .env_clear()
    .env("HOME", root)
    .env("MULTI_AI_STATE_DIR", root.join("state"));
    let feedback = cmd.output().await.unwrap();
    assert!(feedback.status.success());
    let v: Value = serde_json::from_slice(&feedback.stdout).unwrap();
    assert_eq!(v["source"], "agent");
    assert_eq!(v["report_sha256"], first["report_sha256"]);
    fs::write(first["artifacts"]["report"].as_str().unwrap(), "tampered").unwrap();
    assert!(!cmd.output().await.unwrap().status.success());
}
#[tokio::test]
async fn malformed_and_invalid_evidence_preserve_last_generation_without_fallback() {
    let f = fixture().await;
    let tmp = setup(&f);
    let root = tmp.path();
    assert!(result(root, "glm-flash", "out", &[]).await.0);
    let prior = pointer(root, "out");
    *f.answer.lock().unwrap() = "not structured".into();
    let (ok, v) = result(root, "glm-flash", "out", &["--fallback", "deepseek-flash"]).await;
    assert!(!ok, "{v}");
    assert_eq!(f.calls.load(Ordering::SeqCst), 2);
    assert_eq!(pointer(root, "out"), prior);
    let mut bad: Value = json!({"schema_version":1,"findings":[{"importance":"high","conclusion":"fake","evidence":[{"path":"entry.rs","start_line":99,"end_line":99,"sha256":"bad"}]}],"checked_scope":["entry.rs"],"uncovered_scope":[],"uncertainties":[]});
    *f.answer.lock().unwrap() = bad.to_string();
    assert!(!result(root, "glm-flash", "out", &[]).await.0);
    assert_eq!(pointer(root, "out"), prior);
    bad["checked_scope"] = json!(["../outside"]);
    *f.answer.lock().unwrap() = bad.to_string();
    assert!(!result(root, "glm-flash", "out", &[]).await.0);
}
#[tokio::test]
async fn missing_cli_uses_only_explicit_fallback_and_auth_is_not_retried() {
    let f = fixture().await;
    let tmp = setup(&f);
    let root = tmp.path();
    let (ok, v) = result(
        root,
        "kimi-cli-default",
        "out",
        &["--fallback", "glm-flash"],
    )
    .await;
    assert!(ok, "{v}");
    assert_eq!(v["target"], "glm-flash");
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    let (ok, _) = result(root, "grok-cli", "other", &[]).await;
    assert!(!ok);
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    f.status.store(401, Ordering::SeqCst);
    assert!(!result(root, "deepseek-flash", "auth", &[]).await.0);
    assert_eq!(f.calls.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn overlapping_outputs_make_zero_requests_and_unrelated_outputs_can_parallel() {
    let f = fixture().await;
    let tmp = setup(&f);
    let root = tmp.path();
    f.hold.store(true, Ordering::SeqCst);
    let mut command = cli(root, "glm-flash", "out", &[]);
    let first = tokio::spawn(async move { command.output().await.unwrap() });
    f.started.notified().await;
    for dir in ["out", "out/nested"] {
        let output = cli(root, "glm-flash", dir, &[]).output().await.unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("output_busy"));
    }
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    f.hold.store(false, Ordering::SeqCst);
    f.release.notify_one();
    assert!(first.await.unwrap().status.success());
    let (a, b) = tokio::join!(
        result(root, "glm-flash", "a", &[]),
        result(root, "glm-flash", "b", &[])
    );
    assert!(a.0 && b.0);
    assert_eq!(f.calls.load(Ordering::SeqCst), 3);
}
#[tokio::test]
async fn corrupt_cache_changed_task_and_packet_damage_are_not_reused() {
    let f = fixture().await;
    let tmp = setup(&f);
    let root = tmp.path();
    assert!(result(root, "glm-flash", "out", &[]).await.0);
    let cache = fs::read_dir(root.join("out/.cache"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(&cache, "damaged").unwrap();
    assert!(result(root, "glm-flash", "out", &["--resume"]).await.0);
    assert_eq!(f.calls.load(Ordering::SeqCst), 2);
    fs::write(
        root.join("task.txt"),
        "Same facts; examine potential missing retries too.",
    )
    .unwrap();
    assert!(result(root, "glm-flash", "out", &["--resume"]).await.0);
    assert_eq!(f.calls.load(Ordering::SeqCst), 3);
    let manifest: Value =
        serde_json::from_slice(&fs::read(root.join("packet/manifest.json")).unwrap()).unwrap();
    fs::write(
        root.join("packet")
            .join(manifest["files"][0]["blob"].as_str().unwrap()),
        "tampered",
    )
    .unwrap();
    let output = cli(root, "glm-flash", "out", &["--resume"])
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(f.calls.load(Ordering::SeqCst), 3);
}
#[tokio::test]
async fn oversized_api_packet_and_credentials_fail_before_sending() {
    let f = fixture().await;
    let tmp = setup(&f);
    let root = tmp.path();
    fs::write(
        root.join("task.txt"),
        "credential sk-abcdefghijklmnopABCDEFGHIJKLMNOP",
    )
    .unwrap();
    assert!(
        !cli(root, "glm-flash", "out", &[])
            .output()
            .await
            .unwrap()
            .status
            .success()
    );
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    fs::write(root.join("task.txt"), "Extract authorized fixture.").unwrap();
    let source = root.join("large");
    fs::create_dir(&source).unwrap();
    for name in ["a.txt", "b.txt"] {
        fs::write(source.join(name), "a".repeat(300 * 1024)).unwrap();
    }
    let large = packet::create(&source, &[], &[".".into()], &root.join("large-packet")).unwrap();
    let mut cmd = cli(root, "glm-flash", "out", &[]);
    cmd.arg("--packet").arg(large.dir);
    assert!(!cmd.output().await.unwrap().status.success());
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn handoff_is_bounded_full_report_retains_all_findings() {
    use multi_ai::delegate::{Finding, StructuredResult, render_handoff, render_report};
    let result = StructuredResult {
        schema_version: 1,
        findings: (0..20)
            .map(|_| Finding {
                importance: "high".into(),
                conclusion: "结论".repeat(2000),
                evidence: vec![],
            })
            .collect(),
        checked_scope: vec!["source".into()],
        uncovered_scope: vec!["outside scope".into()],
        uncertainties: vec!["test not run".into()],
    };
    let handoff = render_handoff(&result, &PathBuf::from("/tmp/report.md"));
    assert!(handoff.chars().count() <= 3500);
    assert!(handoff.contains("剩余 15"));
    assert!(handoff.contains("test not run"));
    assert!(
        render_report(&result, "challenge", "fixture")
            .chars()
            .count()
            > 80000
    );
}

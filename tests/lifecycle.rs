use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    process::Command,
    sync::Notify,
};
struct Fixture {
    url: String,
    count: Arc<AtomicUsize>,
    started: Arc<Notify>,
    release: Arc<Notify>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.release.notify_waiters();
        self.server.abort();
    }
}
async fn fixture(hold_first: bool) -> Fixture {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let counts = count.clone();
    let begins = started.clone();
    let released = release.clone();
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let counts = counts.clone();
            let begins = begins.clone();
            let released = released.clone();
            tokio::spawn(async move {
                let mut data = Vec::new();
                let mut buf = [0; 2048];
                let end;
                loop {
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    data.extend_from_slice(&buf[..n]);
                    if let Some(index) = data.windows(4).position(|s| s == b"\r\n\r\n") {
                        end = index + 4;
                        break;
                    }
                }
                let size = String::from_utf8_lossy(&data[..end])
                    .lines()
                    .find_map(|line| {
                        line.to_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while data.len() < end + size {
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    data.extend_from_slice(&buf[..n]);
                }
                let number = counts.fetch_add(1, Ordering::SeqCst) + 1;
                if number == 1 {
                    begins.notify_one();
                    if hold_first {
                        released.notified().await;
                    }
                }
                let payload = format!(
                    "data: {}\n\ndata: [DONE]\n\n",
                    json!({"model":"fixture-observed","choices":[{"delta":{"content":format!("report-{number}")},"finish_reason":"stop"}],"usage":{"completion_tokens":3}})
                );
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                let _ = socket.write_all(header.as_bytes()).await;
                let _ = socket.write_all(payload.as_bytes()).await;
            });
        }
    });
    Fixture {
        url,
        count,
        started,
        release,
        server,
    }
}
fn setup(f: &Fixture) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let cfg = root.path().join("config");
    std::fs::create_dir(&cfg).unwrap();
    std::fs::create_dir(root.path().join("empty-path")).unwrap();
    let mut profiles: Value =
        serde_json::from_str(include_str!("../config/profiles.json")).unwrap();
    profiles["providers"]["glm"]["baseUrl"] = json!(f.url);
    std::fs::write(
        cfg.join("profiles.json"),
        serde_json::to_vec(&profiles).unwrap(),
    )
    .unwrap();
    root
}
fn cli(root: &Path, tokens: &[String]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_multi-ai"));
    command
        .args(tokens)
        .current_dir(root)
        .env_clear()
        .env("HOME", root)
        .env("PATH", root.join("empty-path"))
        .env("MULTI_AI_CONFIG_DIR", root.join("config"))
        .env("MULTI_AI_STATE_DIR", root.join("state"))
        .env("ZHIPU_API_KEY", "fixture-key");
    command
}
fn text(out: &Path) -> Vec<String> {
    vec![
        "text".into(),
        "glm".into(),
        "fixture".into(),
        "--out".into(),
        out.display().to_string(),
        "--json".into(),
        "--quiet".into(),
        "--timeout-ms".into(),
        "5000".into(),
        "--source-id".into(),
        "snapshot-1".into(),
    ]
}
fn review(dir: &Path) -> Vec<String> {
    vec![
        "review".into(),
        "fixture".into(),
        "--only".into(),
        "glm-api".into(),
        "--seats".into(),
        "1".into(),
        "--text-ready".into(),
        "--outdir".into(),
        dir.display().to_string(),
        "--timeout".into(),
        "5".into(),
        "--no-quota".into(),
    ]
}
async fn started(f: &Fixture) {
    tokio::time::timeout(Duration::from_secs(4), f.started.notified())
        .await
        .unwrap();
}
fn status(path: &Path) -> Value {
    serde_json::from_slice(
        &std::fs::read(PathBuf::from(format!("{}.status.json", path.display()))).unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn native_api_runs_without_interpreters_and_resume_does_not_count_a_request() {
    let f = fixture(false).await;
    let root = setup(&f);
    let out = root.path().join("result.md");
    let args = text(&out);
    let first = cli(root.path(), &args).output().await.unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_status = status(&out);
    let mut resumed = args.clone();
    resumed.push("--resume".into());
    let second = cli(root.path(), &resumed).output().await.unwrap();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(f.count.load(Ordering::SeqCst), 1);
    assert_eq!(status(&out)["result"]["cached"], true);
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(status(&out)["manifest"].as_str().unwrap()).unwrap())
            .unwrap();
    assert_eq!(manifest["reports"][0]["model_calls"], 0);
    assert_eq!(manifest["reports"][0]["cached"], true);
    assert_eq!(
        manifest["reports"][0]["result"]["reused_from"]["run_id"],
        first_status["run_id"]
    );
    let changed = cli(root.path(), &resumed)
        .env("GLM_MODEL", "changed-request-model")
        .output()
        .await
        .unwrap();
    assert!(
        changed.status.success(),
        "{}",
        String::from_utf8_lossy(&changed.stderr)
    );
    assert_eq!(f.count.load(Ordering::SeqCst), 2);
    let history = root.path().join("state/history");
    assert!(history.is_dir());
    let archived = std::fs::read_dir(history)
        .unwrap()
        .flat_map(|d| std::fs::read_dir(d.unwrap().path()).unwrap())
        .filter_map(|d| d.ok())
        .filter(|d| d.file_name().to_string_lossy().ends_with("-result.md"))
        .collect::<Vec<_>>();
    assert!(!archived.is_empty());
    assert!(archived.iter().any(|d| {
        std::fs::read_to_string(d.path())
            .unwrap()
            .contains("report-1")
    }));
}
#[tokio::test]
async fn same_output_and_dispatch_overlap_refuse_before_sending_or_touching_status() {
    let f = fixture(true).await;
    let root = setup(&f);
    let out = root.path().join("reports/sub/a.md");
    let first = cli(root.path(), &text(&out))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    started(&f).await;
    let before = std::fs::read(format!("{}.status.json", out.display())).unwrap();
    let second = cli(root.path(), &text(&out)).output().await.unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("output_busy"));
    let dispatch = cli(root.path(), &review(&root.path().join("reports")))
        .output()
        .await
        .unwrap();
    assert!(
        !dispatch.status.success(),
        "{}",
        String::from_utf8_lossy(&dispatch.stderr)
    );
    assert!(String::from_utf8_lossy(&dispatch.stderr).contains("output_busy"));
    assert_eq!(f.count.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read(format!("{}.status.json", out.display())).unwrap(),
        before
    );
    f.release.notify_one();
    assert!(first.wait_with_output().await.unwrap().status.success());
}
#[tokio::test]
async fn dispatch_root_blocks_text_in_reverse_order_but_unrelated_paths_parallel() {
    let f = fixture(true).await;
    let root = setup(&f);
    let dir = root.path().join("reports");
    let first = cli(root.path(), &review(&dir))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    started(&f).await;
    let before = std::fs::read(dir.join("dispatch.json")).unwrap();
    let blocked = cli(root.path(), &text(&dir.join("anything.md")))
        .output()
        .await
        .unwrap();
    assert!(!blocked.status.success());
    assert_eq!(f.count.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read(dir.join("dispatch.json")).unwrap(), before);
    let parallel = cli(root.path(), &text(&root.path().join("unrelated.md")))
        .output()
        .await
        .unwrap();
    assert!(
        parallel.status.success(),
        "{}",
        String::from_utf8_lossy(&parallel.stderr)
    );
    assert_eq!(f.count.load(Ordering::SeqCst), 2);
    f.release.notify_one();
    assert!(first.wait_with_output().await.unwrap().status.success());
}
#[tokio::test]
async fn sigkill_releases_ownership_despite_running_status_and_preserves_unknown_files() {
    let f = fixture(true).await;
    let root = setup(&f);
    let out = root.path().join("result.md");
    std::fs::write(&out, "previous complete").unwrap();
    std::fs::write(root.path().join("untouched.txt"), "unknown").unwrap();
    let mut first = cli(root.path(), &text(&out))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    started(&f).await;
    unsafe {
        libc::kill(first.id().unwrap() as i32, libc::SIGKILL);
    }
    let _ = first.wait().await;
    assert_eq!(status(&out)["status"], "running");
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "previous complete");
    let second = cli(root.path(), &text(&out)).output().await.unwrap();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(std::fs::read_to_string(out).unwrap().contains("report-2"));
    assert_eq!(
        std::fs::read_to_string(root.path().join("untouched.txt")).unwrap(),
        "unknown"
    );
    f.release.notify_one();
}
#[tokio::test]
async fn feedback_binds_immutable_report_hash_and_human_decision_is_explicit() {
    let f = fixture(false).await;
    let root = setup(&f);
    let out = root.path().join("result.md");
    let result = cli(root.path(), &text(&out)).output().await.unwrap();
    assert!(result.status.success());
    let state = status(&out);
    let manifest_path = PathBuf::from(state["manifest"].as_str().unwrap());
    let manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    let report = &manifest["reports"][0];
    let run = state["run_id"].as_str().unwrap();
    let id = report["attempt_id"].as_str().unwrap();
    let mut args = vec![
        "feedback".into(),
        "--run".into(),
        run.into(),
        "--report".into(),
        id.into(),
        "--accepted".into(),
        "1".into(),
        "--evidence".into(),
        "fixture acceptance evidence".into(),
    ];
    let agent = cli(root.path(), &args).output().await.unwrap();
    assert!(
        agent.status.success(),
        "{}",
        String::from_utf8_lossy(&agent.stderr)
    );
    let record: Value = serde_json::from_slice(&agent.stdout).unwrap();
    assert_eq!(record["source"], "agent");
    assert!(record["counts"]["false-positive"].is_null());
    args.extend(["--source".into(), "human".into()]);
    let rejected = cli(root.path(), &args).output().await.unwrap();
    assert!(!rejected.status.success());
    args.push("--human-confirmed".into());
    let human = cli(root.path(), &args).output().await.unwrap();
    assert!(human.status.success());
    std::fs::write(report["path"].as_str().unwrap(), "changed").unwrap();
    let stale = cli(root.path(), &args).output().await.unwrap();
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("hash mismatch"));
    assert_eq!(
        std::fs::read_to_string(manifest_path.parent().unwrap().join("feedback.jsonl"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert_eq!(f.count.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn offline_help_routes_and_doctor_do_not_create_run_artifacts() {
    let f = fixture(false).await;
    let root = setup(&f);
    for args in [
        vec!["help".into()],
        vec!["routes".into(), "--json".into()],
        vec!["doctor".into()],
    ] {
        let result = cli(root.path(), &args).output().await.unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!String::from_utf8_lossy(&result.stdout).contains("fixture-key"));
    }
    assert!(!root.path().join("state/runs").exists());
    assert_eq!(f.count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_arguments_and_material_fail_before_requests_and_mark_started_run_failed() {
    let f = fixture(false).await;
    let root = setup(&f);
    let out = root.path().join("result.md");
    std::fs::write(&out, "previous complete").unwrap();
    let args = vec![
        "run".into(),
        "glm-api".into(),
        "--out".into(),
        out.display().to_string(),
        "--bad-flag".into(),
    ];
    let bad = cli(root.path(), &args).output().await.unwrap();
    assert_eq!(bad.status.code(), Some(2));
    assert!(!root.path().join("state/runs").exists());
    let mut args = text(&out);
    args.extend([
        "--input".into(),
        root.path().join("missing.txt").display().to_string(),
    ]);
    let missing = cli(root.path(), &args).output().await.unwrap();
    assert!(!missing.status.success());
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "previous complete");
    assert_eq!(status(&out)["status"], "error");
    assert_eq!(f.count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn dispatch_rejects_report_symlinks_and_escaping_cache_directory_before_requests() {
    use std::os::unix::fs::symlink;
    let f = fixture(false).await;
    let root = setup(&f);
    let dir = root.path().join("reports");
    std::fs::create_dir(&dir).unwrap();
    let unknown = dir.join("unknown.txt");
    std::fs::write(&unknown, "unknown content").unwrap();
    symlink(&unknown, dir.join("glm.md")).unwrap();
    let linked = cli(root.path(), &review(&dir)).output().await.unwrap();
    assert!(!linked.status.success());
    assert_eq!(
        std::fs::read_to_string(&unknown).unwrap(),
        "unknown content"
    );
    assert_eq!(f.count.load(Ordering::SeqCst), 0);
    std::fs::remove_file(dir.join("glm.md")).unwrap();
    let external = root.path().join("unmanaged");
    std::fs::create_dir(&external).unwrap();
    symlink(&external, dir.join(".cache")).unwrap();
    let cache = cli(root.path(), &review(&dir)).output().await.unwrap();
    assert!(!cache.status.success());
    assert!(String::from_utf8_lossy(&cache.stderr).contains("derived cache directory"));
    assert_eq!(std::fs::read_dir(external).unwrap().count(), 0);
    assert_eq!(f.count.load(Ordering::SeqCst), 0);
}

use multi_ai::{
    api::{CompletionError, request},
    args::Args,
    config::ResolvedRequest,
    runtime::OutputContext,
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

struct Reply {
    status: u16,
    header_delay: u64,
    chunks: Vec<(u64, Vec<u8>)>,
    end: bool,
    gate: Option<Arc<tokio::sync::Notify>>,
}
impl Reply {
    fn payload(payload: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            header_delay: 0,
            chunks: vec![(0, payload.into())],
            end: true,
            gate: None,
        }
    }
}
struct Fixture {
    endpoint: String,
    requests: Arc<AtomicUsize>,
    wires: Arc<Mutex<Vec<Value>>>,
    server: JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
async fn fixture<F>(handler: F) -> Fixture
where
    F: Fn(Value, usize) -> Reply + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let wires = Arc::new(Mutex::new(vec![]));
    let handler = Arc::new(handler);
    let counts = requests.clone();
    let captures = wires.clone();
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut connection, _)) = listener.accept().await else {
                return;
            };
            let count = counts.clone();
            let captures = captures.clone();
            let handler = handler.clone();
            tokio::spawn(async move {
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 2048];
                let headers;
                loop {
                    let n = connection.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                    if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                        headers = end + 4;
                        break;
                    }
                }
                let head = String::from_utf8_lossy(&buffer[..headers]);
                let length = head
                    .lines()
                    .find_map(|line| {
                        line.to_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                while buffer.len() < headers + length {
                    let n = connection.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                }
                let wire = serde_json::from_slice(&buffer[headers..headers + length])
                    .unwrap_or(Value::Null);
                captures.lock().unwrap().push(wire.clone());
                let number = count.fetch_add(1, Ordering::SeqCst) + 1;
                let reply = handler(wire, number);
                tokio::time::sleep(Duration::from_millis(reply.header_delay)).await;
                let headers = format!(
                    "HTTP/1.1 {} Test\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                    reply.status
                );
                if connection.write_all(headers.as_bytes()).await.is_err() {
                    return;
                }
                let last = reply.chunks.len().saturating_sub(1);
                for (index, (delay, bytes)) in reply.chunks.into_iter().enumerate() {
                    if index == last
                        && let Some(gate) = &reply.gate
                    {
                        gate.notified().await;
                    }
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    if !bytes.is_empty() {
                        let size = format!("{:x}\r\n", bytes.len());
                        if connection.write_all(size.as_bytes()).await.is_err() {
                            return;
                        }
                        if connection.write_all(&bytes).await.is_err() {
                            return;
                        }
                        if connection.write_all(b"\r\n").await.is_err() {
                            return;
                        }
                    }
                }
                if reply.end {
                    let _ = connection.write_all(b"0\r\n\r\n").await;
                } else {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            });
        }
    });
    Fixture {
        endpoint: format!("http://{address}/chat"),
        requests,
        wires,
        server,
    }
}
fn event(content: &str, reason: Option<&str>) -> String {
    format!(
        "data: {}\r\n\r\n",
        json!({"model":"fixture-observed","choices":[{"delta":{"content":content},"finish_reason":reason}]})
    )
}
fn spec(f: &Fixture) -> ResolvedRequest {
    ResolvedRequest {
        provider: "fixture".into(),
        model: "fixture-requested".into(),
        model_source: "test".into(),
        key: "fixture-key".into(),
        url: f.endpoint.clone(),
        body: json!({"model":"fixture-requested","stream":true,"messages":[{"role":"user","content":"fixture"}]}),
        timeout_ms: 1500,
        headers_timeout_ms: 500,
        idle_timeout_ms: 500,
        retries: 0,
        stream: true,
        thinking: "off".into(),
        effort: None,
    }
}
fn context(root: &tempfile::TempDir) -> OutputContext {
    let mut args = Args::default();
    args.set("out", root.path().join("out.md").display());
    args.set("progress-dir", root.path().join("progress").display());
    OutputContext::new(&args, "transport-test", false).unwrap()
}
fn error_code(e: &anyhow::Error) -> &str {
    e.downcast_ref::<CompletionError>().unwrap().code
}

#[tokio::test]
async fn persists_partial_before_completion_split_utf8_crlf_and_usage() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let server_gate = gate.clone();
    let f=fixture(move|_,_| {
        let bytes=event("测试✓",None).into_bytes();let start=bytes.windows("测".len()).position(|w|w=="测".as_bytes()).unwrap()+1;
        let finish=format!("data: {}\r\n\r\ndata: [DONE]",json!({"choices":[{"delta":{"content":"完成"},"finish_reason":"stop"}],"usage":{"completion_tokens":12}})).into_bytes();
        Reply {status:200,header_delay:0,chunks:vec![(0,bytes[..start].to_vec()),(0,bytes[start..].to_vec()),(0,finish)],end:true, gate: Some(server_gate.clone())}
    }).await;
    let root = tempfile::tempdir().unwrap();
    let ctx = context(&root);
    let attempt = ctx.attempt("utf8").unwrap();
    let mut spec = spec(&f);
    spec.timeout_ms = 5000;
    spec.headers_timeout_ms = 2000;
    spec.idle_timeout_ms = 3000;
    let work = request(&spec, &attempt);
    tokio::pin!(work);
    let partial = attempt.dir.join("answer.partial.txt");
    let wait_for_partial = async {
        while !partial.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    tokio::select! {result=tokio::time::timeout(Duration::from_secs(2),wait_for_partial)=>result.expect("partial must be durable before the terminal barrier is released"),result=&mut work=>panic!("completed before terminal barrier: {result:?}")};
    assert_eq!(
        std::fs::read_to_string(attempt.dir.join("answer.partial.txt")).unwrap(),
        "测试✓"
    );
    let metadata: Value =
        serde_json::from_slice(&std::fs::read(attempt.dir.join("metadata.json")).unwrap()).unwrap();
    assert_eq!(metadata["status"], "running");
    gate.notify_one();
    let result = work.await.unwrap();
    assert_eq!(result["content"], "测试✓完成");
    assert_eq!(result["model"], "fixture-observed");
    assert_eq!(result["usage"]["completion_tokens"], 12);
    assert_eq!(result["complete"], true);
    assert!(
        result["timings"]["firstDataMs"].as_u64().unwrap()
            <= result["timings"]["totalMs"].as_u64().unwrap()
    );
}
#[tokio::test]
async fn rejects_truncation_missing_terminal_malformed_sse_and_thinking_only() {
    for (label, payload, expected, partial) in [
        (
            "length",
            format!("{}data: [DONE]\n", event("partial", Some("length"))),
            "truncated",
            true,
        ),
        ("eof", event("partial", None), "incomplete", true),
        (
            "malformed",
            format!("{}data: {{broken}}\n", event("partial", None)),
            "incomplete",
            true,
        ),
        (
            "reasoning",
            format!(
                "data: {}\n",
                json!({"choices":[{"delta":{"reasoning_content":"thinking"},"finish_reason":"stop"}]})
            ),
            "empty",
            false,
        ),
    ] {
        let f = fixture(move |_, _| Reply::payload(payload.clone())).await;
        let root = tempfile::tempdir().unwrap();
        let ctx = context(&root);
        let attempt = ctx.attempt(label).unwrap();
        let failure = request(&spec(&f), &attempt).await.unwrap_err();
        assert_eq!(error_code(&failure), expected);
        let metadata: Value =
            serde_json::from_slice(&std::fs::read(attempt.dir.join("metadata.json")).unwrap())
                .unwrap();
        assert_eq!(metadata["status"], expected);
        assert!(attempt.dir.join("response.raw").exists());
        if partial {
            assert_eq!(
                std::fs::read_to_string(attempt.dir.join("answer.partial.txt")).unwrap(),
                "partial"
            );
        }
    }
}
#[tokio::test]
async fn distinguish_header_idle_and_total_timeout_and_never_retry_received_answer() {
    let headers = fixture(|_, _| Reply {
        status: 200,
        header_delay: 200,
        chunks: vec![(0, event("late", Some("stop")).into_bytes())],
        end: true,
        gate: None,
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let ctx = context(&root);
    let mut s = spec(&headers);
    s.headers_timeout_ms = 35;
    assert_eq!(
        error_code(
            &request(&s, &ctx.attempt("headers").unwrap())
                .await
                .unwrap_err()
        ),
        "headers_timeout"
    );
    let idle = fixture(|_, _| Reply {
        status: 200,
        header_delay: 0,
        chunks: vec![(0, event("partial", None).into_bytes())],
        end: false,
        gate: None,
    })
    .await;
    let mut s = spec(&idle);
    s.idle_timeout_ms = 35;
    s.retries = 2;
    let attempt = ctx.attempt("idle").unwrap();
    assert_eq!(
        error_code(&request(&s, &attempt).await.unwrap_err()),
        "idle_timeout"
    );
    assert_eq!(idle.requests.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read_to_string(attempt.dir.join("answer.partial.txt")).unwrap(),
        "partial"
    );
    let busy = fixture(|_, _| Reply {
        status: 200,
        header_delay: 0,
        chunks: (0..15)
            .map(|_| (15, event("x", None).into_bytes()))
            .collect(),
        end: true,
        gate: None,
    })
    .await;
    let mut s = spec(&busy);
    s.timeout_ms = 80;
    s.idle_timeout_ms = 50;
    assert_eq!(
        error_code(
            &request(&s, &ctx.attempt("total").unwrap())
                .await
                .unwrap_err()
        ),
        "total_timeout"
    );
}
#[tokio::test]
async fn active_data_resets_idle_timeout() {
    let f = fixture(|_, _| Reply {
        status: 200,
        header_delay: 0,
        chunks: (0..5)
            .map(|i| {
                (
                    25,
                    event("x", if i == 4 { Some("stop") } else { None }).into_bytes(),
                )
            })
            .collect(),
        end: true,
        gate: None,
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let ctx = context(&root);
    let mut s = spec(&f);
    s.idle_timeout_ms = 65;
    assert_eq!(
        request(&s, &ctx.attempt("active").unwrap()).await.unwrap()["content"],
        "xxxxx"
    );
}
#[tokio::test]
async fn retries_count_in_total_and_auth_never_retries_with_split_key_redaction() {
    let f = fixture(|_, _| Reply {
        status: 503,
        ..Reply::payload(b"busy".to_vec())
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let ctx = context(&root);
    let mut s = spec(&f);
    s.timeout_ms = 85;
    s.retries = 2;
    let start = Instant::now();
    assert_eq!(
        error_code(
            &request(&s, &ctx.attempt("retry").unwrap())
                .await
                .unwrap_err()
        ),
        "total_timeout"
    );
    assert_eq!(f.requests.load(Ordering::SeqCst), 1);
    assert!(start.elapsed() < Duration::from_millis(350));
    for status in [401, 403] {
        let f = fixture(move |_, _| Reply {
            status,
            header_delay: 0,
            chunks: vec![
                (0, b"{\"error\":\"fixture-".to_vec()),
                (15, b"key\"}".to_vec()),
            ],
            end: true,
            gate: None,
        })
        .await;
        let mut s = spec(&f);
        s.retries = 2;
        let attempt = ctx.attempt("auth").unwrap();
        let failure = request(&s, &attempt).await.unwrap_err();
        assert_eq!(error_code(&failure), "auth");
        assert!(!failure.to_string().contains("fixture-key"));
        let raw = std::fs::read_to_string(attempt.dir.join("response.raw")).unwrap();
        assert!(!raw.contains("fixture-key"));
        assert!(raw.contains("[REDACTED]"));
        assert_eq!(f.requests.load(Ordering::SeqCst), 1);
    }
}
#[tokio::test]
async fn non_streaming_rejects_truncation_thinking_only_invalid_json_and_unsupported_finish() {
    for (payload,code) in [
        (json!({"choices":[{"message":{"content":"partial"},"finish_reason":"length"}]}).to_string(),"truncated"),
        (json!({"choices":[{"message":{"content":"","reasoning_content":"think"},"finish_reason":"stop"}]}).to_string(),"empty"),
        ("{broken}".to_owned(),"incomplete"),
        (json!({"choices":[{"message":{"content":"filtered"},"finish_reason":"content_filter"}]}).to_string(),"incomplete"),
    ] {let f=fixture(move|_,_|Reply::payload(payload.clone())).await;let root=tempfile::tempdir().unwrap();let ctx=context(&root);let mut s=spec(&f);s.stream=false;s.body.as_object_mut().unwrap().remove("stream");assert_eq!(error_code(&request(&s,&ctx.attempt("nonstream").unwrap()).await.unwrap_err()),code);}
}
#[tokio::test]
async fn final_content_and_raw_redact_key_spanning_sse_deltas() {
    let f = fixture(|_, _| {
        Reply::payload(format!(
            "{}{}",
            event("fixture-", None),
            event("key", Some("stop"))
        ))
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let ctx = context(&root);
    let attempt = ctx.attempt("redact").unwrap();
    let result = request(&spec(&f), &attempt).await.unwrap();
    assert_eq!(result["content"], "[REDACTED]");
    assert!(
        !std::fs::read_to_string(attempt.dir.join("answer.partial.txt"))
            .unwrap()
            .contains("fixture-key")
    );
}

fn binary(root: &std::path::Path) -> std::path::PathBuf {
    let target = root.join("multi-ai-fixture");
    if !target.exists() {
        let source = std::path::Path::new(env!("CARGO_BIN_EXE_multi-ai"));
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match std::fs::copy(source, &target) {
                Ok(_) => break,
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("Cannot snapshot test binary: {error}"),
            }
        }
    }
    target
}

async fn cli(
    f: &Fixture,
    root: &tempfile::TempDir,
    arguments: Vec<String>,
) -> std::process::Output {
    let config = root.path().join("config");
    std::fs::create_dir_all(&config).unwrap();
    let mut profiles: Value =
        serde_json::from_str(include_str!("../config/profiles.json")).unwrap();
    for provider in profiles["providers"].as_object_mut().unwrap().values_mut() {
        provider["baseUrl"] = json!(f.endpoint.trim_end_matches("/chat"));
        provider["chatPath"] = json!("/chat");
    }
    std::fs::write(
        config.join("profiles.json"),
        serde_json::to_vec(&profiles).unwrap(),
    )
    .unwrap();
    tokio::process::Command::new(binary(root.path()))
        .arg("text")
        .args(arguments)
        .current_dir(root.path())
        .env("HOME", root.path())
        .env("MULTI_AI_CONFIG_DIR", config)
        .env("MULTI_AI_STATE_DIR", root.path().join("state"))
        .env("ZHIPU_API_KEY", "fixture-key")
        .env("DEEPSEEK_API_KEY", "fixture-key")
        .env("KIMI_API_KEY", "fixture-key")
        .env("LLM_RUN_RETRIES", "0")
        .env("LLM_RUN_QUIET", "1")
        .output()
        .await
        .unwrap()
}
fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}
#[tokio::test]
async fn aliases_material_json_units_and_failure_preserve_previous_output() {
    let truncated = Arc::new(AtomicUsize::new(0));
    let setting = truncated.clone();
    let f = fixture(move |wire, _| {
        Reply::payload(if wire["stream"] == true {
            event(
                "answer",
                Some(if setting.load(Ordering::SeqCst) == 1 {
                    "length"
                } else {
                    "stop"
                }),
            )
        } else {
            json!({"choices":[{"message":{"content":"answer"},"finish_reason":"stop"}]}).to_string()
        })
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("evidence.txt");
    let second = root.path().join("second.txt");
    let out = root.path().join("answer.json");
    std::fs::write(&input, "LOCAL_SENTINEL_817").unwrap();
    std::fs::write(&second, "SECOND_SENTINEL_331").unwrap();
    let request_args = vec![
        "glm".into(),
        "只依据材料".into(),
        "--input".into(),
        input.display().to_string(),
        "--input".into(),
        second.display().to_string(),
        "--out".into(),
        out.display().to_string(),
        "--json".into(),
        "--effort".into(),
        "low".into(),
        "--timeout-ms".into(),
        "3500".into(),
    ];
    let good = cli(&f, &root, request_args.clone()).await;
    assert!(
        good.status.success(),
        "{}",
        String::from_utf8_lossy(&good.stderr)
    );
    let original = std::fs::read_to_string(&out).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&original).unwrap()["content"],
        "answer"
    );
    let wire = f.wires.lock().unwrap()[0].clone();
    assert_eq!(wire["stream"], true);
    assert_eq!(wire["reasoning_effort"], "low");
    assert!(
        wire["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("LOCAL_SENTINEL_817")
    );
    assert!(
        wire["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("SECOND_SENTINEL_331")
    );
    truncated.store(1, Ordering::SeqCst);
    let failed = cli(&f, &root, request_args).await;
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("truncated"));
    assert_eq!(std::fs::read_to_string(out).unwrap(), original);
}
#[tokio::test]
async fn explicit_no_stream_images_stdin_empty_input_and_dead_aliases() {
    let f = fixture(|wire, _| {
        Reply::payload(if wire["stream"] == true {
            event("answer", Some("stop"))
        } else {
            json!({"choices":[{"message":{"content":"answer"},"finish_reason":"stop"}]}).to_string()
        })
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let good = cli(&f, &root, args(&["glm", "health", "--no-stream", "--json"])).await;
    assert!(
        good.status.success(),
        "{}",
        String::from_utf8_lossy(&good.stderr)
    );
    assert_eq!(f.wires.lock().unwrap()[0]["stream"], Value::Null);
    let image = root.path().join("image.png");
    std::fs::write(&image, [137, 80, 78, 71]).unwrap();
    let good = cli(
        &f,
        &root,
        vec![
            "glm-flash".into(),
            "inspect".into(),
            "--image".into(),
            image.display().to_string(),
        ],
    )
    .await;
    assert!(
        good.status.success(),
        "{}",
        String::from_utf8_lossy(&good.stderr)
    );
    assert!(
        f.wires.lock().unwrap()[1]["messages"][1]["content"][1]["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,")
    );
    let empty = root.path().join("empty");
    std::fs::write(&empty, " ").unwrap();
    let failed = cli(
        &f,
        &root,
        vec!["glm".into(), "--input".into(), empty.display().to_string()],
    )
    .await;
    assert!(!failed.status.success());
    let failed = cli(&f, &root, args(&["kimi-fast", "health"])).await;
    assert!(!failed.status.success());
    assert_eq!(f.requests.load(Ordering::SeqCst), 2);
    let good = cli(&f, &root, args(&["kimi-fast", "health", "--legacy"])).await;
    assert!(
        good.status.success(),
        "{}",
        String::from_utf8_lossy(&good.stderr)
    );
    assert_eq!(f.wires.lock().unwrap()[2]["model"], "deepseek-v4-flash");
}
#[tokio::test]
async fn roundtable_partial_seat_preserves_previous_complete_report_and_domains_resolve_tasks() {
    let f = fixture(|wire, _| {
        let reason = if wire["model"] == "glm-5.3" {
            "length"
        } else {
            "stop"
        };
        Reply::payload(if wire["stream"] == true {
            event("answer", Some(reason))
        } else {
            json!({"choices":[{"message":{"content":"answer"},"finish_reason":reason}]}).to_string()
        })
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("round.md");
    std::fs::write(&out, "previous complete report").unwrap();
    let failed = cli(
        &f,
        &root,
        vec![
            "run".into(),
            "--task".into(),
            "roundtable".into(),
            "--out".into(),
            out.display().to_string(),
        ],
    )
    .await;
    assert!(
        !failed.status.success(),
        "{}",
        String::from_utf8_lossy(&failed.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "previous complete report"
    );
    assert!(std::path::PathBuf::from(format!("{}.partial", out.display())).exists());
    assert_eq!(f.requests.load(Ordering::SeqCst), 3);
    let good = cli(
        &f,
        &root,
        args(&["dispatch", "--domain", "extract", "--kind", "map", "--json"]),
    )
    .await;
    assert!(
        good.status.success(),
        "{}",
        String::from_utf8_lossy(&good.stderr)
    );
    let wires = f.wires.lock().unwrap();
    assert_eq!(wires[3]["model"], "deepseek-v4-flash");
    assert!(
        wires[3]["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("证据包")
    );
}
#[tokio::test]
async fn help_and_host_adapter_are_offline_with_no_progress_artifacts() {
    let f = fixture(|_, _| Reply {
        status: 500,
        ..Reply::payload(b"must not dispatch".to_vec())
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    for arguments in [
        args(&["help"]),
        args(&["glm", "--help"]),
        args(&["run", "-h"]),
        args(&["profiles", "--json"]),
        args(&["domains", "--json"]),
        args(&["aliases", "--json"]),
        args(&["dispatch", "--domain", "private"]),
    ] {
        let result = cli(&f, &root, arguments).await;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    assert_eq!(f.requests.load(Ordering::SeqCst), 0);
    assert!(!root.path().join("state/runs").exists());
}

#[tokio::test]
async fn text_resume_reuses_exact_complete_and_model_or_input_changes_rerun() {
    let f = fixture(|wire, _| {
        Reply::payload(if wire["stream"] == true {
            event("answer", Some("stop"))
        } else {
            json!({"choices":[{"message":{"content":"answer"},"finish_reason":"stop"}]}).to_string()
        })
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("answer.md");
    let input = root.path().join("input.md");
    std::fs::write(&input, "first material").unwrap();
    let base = vec![
        "glm".into(),
        "health".into(),
        "--out".into(),
        out.display().to_string(),
        "--input".into(),
        input.display().to_string(),
        "--source-id".into(),
        "fixture-workspace".into(),
    ];
    let good = cli(&f, &root, base.clone()).await;
    assert!(
        good.status.success(),
        "{}",
        String::from_utf8_lossy(&good.stderr)
    );
    assert_eq!(f.requests.load(Ordering::SeqCst), 1);
    let mut resume = base.clone();
    resume.push("--resume".into());
    let good = cli(&f, &root, resume.clone()).await;
    assert!(
        good.status.success(),
        "{}",
        String::from_utf8_lossy(&good.stderr)
    );
    assert_eq!(f.requests.load(Ordering::SeqCst), 1);
    let status: Value =
        serde_json::from_slice(&std::fs::read(format!("{}.status.json", out.display())).unwrap())
            .unwrap();
    assert_eq!(status["result"]["cached"], true);
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(status["manifest"].as_str().unwrap()).unwrap())
            .unwrap();
    assert_eq!(manifest["reports"][0]["model_calls"], 0);
    std::fs::write(&input, "changed material").unwrap();
    let good = cli(&f, &root, resume.clone()).await;
    assert!(
        good.status.success(),
        "{}",
        String::from_utf8_lossy(&good.stderr)
    );
    assert_eq!(f.requests.load(Ordering::SeqCst), 2);
    resume.extend(args(&["--model", "glm-5.3-flash"]));
    let good = cli(&f, &root, resume).await;
    assert!(
        good.status.success(),
        "{}",
        String::from_utf8_lossy(&good.stderr)
    );
    assert_eq!(f.requests.load(Ordering::SeqCst), 3);
    assert_eq!(f.wires.lock().unwrap()[2]["model"], "glm-5.3-flash");
}
#[tokio::test]
async fn roundtable_resume_has_zero_calls_and_each_report_preserves_original_identity() {
    let f = fixture(|wire, _| {
        Reply::payload(if wire["stream"] == true {
            event("answer", Some("stop"))
        } else {
            json!({"choices":[{"message":{"content":"answer"},"finish_reason":"stop"}]}).to_string()
        })
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("round.md");
    let base = vec![
        "run".into(),
        "--task".into(),
        "roundtable".into(),
        "--out".into(),
        out.display().to_string(),
        "--source-id".into(),
        "fixture-workspace".into(),
    ];
    let good = cli(&f, &root, base.clone()).await;
    assert!(
        good.status.success(),
        "{}",
        String::from_utf8_lossy(&good.stderr)
    );
    let original = std::fs::read_to_string(&out).unwrap();
    assert_eq!(f.requests.load(Ordering::SeqCst), 3);
    let mut resume = base;
    resume.push("--resume".into());
    let good = cli(&f, &root, resume).await;
    assert!(
        good.status.success(),
        "{}",
        String::from_utf8_lossy(&good.stderr)
    );
    assert_eq!(f.requests.load(Ordering::SeqCst), 3);
    assert_eq!(std::fs::read_to_string(&out).unwrap(), original);
    let status: Value =
        serde_json::from_slice(&std::fs::read(format!("{}.status.json", out.display())).unwrap())
            .unwrap();
    assert_eq!(status["cached"], true);
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(status["manifest"].as_str().unwrap()).unwrap())
            .unwrap();
    assert_eq!(manifest["reports"].as_array().unwrap().len(), 3);
    for row in manifest["reports"].as_array().unwrap() {
        assert_eq!(row["model_calls"], 0);
        assert!(row["result"]["reused_from"]["attempt_id"].is_string());
    }
}
#[tokio::test]
async fn stdin_repeatable_and_project_env_model_resolution_match_alias_display() {
    let f = fixture(|wire, _| {
        Reply::payload(if wire["stream"] == true {
            event("answer", Some("stop"))
        } else {
            json!({"choices":[{"message":{"content":"answer"},"finish_reason":"stop"}]}).to_string()
        })
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(".env"), "GLM_MODEL=glm-5.3-flash\n").unwrap();
    let displayed = cli(&f, &root, args(&["aliases"])).await;
    assert!(displayed.status.success());
    assert!(
        String::from_utf8_lossy(&displayed.stdout)
            .lines()
            .any(|line| line.split_whitespace().next() == Some("glm")
                && line.contains("glm-5.3-flash"))
    );
    let prime = cli(&f, &root, args(&["glm", "prime"])).await;
    assert!(prime.status.success());
    assert_eq!(f.wires.lock().unwrap()[0]["model"], "glm-5.3-flash");
    let config = root.path().join("config");
    let mut child = tokio::process::Command::new(binary(root.path()))
        .args(["text", "glm", "--input", "-", "--input", "-", "--json"])
        .current_dir(root.path())
        .env("HOME", root.path())
        .env("MULTI_AI_CONFIG_DIR", config)
        .env("MULTI_AI_STATE_DIR", root.path().join("state"))
        .env("ZHIPU_API_KEY", "fixture-key")
        .env("LLM_RUN_RETRIES", "0")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"STDIN_SENTINEL_928")
        .await
        .unwrap();
    let result = child.wait_with_output().await.unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let wire = &f.wires.lock().unwrap()[1];
    assert_eq!(
        wire["messages"][1]["content"]
            .as_str()
            .unwrap()
            .matches("STDIN_SENTINEL_928")
            .count(),
        2
    );
}

#[tokio::test]
async fn stream_false_and_models_listing_preserve_legacy_commands() {
    let f = fixture(|wire, _| {
        Reply::payload(if wire.is_null() {
            json!({"data":[{"id":"model-a"},{"id":"model-b"}]}).to_string()
        } else if wire["stream"] == true {
            event("answer", Some("stop"))
        } else {
            json!({"choices":[{"message":{"content":"answer"},"finish_reason":"stop"}]}).to_string()
        })
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let result = cli(&f, &root, args(&["glm", "health", "--stream", "false"])).await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(f.wires.lock().unwrap()[0]["stream"], Value::Null);
    let result = cli(&f, &root, args(&["list-models", "--json"])).await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let listed: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(listed["providers"].as_object().unwrap().len(), 3);
    assert_eq!(
        listed["providers"]["kimi"]["models"],
        json!(["model-a", "model-b"])
    );
    assert_eq!(f.requests.load(Ordering::SeqCst), 4);
}
#[tokio::test]
async fn api_works_with_python_node_uv_cargo_absent_and_missing_clis_remain_optional() {
    let f = fixture(|wire, _| {
        Reply::payload(if wire["stream"] == true {
            event("answer", Some("stop"))
        } else {
            json!({"choices":[{"message":{"content":"answer"},"finish_reason":"stop"}]}).to_string()
        })
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let prime = cli(&f, &root, args(&["help"])).await;
    assert!(prime.status.success());
    for arguments in [
        args(&["help"]),
        args(&["routes", "--json"]),
        args(&["doctor"]),
        args(&["text", "glm", "health", "--json"]),
    ] {
        let result = tokio::process::Command::new(binary(root.path()))
            .args(arguments)
            .current_dir(root.path())
            .env("HOME", root.path())
            .env("PATH", "")
            .env("MULTI_AI_CONFIG_DIR", root.path().join("config"))
            .env("MULTI_AI_STATE_DIR", root.path().join("state"))
            .env("ZHIPU_API_KEY", "fixture-key")
            .env("LLM_RUN_RETRIES", "0")
            .output()
            .await
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    assert_eq!(f.requests.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn authorization_override_is_per_invocation_and_material_keys_are_redacted() {
    let f = fixture(|wire, _| {
        Reply::payload(if wire["stream"] == true {
            event("answer", Some("stop"))
        } else {
            json!({"choices":[{"message":{"content":"answer"},"finish_reason":"stop"}]}).to_string()
        })
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("synthetic.txt");
    std::fs::write(
        &input,
        "SYNTHETIC: 合同金额 100000; sk-abcdefghijklmnopqrstuvwxyz123456",
    )
    .unwrap();
    let arguments = vec!["glm".into(), "--input".into(), input.display().to_string()];
    let refused = cli(&f, &root, arguments.clone()).await;
    assert!(!refused.status.success());
    assert_eq!(f.requests.load(Ordering::SeqCst), 0);
    std::fs::write(root.path().join(".env"), "LLM_RUN_ALLOW_CLIENT_DATA=1\n").unwrap();
    let authorized = cli(&f, &root, arguments.clone()).await;
    assert!(
        authorized.status.success(),
        "{}",
        String::from_utf8_lossy(&authorized.stderr)
    );
    assert_eq!(f.requests.load(Ordering::SeqCst), 1);
    let wire = f.wires.lock().unwrap()[0].clone();
    let material = wire["messages"][1]["content"].as_str().unwrap();
    assert!(!material.contains("sk-abcdefghijklmnopqrstuvwxyz123456"));
    assert!(material.contains("[REDACTED_API_KEY]"));
    std::fs::remove_file(root.path().join(".env")).unwrap();
    let refused = cli(&f, &root, arguments).await;
    assert!(!refused.status.success());
    assert_eq!(f.requests.load(Ordering::SeqCst), 1);
}

fn find_file(directory: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    for entry in std::fs::read_dir(directory).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir()
            && let Some(file) = find_file(&path, name)
        {
            return Some(file);
        } else if path.file_name().and_then(|v| v.to_str()) == Some(name) {
            return Some(path);
        }
    }
    None
}
#[tokio::test]
async fn sigterm_and_sigint_cancel_native_request_without_publishing_or_retrying() {
    let f = fixture(|_, _| Reply {
        status: 200,
        header_delay: 0,
        chunks: vec![(0, event("partial", None).into_bytes())],
        end: false,
        gate: None,
    })
    .await;
    for signal in [libc::SIGTERM, libc::SIGINT] {
        let root = tempfile::tempdir().unwrap();
        let prime = cli(&f, &root, args(&["help"])).await;
        assert!(prime.status.success());
        let output = root.path().join("answer.md");
        let progress = root.path().join("cancel-progress");
        std::fs::write(&output, "previous complete report").unwrap();
        let child = tokio::process::Command::new(binary(root.path()))
            .args(["text", "glm", "health", "--out"])
            .arg(&output)
            .arg("--progress-dir")
            .arg(&progress)
            .current_dir(root.path())
            .env("HOME", root.path())
            .env("MULTI_AI_CONFIG_DIR", root.path().join("config"))
            .env("MULTI_AI_STATE_DIR", root.path().join("state"))
            .env("ZHIPU_API_KEY", "fixture-key")
            .env("LLM_RUN_RETRIES", "2")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        let partial = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(path) = find_file(&progress, "answer.partial.txt") {
                    break path;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("native API must write partial before signalling");
        assert_eq!(std::fs::read_to_string(&partial).unwrap(), "partial");
        unsafe {
            assert_eq!(libc::kill(pid as i32, signal), 0);
        }
        let result = tokio::time::timeout(Duration::from_secs(2), child.wait_with_output())
            .await
            .expect("cancel must promptly terminate request")
            .unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("cancelled"));
        assert_eq!(
            std::fs::read_to_string(&output).unwrap(),
            "previous complete report"
        );
        let metadata: Value = serde_json::from_slice(
            &std::fs::read(partial.parent().unwrap().join("metadata.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(metadata["status"], "cancelled");
    }
    assert_eq!(f.requests.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn progress_env_uses_bounded_stderr_not_stdout_and_never_emits_material() {
    let f = fixture(|_, _| Reply::payload(event("answer", Some("stop")))).await;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(".env"), "LLM_RUN_PROGRESS=1\n").unwrap();
    let result = cli(
        &f,
        &root,
        args(&["glm", "PRIVATE_PROMPT_SENTINEL_996", "--json"]),
    )
    .await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(stdout["content"], "answer");
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!stderr.contains("PRIVATE_PROMPT_SENTINEL_996"));
    assert!(!stderr.contains("fixture-key"));
    let events = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("[llm-run-progress] "))
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(!events.is_empty());
    assert!(events.len() <= 5);
    assert!(events.iter().any(|event| event["type"] == "complete"));
    assert!(
        events
            .iter()
            .all(|event| event["elapsedMs"].is_number() && event["bytes"].is_number())
    );
}

#[tokio::test]
async fn resumed_json_ids_bind_new_reports_and_can_record_feedback_without_model_calls() {
    let f = fixture(|wire, _| {
        Reply::payload(if wire["stream"] == true {
            event("answer", Some("stop"))
        } else {
            json!({"choices":[{"message":{"content":"answer"},"finish_reason":"stop"}]}).to_string()
        })
    })
    .await;
    for task in [None, Some("roundtable")] {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("answer.json");
        let mut command = if task.is_some() {
            args(&["run", "--task", "roundtable"])
        } else {
            args(&["glm", "health"])
        };
        command.extend(vec![
            "--out".into(),
            output.display().to_string(),
            "--json".into(),
            "--source-id".into(),
            "fixture-workspace".into(),
        ]);
        let first = cli(&f, &root, command.clone()).await;
        assert!(
            first.status.success(),
            "{}",
            String::from_utf8_lossy(&first.stderr)
        );
        let original: Value = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        let before = f.requests.load(Ordering::SeqCst);
        command.push("--resume".into());
        let resumed = cli(&f, &root, command).await;
        assert!(
            resumed.status.success(),
            "{}",
            String::from_utf8_lossy(&resumed.stderr)
        );
        assert_eq!(f.requests.load(Ordering::SeqCst), before);
        let json: Value = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        let results = if task.is_some() {
            json["results"].as_array().unwrap().clone()
        } else {
            vec![json]
        };
        let originals = if task.is_some() {
            original["results"].as_array().unwrap().clone()
        } else {
            vec![original]
        };
        for (report, old) in results.iter().zip(originals) {
            assert_eq!(report["runId"], report["run_id"]);
            assert_eq!(report["attemptId"], report["attempt_id"]);
            assert_ne!(report["run_id"], old["run_id"]);
            assert_ne!(report["attempt_id"], old["attempt_id"]);
            assert_eq!(report["reused_from"]["run_id"], old["run_id"]);
            assert_eq!(report["reused_from"]["attempt_id"], old["attempt_id"]);
            let feedback = tokio::process::Command::new(binary(root.path()))
                .args([
                    "feedback",
                    "--run",
                    report["run_id"].as_str().unwrap(),
                    "--report",
                    report["attempt_id"].as_str().unwrap(),
                    "--accepted",
                    "1",
                    "--evidence",
                    "local fixture confirms answer",
                ])
                .current_dir(root.path())
                .env("HOME", root.path())
                .env("MULTI_AI_STATE_DIR", root.path().join("state"))
                .output()
                .await
                .unwrap();
            assert!(
                feedback.status.success(),
                "{}",
                String::from_utf8_lossy(&feedback.stderr)
            );
            let recorded: Value = serde_json::from_slice(&feedback.stdout).unwrap();
            assert_eq!(recorded["run_id"], report["run_id"]);
            assert_eq!(recorded["attempt_id"], report["attempt_id"]);
        }
        assert_eq!(f.requests.load(Ordering::SeqCst), before);
    }
}

#[tokio::test]
async fn external_workspace_symlink_allows_fresh_call_but_requires_source_identity_for_resume() {
    use std::os::unix::fs::symlink;
    let f = fixture(|_, _| Reply::payload(event("answer", Some("stop")))).await;
    let root = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    std::fs::write(
        external.path().join("context.txt"),
        "synthetic external context",
    )
    .unwrap();
    symlink(external.path(), root.path().join("external-link")).unwrap();
    assert!(
        std::process::Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(root.path())
            .status()
            .unwrap()
            .success()
    );
    let output = root.path().join("answer.md");
    let arguments = vec![
        "glm".into(),
        "health".into(),
        "--out".into(),
        output.display().to_string(),
    ];
    let first = cli(&f, &root, arguments.clone()).await;
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(f.requests.load(Ordering::SeqCst), 1);
    assert!(!std::path::PathBuf::from(format!("{}.cache.json", output.display())).exists());
    let mut resume = arguments;
    resume.push("--resume".into());
    let refused = cli(&f, &root, resume).await;
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("source-id"));
    assert_eq!(f.requests.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read_to_string(output).unwrap(), "answer");
}

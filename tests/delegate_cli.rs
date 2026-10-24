//! Deterministic adapter boundary tests; no user auth or external model calls.
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_multi-ai")
}

struct MockInference {
    address: std::net::SocketAddr,
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    server: thread::JoinHandle<()>,
}
fn mock_server(calls: Vec<Value>) -> MockInference {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(40);
        while Instant::now() < deadline && !stopped.load(Ordering::Relaxed) {
            let Ok((mut stream, _)) = listener.accept() else {
                thread::sleep(Duration::from_millis(10));
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut end = None;
            let mut length = 0;
            loop {
                let mut buf = [0; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buf[..n]);
                if end.is_none()
                    && let Some(p) = bytes.windows(4).position(|w| w == b"\r\n\r\n")
                {
                    length = String::from_utf8_lossy(&bytes[..p])
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|s| s.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    end = Some(p + 4);
                }
                if end.is_some_and(|p| bytes.len() >= p + length) {
                    break;
                }
            }
            if bytes.starts_with(b"GET ") {
                let body = r#"{"object":"list","data":[{"id":"mock","object":"model"}]}"#;
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
                continue;
            }
            if !bytes.starts_with(b"POST /v1/chat/completions ") {
                // Vendor token counting and connection probes are not model
                // turns; keep the forced tool-call sequence on inference only.
                let body = r#"{"token_count":1,"total_tokens":1}"#;
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                continue;
            }
            let Some(start) = end.filter(|start| bytes.len() >= start + length) else {
                continue;
            };
            let Ok(request) = serde_json::from_slice::<Value>(&bytes[start..start + length]) else {
                continue;
            };
            let index = {
                let mut v = captured.lock().unwrap();
                v.push(request);
                v.len()
            };
            let delta = if let Some(call) = calls.get(index - 1) {
                json!({"role":"assistant","tool_calls":[{"index":0,"id":format!("boundary-call-{index}"),"type":"function","function":call}]})
            } else {
                json!({"role":"assistant","content":"done"})
            };
            let body = format!(
                "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                json!({"id":"mock","object":"chat.completion.chunk","created":0,"model":"mock","choices":[{"index":0,"delta":delta,"finish_reason":null}]}),
                json!({"id":"mock","object":"chat.completion.chunk","created":0,"model":"mock","choices":[{"index":0,"delta":{},"finish_reason":if index <= calls.len() {"tool_calls"}else{"stop"}}]})
            );
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    MockInference {
        address,
        requests,
        stop,
        server,
    }
}

#[test]
#[ignore = "optional installed Grok boundary check; dummy loopback inference only"]
fn installed_grok_rejects_native_tools_and_file_backed_mcp_arguments() {
    let version = Command::new("grok")
        .arg("--version")
        .output()
        .expect("install validated Grok CLI for this optional check");
    // The ` [stable]` channel suffix comes and goes with Grok's own config; the build is gated.
    assert!(
        String::from_utf8_lossy(&version.stdout)
            .trim()
            .starts_with("grok 1.0.67 (2fe064390d07)")
    );
    let exe = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|p| p.join("grok"))
        .find(|p| p.is_file())
        .unwrap();
    for form in ["native", "file", "tool_input_file"] {
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("home");
        let vendor = home.join(".grok");
        let work = temp.path().join("work");
        fs::create_dir_all(&vendor).unwrap();
        fs::create_dir(&work).unwrap();
        let marker = temp.path().join("native-shell-marker");
        let outside = temp.path().join("outside.json");
        fs::write(&outside, "INVALID-PRIVATE-FILE-CANARY").unwrap();
        let arguments = match form {
            "native" => json!({"command":format!("touch {}",marker.display())}),
            "file" => json!({"file":outside}),
            _ => json!({"tool_name":"multi_ai__read","tool_input_file":outside}),
        };
        let call = json!({"name":if form=="native" {"bash"}else{"use_tool"},"arguments":serde_json::to_string(&arguments).unwrap()});
        let MockInference {
            address,
            requests,
            stop,
            server,
        } = mock_server(vec![call]);
        fs::write(vendor.join("config.toml"),format!("[cli]\nauto_update = false\nuse_leader = false\n[models]\ndefault = \"mock\"\nmax_retries = 0\n[model.mock]\nmodel = \"mock\"\nbase_url = \"http://{address}/v1\"\napi_key = \"dummy-only-loopback\"\napi_backend = \"chat_completions\"\n[[permission.rules]]\naction = \"deny\"\ntool = \"read\"\npattern = \"**\"\n[subagents]\nenabled = false\n[memory]\nenabled = false\n[compat.cursor]\nskills = false\nrules = false\nagents = false\nmcps = false\nhooks = false\n[compat.claude]\nskills = false\nrules = false\nagents = false\nmcps = false\nhooks = false\n")).unwrap();
        let agent = temp.path().join("agent.md");
        fs::write(&agent,"---\nname: multi-ai-worker\ndescription: Restricted worker\ndiscoverSkills: false\ninheritSkills: false\nagentsMd: false\ntools: [search_tool, use_tool]\nhooks: {}\n---\nOnly use the restricted tools.\n").unwrap();
        let mut child = Command::new(&exe)
            .env_clear()
            .env("HOME", &home)
            .env("GROK_HOME", &vendor)
            .env("PATH", "/usr/bin:/bin")
            .env("GROK_DISABLE_AUTOUPDATER", "1")
            .env("XAI_API_KEY", "dummy-only-loopback")
            .env("GROK_SUBAGENTS", "0")
            .env("GROK_MEMORY", "0")
            .current_dir(&work)
            .arg("--agent")
            .arg(&agent)
            .args([
                "--model",
                "mock",
                "--tools",
                "search_tool,use_tool",
                "--disable-web-search",
                "--no-subagents",
                "--always-approve",
                "--max-turns",
                "3",
                "--output-format",
                "streaming-messages-json",
                "-p",
                "Return done.",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let started = Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if started.elapsed() > Duration::from_secs(30) {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let out = child.wait_with_output().unwrap();
        stop.store(true, Ordering::Relaxed);
        let _ = server.join();
        let data = requests.lock().unwrap();
        assert!(
            data.len() >= 2,
            "no local tool rejection: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let names: Vec<_> = data[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["search_tool", "use_tool"]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        if form == "native" {
            assert!(stdout.contains("Tool not found: bash"));
        } else {
            assert!(stdout.contains("MCP source Read denied"), "{stdout}");
        }
        assert!(!marker.exists());
        assert!(!stdout.contains("INVALID-PRIVATE-FILE-CANARY"));
    }
}

#[test]
#[ignore = "optional installed Kimi/Grok positive broker check; dummy loopback inference only"]
fn installed_vendors_read_only_packet_material_and_finish() {
    for (vendor_name, expected_version) in
        [("kimi", "2.3.1"), ("grok", "grok 1.0.67 (2fe064390d07)")]
    {
        let exe = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|p| p.join(vendor_name))
            .find(|p| p.is_file())
            .expect("install the validated vendor CLI");
        let version = Command::new(&exe).arg("--version").output().unwrap();
        assert!(version.status.success());
        let printed = String::from_utf8_lossy(&version.stdout).trim().to_owned();
        assert!(
            printed == expected_version || printed.starts_with(&format!("{expected_version} [")),
            "{printed}"
        );
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("home");
        let vendor = home.join(if vendor_name == "kimi" {
            ".kimi-code"
        } else {
            ".grok"
        });
        let work = temp.path().join("work");
        fs::create_dir_all(&vendor).unwrap();
        fs::create_dir(&work).unwrap();
        fs::create_dir(home.join("empty-skills")).unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        fs::write(repo.join("source.txt"), "packet-only-answer-7319\n").unwrap();
        fs::write(temp.path().join("outside.txt"), "OUTSIDE-PACKET-CANARY").unwrap();
        let packet = temp.path().join("packet");
        assert!(
            Command::new(binary())
                .env_clear()
                .env("HOME", &home)
                .env("PATH", "/usr/bin:/bin")
                .args(["pack", "--root"])
                .arg(&repo)
                .args(["--input", "source.txt", "--outdir"])
                .arg(&packet)
                .output()
                .unwrap()
                .status
                .success()
        );
        let audit = temp.path().join("audit.json");
        let broker_args = json!(["_broker", "--packet", packet, "--audit", audit]);
        let mut calls = Vec::new();
        if vendor_name == "grok" {
            calls.push(json!({"name":"search_tool","arguments":"{\"query\":\"multi_ai\"}"}));
        }
        for (tool, input) in [
            ("list", json!({})),
            ("search", json!({"query":"7319"})),
            ("read", json!({"path":"source.txt"})),
            ("read", json!({"path":"../outside.txt"})),
        ] {
            let (name, arguments) = if vendor_name == "kimi" {
                (format!("mcp__multi_ai__{tool}"), input)
            } else {
                (
                    "use_tool".into(),
                    json!({"tool_name":format!("multi_ai__{tool}"),"tool_input":input}),
                )
            };
            calls.push(json!({"name":name,"arguments":serde_json::to_string(&arguments).unwrap()}));
        }
        let count = calls.len() + 1;
        let MockInference {
            address,
            requests,
            stop,
            server,
        } = mock_server(calls);
        let agent = temp.path().join("agent.md");
        let mut command = Command::new(&exe);
        command
            .env_clear()
            .env("HOME", &home)
            .env("PATH", "/usr/bin:/bin")
            .current_dir(&work);
        if vendor_name == "kimi" {
            fs::write(vendor.join("mcp.json"), serde_json::to_vec(&json!({"mcpServers":{"multi_ai":{"command":binary(),"args":broker_args,"enabledTools":["list","search","read"]}}})).unwrap()).unwrap();
            fs::write(vendor.join("config.toml"), format!("default_model = \"mock\"\ntelemetry = false\nmerge_all_available_skills = false\nbuiltin_product_skills = false\n[providers.mock]\ntype = \"kimi\"\nbase_url = \"http://{address}/v1\"\napi_key = \"dummy-only-loopback\"\n[models.mock]\nprovider = \"mock\"\nmodel = \"k3\"\nmax_context_size = 1048576\ncapabilities = [\"thinking\",\"always_thinking\",\"tool_use\"]\nsupport_efforts = [\"low\",\"high\",\"max\"]\ndefault_effort = \"low\"\n[thinking]\nenabled = true\neffort = \"low\"\n[loop_control]\nmax_attempts_per_step = 1\nmax_steps_per_turn = 8\n")).unwrap();
            fs::write(&agent,"---\nname: multi-ai-worker\ndescription: Restricted worker\ntools: [mcp__multi_ai__list, mcp__multi_ai__search, mcp__multi_ai__read]\ndisallowedTools: [select_tools]\nsubagents: []\n---\nOnly use the authorized broker.\n").unwrap();
            command
                .env("KIMI_CODE_HOME", &vendor)
                .env("KIMI_DISABLE_TELEMETRY", "1")
                .arg("--agent-file")
                .arg(&agent)
                .arg("--skills-dir")
                .arg(home.join("empty-skills"))
                .args(["--output-format", "stream-json"]);
        } else {
            fs::write(vendor.join("config.toml"), format!("[cli]\nauto_update = false\nuse_leader = false\n[session]\nload_envrc = false\n[models]\ndefault = \"mock\"\nmax_retries = 0\n[model.mock]\nmodel = \"mock\"\nbase_url = \"http://{address}/v1\"\napi_key = \"dummy-only-loopback\"\napi_backend = \"chat_completions\"\n[[permission.rules]]\naction = \"deny\"\ntool = \"read\"\npattern = \"**\"\n[subagents]\nenabled = false\n[memory]\nenabled = false\n[mcp_servers.multi_ai]\ncommand = {}\nargs = {broker_args}\n",serde_json::to_string(binary()).unwrap())).unwrap();
            fs::write(&agent,"---\nname: multi-ai-worker\ndescription: Restricted worker\ndiscoverSkills: false\ninheritSkills: false\nagentsMd: false\ntools: [search_tool, use_tool]\nhooks: {}\n---\nOnly use the authorized broker.\n").unwrap();
            command
                .env("GROK_HOME", &vendor)
                .env("GROK_DISABLE_AUTOUPDATER", "1")
                .env("XAI_API_KEY", "dummy-only-loopback")
                .env("GROK_SUBAGENTS", "0")
                .env("GROK_MEMORY", "0")
                .env("GROK_MANAGED_MCPS_ENABLED", "0")
                .env("GROK_MANAGED_MCP_GATEWAY_TOOLS_ENABLED", "0")
                .arg("--agent")
                .arg(&agent)
                .args([
                    "--model",
                    "mock",
                    "--tools",
                    "search_tool,use_tool",
                    "--disable-web-search",
                    "--no-subagents",
                    "--no-plan",
                    "--always-approve",
                    "--max-turns",
                    "8",
                    "--output-format",
                    "streaming-messages-json",
                    "--include-partial-messages",
                ]);
        }
        let mut child = command
            .args(["-p", "Inspect the packet and finish."])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let start = Instant::now();
        while child.try_wait().unwrap().is_none() {
            if start.elapsed() > Duration::from_secs(35) {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{vendor_name}: {}\n{stdout}\nlocal requests: {}",
            String::from_utf8_lossy(&output.stderr),
            requests.lock().unwrap().len()
        );
        let data = requests.lock().unwrap();
        assert!(
            data.len() >= count,
            "{vendor_name}: missing inference requests"
        );
        assert!(
            serde_json::to_string(&*data)
                .unwrap()
                .contains("packet-only-answer-7319")
        );
        assert!(
            !serde_json::to_string(&*data)
                .unwrap()
                .contains("OUTSIDE-PACKET-CANARY")
        );
        assert!(!stdout.contains("OUTSIDE-PACKET-CANARY"));
        let audit: Value = serde_json::from_slice(&fs::read(&audit).unwrap()).unwrap();
        assert_eq!(audit["tool_calls"], 4);
        assert_eq!(audit["failed_calls"], 1);
        let events = audit["events"].as_array().unwrap();
        assert!(events[..3].iter().all(|e| e["success"] == true));
        assert_eq!(events[2]["path"], "source.txt");
        assert_eq!(events[3]["success"], false);
        let rows: Vec<Value> = stdout
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        if vendor_name == "kimi" {
            assert!(
                rows.iter()
                    .any(|r| r["role"] == "assistant" && r["content"] == "done")
            );
            assert!(
                data.iter()
                    .filter(|r| r["messages"].is_array())
                    .all(|r| r["model"] == "k3" && r["thinking"]["type"] == "enabled" && r["thinking"]["effort"] == "low"),
                "request metadata: {:?}", data.iter().map(|r| json!({"model":r["model"],"thinking":r["thinking"],"reasoning_effort":r["reasoning_effort"]})).collect::<Vec<_>>()
            );
            assert!(rows.iter().any(|r| r["role"] == "meta"
                && r["type"] == "session.resume_hint"
                && r["session_id"].as_str().is_some_and(|id| !id.is_empty())));
        } else {
            let discovery = data[1]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "tool" && m["tool_call_id"] == "boundary-call-1")
                .unwrap();
            let discovered = discovery["content"].as_str().unwrap();
            assert!(discovered.contains("multi_ai"), "{discovered}");
            assert!(
                !discovered.contains("Denied") && !discovered.contains("not executed"),
                "{discovered}"
            );
            assert!(rows.iter().any(|r| r["type"] == "result"
                && r["subtype"] == "success"
                && r["is_error"] == false
                && r["result"] == "done"));
            let init = rows
                .iter()
                .find(|r| r["type"] == "system" && r["subtype"] == "init")
                .unwrap();
            assert_eq!(init["tools"], json!(["search_tool", "use_tool"]));
            assert!(
                init["mcp_servers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|s| s["name"] == "multi_ai")
            );
        }
    }
}
fn setup(version: &str, body: &str) -> (TempDir, PathBuf) {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(home.join(".grok")).unwrap();
    fs::write(home.join(".grok/auth.json"), "{}\n").unwrap();
    let exe = temp.path().join("fake-grok");
    fs::write(&exe, format!("#!/bin/sh\nif [ \"$1\" = --version ]; then printf '%s\\n' '{version}'; exit 0; fi\n{body}\n")).unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
    let cfg = temp.path().join("config");
    fs::create_dir(&cfg).unwrap();
    let mut routes: Value = serde_json::from_str(include_str!("../config/routes.json")).unwrap();
    for route in routes.as_array_mut().unwrap() {
        if route["id"] == "grok-cli" {
            route["argv"] = json!([exe]);
        }
    }
    fs::write(
        cfg.join("routes.json"),
        serde_json::to_vec(&routes).unwrap(),
    )
    .unwrap();
    (temp, home)
}
fn command(temp: &TempDir, home: &Path) -> Command {
    let mut c = Command::new(binary());
    c.env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("MULTI_AI_CONFIG_DIR", temp.path().join("config"))
        .env("MULTI_AI_STATE_DIR", temp.path().join("state"));
    c
}

fn setup_kimi(body: &str) -> (TempDir, PathBuf) {
    let (temp, home) = setup("grok 1.0.67 (2fe064390d07) [stable]", "exit 77");
    let credentials = home.join(".kimi-code/credentials");
    fs::create_dir_all(&credentials).unwrap();
    fs::write(
        credentials.join("kimi-code.json"),
        r#"{"access_token":"dummy-access","refresh_token":"0","expires_at":1}"#,
    )
    .unwrap();
    let exe = temp.path().join("fake-kimi");
    fs::write(
        &exe,
        format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then printf '2.3.1\\n'; exit 0; fi\n{body}\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
    let cfg = temp.path().join("config/routes.json");
    let mut routes: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
    for route in routes.as_array_mut().unwrap() {
        if route["id"] == "kimi-cli-default" {
            route["argv"] = json!([exe]);
        }
    }
    fs::write(cfg, serde_json::to_vec(&routes).unwrap()).unwrap();
    (temp, home)
}

#[test]
fn kimi_generated_model_metadata_matches_coding_and_k3_contracts() {
    for (alias, context, has_effort) in [
        ("kimi-code/kimi-for-coding", 262144, false),
        ("kimi-code/kimi-for-coding-highspeed", 262144, false),
        ("kimi-code/k3", 1048576, true),
        ("kimi-code/k3-256k", 262144, true),
    ] {
        let body = r#"
cat "$KIMI_CODE_HOME/config.toml" >&2
for arg in "$@"; do
    case "$arg" in --auto|--yolo|--plan) exit 41;; esac
done
printf '%s\n' '{"role":"assistant","content":"{\"schema_version\":1,\"findings\":[],\"checked_scope\":[\"source.txt\"],\"uncovered_scope\":[],\"uncertainties\":[]}"}'
printf '%s\n' '{"role":"meta","type":"session.resume_hint","session_id":"dummy-complete"}'
"#;
        let (temp, home) = setup_kimi(body);
        let cfg = temp.path().join("config/routes.json");
        let mut routes: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        let route = routes
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|r| r["id"] == "kimi-cli-default")
            .unwrap();
        route["argv"]
            .as_array_mut()
            .unwrap()
            .extend([json!("--model"), json!(alias)]);
        fs::write(cfg, serde_json::to_vec(&routes).unwrap()).unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        fs::write(repo.join("source.txt"), "fixture\n").unwrap();
        let packet = temp.path().join("packet");
        assert!(
            command(&temp, &home)
                .args(["pack", "--root"])
                .arg(&repo)
                .args(["--input", "source.txt", "--outdir"])
                .arg(&packet)
                .output()
                .unwrap()
                .status
                .success()
        );
        let task = temp.path().join("task.txt");
        fs::write(&task, "Inspect source.txt").unwrap();
        let output = temp.path().join("out");
        let result = command(&temp, &home)
            .args(["delegate", "investigate", "kimi-cli-default", "--packet"])
            .arg(&packet)
            .arg("--prompt-file")
            .arg(&task)
            .arg("--outdir")
            .arg(&output)
            .args(["--timeout", "10", "--json"])
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&result.stdout).contains("no audited material reads"));
        let config = fs::read_to_string(find(&output, "stderr.raw").unwrap()).unwrap();
        assert!(
            config.contains(&format!("max_context_size = {context}\n")),
            "{alias}: {config}"
        );
        assert_eq!(
            config.contains("support_efforts = [\"low\",\"high\",\"max\"]"),
            has_effort
        );
        assert_eq!(config.contains("default_effort = \"low\""), has_effort);
        assert_eq!(config.contains("\neffort = \"low\"\n"), has_effort);
    }
}

#[test]
fn revoked_kimi_tombstone_is_unavailable_without_starting_model() {
    let (temp, home) = setup_kimi("touch \"$KIMI_CODE_HOME/model-started\"; exit 77");
    fs::write(
        home.join(".kimi-code/credentials/kimi-code.json"),
        r#"{"access_token":"","refresh_token":"","expires_at":0}"#,
    )
    .unwrap();
    let out = command(&temp, &home)
        .args(["doctor", "--only", "kimi-cli-default"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["routes"][0]["delegation"]["available"], false);
    assert_eq!(
        v["routes"][0]["delegation"]["credentials_configured"],
        false
    );
    assert!(!home.join(".kimi-code/model-started").exists());
}

#[test]
fn oauth_values_echoed_without_bearer_prefix_are_redacted() {
    let body = r#"
printf '%s\n' 'unexpected old value: dummy-old-refresh-sensitive' >&2
printf '%s\n' '{"access_token":"dummy-new-access-sensitive","refresh_token":"dummy-new-refresh-sensitive","expires_at":1}' > "$KIMI_CODE_HOME/credentials/kimi-code.json.tmp.$$"
mv "$KIMI_CODE_HOME/credentials/kimi-code.json.tmp.$$" "$KIMI_CODE_HOME/credentials/kimi-code.json"
printf '%s\n' 'unexpected new value: dummy-new-access-sensitive' >&2
printf '%s\n' '{"role":"assistant","content":null,"tool_calls":[{"function":{"name":"dummy-new-refresh-sensitive","arguments":"{}"}}]}'
exit 1
"#;
    let (temp, home) = setup_kimi(body);
    fs::write(home.join(".kimi-code/credentials/kimi-code.json"),r#"{"access_token":"dummy-old-access-sensitive","refresh_token":"dummy-old-refresh-sensitive","expires_at":1}"#).unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    fs::write(repo.join("source.txt"), "fixture\n").unwrap();
    let packet = temp.path().join("packet");
    let task = temp.path().join("task.txt");
    let output = temp.path().join("out");
    fs::write(&task, "Inspect source.txt").unwrap();
    assert!(
        command(&temp, &home)
            .arg("pack")
            .arg("--root")
            .arg(&repo)
            .args(["--input", "source.txt"])
            .arg("--outdir")
            .arg(&packet)
            .output()
            .unwrap()
            .status
            .success()
    );
    let out = command(&temp, &home)
        .args(["delegate", "investigate", "kimi-cli-default"])
        .arg("--packet")
        .arg(&packet)
        .arg("--prompt-file")
        .arg(&task)
        .arg("--outdir")
        .arg(&output)
        .args(["--timeout", "10", "--json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = fs::read_to_string(find(&output, "stderr.raw").unwrap()).unwrap();
    assert_eq!(stderr.matches("[REDACTED]").count(), 2);
    let public = format!(
        "{}{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
        stderr
    );
    assert!(!public.contains("dummy-old-refresh-sensitive"));
    assert!(!public.contains("dummy-new-access-sensitive"));
    assert!(!public.contains("dummy-new-refresh-sensitive"));
    assert!(public.contains("[REDACTED]"));
    let tools = fs::read_to_string(find(&output, "cli-tools.json").unwrap()).unwrap();
    assert!(!tools.contains("dummy-new-refresh-sensitive"));
}

#[test]
fn kimi_atomic_refresh_survives_cleanup_and_shares_concurrent_refresh_lock() {
    // Mimic the vendor's mkdir lock and tmp+rename credential refresh. All
    // credentials are dummy data. Distinct private homes must share this lock
    // and the atomic-write directory, including after their cleanup.
    let body = r#"
test -L "$KIMI_CODE_HOME/credentials" || exit 41
test -L "$KIMI_CODE_HOME/oauth" || exit 42
lock="$KIMI_CODE_HOME/oauth/kimi-code.lock"
while ! mkdir "$lock" 2>/dev/null; do sleep 0.02; done
trap 'rmdir "$lock"' EXIT
file="$KIMI_CODE_HOME/credentials/kimi-code.json"
n=$(sed -n 's/.*"refresh_token":"\([0-9]*\)".*/\1/p' "$file")
test -n "$n" || exit 43
sleep 0.1
n=$((n+1))
printf '{"access_token":"dummy-access","refresh_token":"%s","expires_at":1}' "$n" > "$file.tmp.$$"
mv "$file.tmp.$$" "$file"
printf '%s\n' '{"role":"assistant","content":"{\"schema_version\":1,\"findings\":[],\"checked_scope\":[\"source.txt\"],\"uncovered_scope\":[],\"uncertainties\":[]}"}'
printf '%s\n' '{"role":"meta","type":"session.resume_hint","session_id":"dummy-complete"}'
"#;
    let (temp, home) = setup_kimi(body);
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    fs::write(repo.join("source.txt"), "fixture\n").unwrap();
    let packet = temp.path().join("packet");
    let task = temp.path().join("task.txt");
    fs::write(&task, "Inspect source.txt").unwrap();
    assert!(
        command(&temp, &home)
            .arg("pack")
            .arg("--root")
            .arg(&repo)
            .args(["--input", "source.txt"])
            .arg("--outdir")
            .arg(&packet)
            .output()
            .unwrap()
            .status
            .success()
    );
    let invoke = |output: &str| {
        command(&temp, &home)
            .args(["delegate", "investigate", "kimi-cli-default"])
            .arg("--packet")
            .arg(&packet)
            .arg("--prompt-file")
            .arg(&task)
            .arg("--outdir")
            .arg(temp.path().join(output))
            .args(["--timeout", "30", "--json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    for output in ["sequential-one", "sequential-two"] {
        let out = invoke(output).wait_with_output().unwrap();
        assert!(!out.status.success());
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("no audited material reads"),
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let first = invoke("parallel-one");
    let second = invoke("parallel-two");
    for out in [
        first.wait_with_output().unwrap(),
        second.wait_with_output().unwrap(),
    ] {
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("no audited material reads"));
    }
    let auth: Value = serde_json::from_slice(
        &fs::read(home.join(".kimi-code/credentials/kimi-code.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(auth["refresh_token"], "4");
    assert!(home.join(".kimi-code/oauth").is_dir());
    assert!(!home.join(".kimi-code/oauth/kimi-code.lock").exists());
    // Mock workers did not use the broker, so their reports correctly fail
    // publication despite successful adapter execution and refresh writes.
    for output in [
        "sequential-one",
        "sequential-two",
        "parallel-one",
        "parallel-two",
    ] {
        assert!(!temp.path().join(output).join("latest.json").exists());
        let stderr =
            fs::read_to_string(find(&temp.path().join(output), "stderr.raw").unwrap()).unwrap();
        assert!(stderr.is_empty(), "{stderr}");
    }
}
#[test]
fn unknown_vendor_version_fails_closed_and_does_not_read_auth_contents() {
    let (temp, home) = setup("grok 99.0.0", "exit 77");
    // This invalid JSON must not be parsed by offline capability inspection.
    fs::write(home.join(".grok/auth.json"), "secret-credential-canary").unwrap();
    let out = command(&temp, &home)
        .args(["doctor", "--only", "grok-cli"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["routes"][0]["delegation"]["available"], false);
    assert_eq!(v["routes"][0]["delegation"]["version_validated"], false);
    assert!(!String::from_utf8_lossy(&out.stdout).contains("secret-credential-canary"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("受限委派已停用：本机 grok 99.0.0"),
        "{stderr}"
    );
    assert_eq!(v["warnings"].as_array().unwrap().len(), 1);
}

#[test]
fn offline_version_probe_does_not_wait_for_orphaned_stdout() {
    let (temp, home) = setup("grok 1.0.67 (2fe064390d07) [stable]", "exit 1");
    fs::write(
        temp.path().join("fake-grok"),
        "#!/bin/sh\n(sleep 20) &\nprintf '%s\\n' 'grok 1.0.67 (2fe064390d07) [stable]'\nexit 0\n",
    )
    .unwrap();
    let started = Instant::now();
    let out = command(&temp, &home)
        .args(["doctor", "--only", "grok-cli"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "version descendant held doctor open"
    );
}

fn find(path: &Path, name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(path).ok()? {
        let entry = entry.ok()?;
        if entry.file_name() == name {
            return Some(entry.path());
        }
        if entry.file_type().ok()?.is_dir()
            && let Some(path) = find(&entry.path(), name)
        {
            return Some(path);
        }
    }
    None
}
#[test]
fn adapter_uses_private_cwd_and_home_and_drops_project_environment() {
    let body = r#"
test "$HOME" != "$ORIGINAL_HOME" || exit 41
test -z "$PROJECT_SECRET_TOKEN" || exit 42
test -z "$NODE_OPTIONS" || exit 43
test -f "$GROK_HOME/config.toml" || exit 44
test -L "$GROK_HOME/auth.json" || exit 45
test "$GROK_CLAUDE_MCPS_ENABLED" = 0 || exit 46
test "$GROK_CURSOR_HOOKS_ENABLED" = 0 || exit 47
test ! -e "$PWD/.grok/config.toml" || exit 48
printf '%s\n' '{"type":"system","subtype":"init","model":"mock","tools":["search_tool","use_tool"],"mcp_servers":[{"name":"multi_ai"}]}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"{\"schema_version\":1,\"findings\":[],\"checked_scope\":[\"source.txt\"],\"uncovered_scope\":[],\"uncertainties\":[]}"}'
"#;
    let (temp, home) = setup("grok 1.0.67 (2fe064390d07) [stable]", body);
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    fs::write(repo.join("source.txt"), "fixture\n").unwrap();
    fs::create_dir(repo.join(".grok")).unwrap();
    fs::write(
        repo.join(".grok/config.toml"),
        "[hooks]\nmalicious = true\n",
    )
    .unwrap();
    let packet = temp.path().join("packet");
    let output = temp.path().join("out");
    let task = temp.path().join("task.txt");
    fs::write(&task, "Inspect source.txt").unwrap();
    assert!(
        command(&temp, &home)
            .arg("pack")
            .arg("--root")
            .arg(&repo)
            .args(["--input", "source.txt"])
            .arg("--outdir")
            .arg(&packet)
            .output()
            .unwrap()
            .status
            .success()
    );
    let out = command(&temp, &home)
        .env("ORIGINAL_HOME", &home)
        .env("PROJECT_SECRET_TOKEN", "must-never-forward")
        .env("NODE_OPTIONS", "must-never-forward")
        .current_dir(&repo)
        .args(["delegate", "investigate", "grok-cli"])
        .arg("--packet")
        .arg(&packet)
        .arg("--prompt-file")
        .arg(&task)
        .arg("--outdir")
        .arg(&output)
        .args(["--timeout", "10", "--json"])
        .output()
        .unwrap();
    // A mock may not publish a report without broker evidence, but reaching this
    // failure proves the child assertions and actual tool-table check passed.
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("no audited material reads"),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let tools: Value =
        serde_json::from_slice(&fs::read(find(&output, "cli-tools.json").unwrap()).unwrap())
            .unwrap();
    assert_eq!(tools["observed_tools"], json!(["search_tool", "use_tool"]));
    let isolation: Value =
        serde_json::from_slice(&fs::read(find(&output, "isolation.json").unwrap()).unwrap())
            .unwrap();
    assert_eq!(isolation["credential_contents_copied"], false);
    assert!(!output.join("latest.json").exists());
}

#[test]
fn timeout_kills_cli_descendants_and_stops_fallback_after_body() {
    let (temp, home) = setup("grok 1.0.67 (2fe064390d07) [stable]", "exit 1");
    let marker = temp.path().join("descendant-survived");
    let body = format!(
        "#!/bin/sh\nif [ \"$1\" = --version ]; then printf '%s\\n' 'grok 1.0.67 (2fe064390d07) [stable]'; exit 0; fi\nprintf '%s\\n' '{{\"type\":\"system\",\"subtype\":\"init\",\"tools\":[\"search_tool\",\"use_tool\"]}}'\nprintf '%s\\n' '{{\"type\":\"assistant\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"received partial body\"}}]}}}}'\n(sleep 5; touch '{}') &\nsleep 20\n",
        marker.display()
    );
    fs::write(temp.path().join("fake-grok"), body).unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    fs::write(repo.join("source.txt"), "fixture\n").unwrap();
    let packet = temp.path().join("packet");
    let output = temp.path().join("out");
    let task = temp.path().join("task.txt");
    fs::write(&task, "Inspect source.txt").unwrap();
    assert!(
        command(&temp, &home)
            .arg("pack")
            .arg("--root")
            .arg(&repo)
            .args(["--input", "source.txt"])
            .arg("--outdir")
            .arg(&packet)
            .output()
            .unwrap()
            .status
            .success()
    );
    let out = command(&temp, &home)
        .args(["delegate", "investigate", "grok-cli"])
        .arg("--packet")
        .arg(&packet)
        .arg("--prompt-file")
        .arg(&task)
        .arg("--outdir")
        .arg(&output)
        // Allow binary fingerprinting and parallel test startup to finish
        // before exercising the timeout after a real received body.
        .args(["--timeout", "3", "--fallback", "kimi-cli-default", "--json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("no fallback after response"));
    thread::sleep(Duration::from_millis(5100));
    assert!(!marker.exists(), "CLI descendant survived timeout");
    assert!(!output.join("latest.json").exists());
}

/// Run deliberately with the exact installed vendor CLI. The fake inference
/// server uses a dummy API key and loopback only; it inspects real request tool
/// schemas and asks the vendor to execute a forbidden Bash call.
#[test]
#[ignore = "optional installed Kimi boundary check; no credentials or external inference"]
fn installed_kimi_rejects_injected_native_tool_and_exposes_only_broker() {
    let version = Command::new("kimi")
        .arg("--version")
        .output()
        .expect("install Kimi 2.3.1 for this optional check");
    assert_eq!(String::from_utf8_lossy(&version.stdout).trim(), "2.3.1");
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("home");
    let vendor = home.join(".kimi-code");
    let work = temp.path().join("work");
    fs::create_dir_all(&vendor).unwrap();
    fs::create_dir(&work).unwrap();
    fs::create_dir(home.join("empty-skills")).unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    fs::write(repo.join("source.txt"), "fixture\n").unwrap();
    let packet = temp.path().join("packet");
    assert!(
        Command::new(binary())
            .env_clear()
            .env("HOME", &home)
            .env("PATH", "/usr/bin:/bin")
            .arg("pack")
            .arg("--root")
            .arg(&repo)
            .args(["--input", "source.txt"])
            .arg("--outdir")
            .arg(&packet)
            .output()
            .unwrap()
            .status
            .success()
    );
    let audit = temp.path().join("audit.json");
    fs::write(vendor.join("mcp.json"), serde_json::to_vec(&json!({"mcpServers":{"multi_ai":{"command":binary(),"args":["_broker","--packet",packet,"--audit",audit],"enabledTools":["list","search","read"]}}})).unwrap()).unwrap();
    let agent = temp.path().join("agent.md");
    fs::write(&agent, "---\nname: multi-ai-worker\ndescription: Restricted investigator\ntools: [mcp__multi_ai__list, mcp__multi_ai__search, mcp__multi_ai__read]\ndisallowedTools: [select_tools]\nsubagents: []\n---\nUse only authorized broker tools.\n").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = requests.clone();
    let marker = temp.path().join("forbidden-shell-ran");
    let attempted_command = format!("touch {}", marker.display());
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(40);
        while Instant::now() < deadline && captured.lock().unwrap().len() < 3 {
            let Ok((mut stream, _)) = listener.accept() else {
                thread::sleep(Duration::from_millis(10));
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut header_end = None;
            let mut length = 0;
            loop {
                let mut buf = [0_u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buf[..n]);
                if header_end.is_none()
                    && let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n")
                {
                    let headers = String::from_utf8_lossy(&bytes[..pos]);
                    length = headers
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|s| s.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    header_end = Some(pos + 4);
                }
                if header_end.is_some_and(|p| bytes.len() >= p + length) {
                    break;
                }
            }
            let Some(start) = header_end.filter(|start| bytes.len() >= start + length) else {
                continue;
            };
            if bytes.starts_with(b"GET ") {
                let body = r#"{"object":"list","data":[{"id":"mock","object":"model"}]}"#;
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
                continue;
            }
            let value: Value = serde_json::from_slice(&bytes[start..start + length]).unwrap();
            let index = {
                let mut data = captured.lock().unwrap();
                data.push(value);
                data.len()
            };
            let delta = if index == 1 {
                json!({"role":"assistant","tool_calls":[{"index":0,"id":"forbidden-call","type":"function","function":{"name":"Bash","arguments":serde_json::to_string(&json!({"command":attempted_command})).unwrap()}}]})
            } else if index == 2 {
                json!({"role":"assistant","tool_calls":[{"index":0,"id":"forbidden-disclosure","type":"function","function":{"name":"select_tools","arguments":"{\"tool_names\":[\"Bash\"]}"}}]})
            } else {
                json!({"role":"assistant","content":"done"})
            };
            let body = format!(
                "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                json!({"id":"mock","object":"chat.completion.chunk","created":0,"model":"mock","choices":[{"index":0,"delta":delta,"finish_reason":null}]}),
                json!({"id":"mock","object":"chat.completion.chunk","created":0,"model":"mock","choices":[{"index":0,"delta":{},"finish_reason":if index<=2 {"tool_calls"}else{"stop"}}]})
            );
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
    });
    fs::write(vendor.join("config.toml"), format!("default_model = \"mock\"\ntelemetry = false\nmerge_all_available_skills = false\nbuiltin_product_skills = false\n[providers.mock]\ntype = \"openai\"\nbase_url = \"http://{address}/v1\"\napi_key = \"dummy-only-loopback\"\n[models.mock]\nprovider = \"mock\"\nmodel = \"mock\"\nmax_context_size = 65536\ncapabilities = [\"tool_use\"]\n[thinking]\nenabled = false\n[loop_control]\nmax_attempts_per_step = 1\nmax_steps_per_turn = 3\n")).unwrap();
    let exe = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|p| p.join("kimi"))
        .find(|p| p.is_file())
        .unwrap();
    let mut child = Command::new(exe)
        .env_clear()
        .env("HOME", &home)
        .env("KIMI_CODE_HOME", &vendor)
        .env("KIMI_DISABLE_TELEMETRY", "1")
        .env("PATH", "/usr/bin:/bin")
        .current_dir(&work)
        .arg("--agent-file")
        .arg(&agent)
        .arg("--skills-dir")
        .arg(home.join("empty-skills"))
        .args(["--output-format", "stream-json", "-p", "Return done."])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if started.elapsed() > Duration::from_secs(35) {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    let output = child.wait_with_output().unwrap();
    let _ = server.join();
    let data = requests.lock().unwrap();
    assert!(
        !data.is_empty(),
        "no local inference request: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let names: Vec<_> = data[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 3, "actual tools: {names:?}");
    assert!(names.iter().all(|n| {
        [
            "mcp__multi_ai__list",
            "mcp__multi_ai__search",
            "mcp__multi_ai__read",
        ]
        .contains(n)
    }));
    assert!(!marker.exists(), "forbidden native Bash call executed");
    assert_eq!(
        data.len(),
        3,
        "vendor did not return a rejected tool result"
    );
    assert!(
        data[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "tool"
                && m["content"].as_str().is_some_and(|s| s.contains("Bash")
                    || s.contains("not found")
                    || s.contains("not allowed")))
    );
    assert!(
        data[2]["messages"].as_array().unwrap().iter().any(|m| {
            m["role"] == "tool"
                && m["content"].as_str().is_some_and(|s| {
                    s.contains("select_tools")
                        && (s.contains("not active")
                            || s.contains("not allowed")
                            || s.contains("not found"))
                })
        }),
        "select_tools did not receive a policy rejection"
    );
}

#[test]
fn restricted_grok_preserves_capacity_diagnostic_through_delegate() {
    let (temp, home) = setup(
        "grok 1.0.67 (2fe064390d07) [stable]",
        r#"
printf '%s\n' '{"type":"system","subtype":"init","tools":["search_tool","use_tool"],"mcp_servers":[{"name":"multi_ai"}]}'
printf '%s\n' '{"role":"tool","content":"401 Unauthorized"}'
printf '%s\n' '{"type":"result","is_error":true,"errors":["Selected model is overloaded"]}'
exit 1
"#,
    );
    fs::create_dir(temp.path().join("repo")).unwrap();
    fs::write(temp.path().join("repo/source.txt"), "fictional fixture").unwrap();
    fs::write(temp.path().join("task"), "Inspect source.txt").unwrap();
    let output = command(&temp, &home)
        .arg("pack")
        .arg("--root")
        .arg(temp.path().join("repo"))
        .args(["--input", "source.txt", "--outdir"])
        .arg(temp.path().join("packet"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let output = command(&temp, &home)
        .args(["delegate", "investigate", "grok-cli", "--packet"])
        .arg(temp.path().join("packet"))
        .arg("--prompt-file")
        .arg(temp.path().join("task"))
        .arg("--outdir")
        .arg(temp.path().join("out"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    let output = command(&temp, &home)
        .args(["stats", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stats: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stats["routes"][0]["failures"]["capacity"], 1, "{stats}");
    assert!(!temp.path().join("state/review/state.json").exists());
}

#[test]
fn restricted_grok_success_breaks_auth_streak_before_result_validation() {
    let (temp, home) = setup(
        "grok 1.0.67 (2fe064390d07) [stable]",
        r#"
printf '%s\n' '{"type":"system","subtype":"init","tools":["search_tool","use_tool"]}'
printf '%s\n' '{"type":"result","is_error":false,"subtype":"success","result":"not a valid investigation report"}'
"#,
    );
    fs::create_dir(temp.path().join("repo")).unwrap();
    fs::write(temp.path().join("repo/source.txt"), "fictional fixture").unwrap();
    fs::write(temp.path().join("task"), "Inspect source.txt").unwrap();
    assert!(
        command(&temp, &home)
            .arg("pack")
            .arg("--root")
            .arg(temp.path().join("repo"))
            .args(["--input", "source.txt", "--outdir"])
            .arg(temp.path().join("packet"))
            .output()
            .unwrap()
            .status
            .success()
    );
    fs::create_dir_all(temp.path().join("state/review")).unwrap();
    fs::write(
        temp.path().join("state/review/state.json"),
        r#"{"pools":{"xai":{"until":1,"kind":"auth","auth_failures":1}}}"#,
    )
    .unwrap();
    let output = command(&temp, &home)
        .args(["delegate", "investigate", "grok-cli", "--packet"])
        .arg(temp.path().join("packet"))
        .arg("--prompt-file")
        .arg(temp.path().join("task"))
        .arg("--outdir")
        .arg(temp.path().join("out"))
        .output()
        .unwrap();
    assert!(!output.status.success()); // Output schema validation still fails.
    let state: Value =
        serde_json::from_slice(&fs::read(temp.path().join("state/review/state.json")).unwrap())
            .unwrap();
    assert!(state["pools"]["xai"].is_null(), "{state}");
}

#[test]
fn grok_release_channel_suffix_does_not_change_the_validated_build() {
    for printed in [
        "grok 1.0.67 (2fe064390d07)",
        "grok 1.0.67 (2fe064390d07) [stable]",
    ] {
        let (temp, home) = setup(printed, "exit 77");
        let out = command(&temp, &home)
            .args(["doctor", "--only", "grok-cli"])
            .output()
            .unwrap();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            v["routes"][0]["delegation"]["version_validated"], true,
            "{printed}"
        );
        assert!(v["warnings"].as_array().unwrap().is_empty(), "{printed}");
    }
}

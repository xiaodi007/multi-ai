use multi_ai::{
    broker::{MAX_TOOL_CALLS, ReadOnlyBroker},
    packet::{self, Packet},
    runtime::OutputLock,
};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::symlink, path::Path};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

fn material(root: &Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/main.rs"),
        "fn run() {\n    // Hidden fixture answer: repair-order-719\n}\n",
    )
    .unwrap();
    fs::write(root.join("README.md"), "Fixture documentation\n").unwrap();
}

#[test]
fn snapshot_is_stable_deduplicated_and_binds_current_worktree() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("repo");
    material(&root);
    let one = packet::create(
        &root,
        &["README.md".into(), "README.md".into()],
        &["src".into()],
        &t.path().join("one"),
    )
    .unwrap();
    let two = packet::create(
        &root,
        &["README.md".into()],
        &["src".into()],
        &t.path().join("two"),
    )
    .unwrap();
    assert_eq!(one.hash(), two.hash());
    assert_eq!(one.manifest.files.len(), 2);
    fs::write(root.join("README.md"), "Uncommitted replacement\n").unwrap();
    assert_eq!(one.read("README.md").unwrap(), "Fixture documentation\n");
    let three = packet::create(
        &root,
        &["README.md".into()],
        &["src".into()],
        &t.path().join("three"),
    )
    .unwrap();
    assert_ne!(one.hash(), three.hash());
    assert!(one.read("../repo/README.md").is_err());
    assert!(one.read("src/missing.rs").is_err());
    assert!(
        !fs::read_to_string(one.dir.join("manifest.json"))
            .unwrap()
            .contains(&root.to_string_lossy().to_string())
    );
}

#[test]
fn snapshot_excludes_credentials_extensions_binary_and_artifacts() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("repo");
    material(&root);
    for dir in [
        ".git",
        ".claude",
        ".grok",
        ".kimi",
        ".kimi-code",
        "node_modules",
        "target",
        "vendor",
    ] {
        fs::create_dir(root.join(dir)).unwrap();
        fs::write(root.join(dir).join("hidden.txt"), "excluded").unwrap();
    }
    fs::write(root.join(".env"), "API_KEY=fixture").unwrap();
    fs::write(root.join("client.pem"), "fixture").unwrap();
    fs::write(root.join("picture.bin"), b"\0binary").unwrap();
    fs::write(root.join("bad.txt"), b"\xff").unwrap();
    fs::write(
        root.join("exposed.txt"),
        "const KEY = 'sk-fixturekeylongenough123456';",
    )
    .unwrap();
    let packet = packet::create(&root, &[], &[".".into()], &t.path().join("packet")).unwrap();
    assert_eq!(packet.manifest.files.len(), 2);
    assert_eq!(packet.manifest.exclusions.len(), 13);
    assert!(
        packet
            .manifest
            .exclusions
            .iter()
            .any(|e| e.path == ".env" && e.reason == "credential-file")
    );
}

#[test]
fn snapshot_rejects_symlinks_escapes_oversize_and_empty_scope_without_output() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("repo");
    material(&root);
    fs::write(t.path().join("outside.txt"), "outside").unwrap();
    symlink(t.path().join("outside.txt"), root.join("escape.txt")).unwrap();
    for (name, input) in [("sym", "escape.txt"), ("parent", "../outside.txt")] {
        let out = t.path().join(name);
        assert!(packet::create(&root, &[input.into()], &[], &out).is_err());
        assert!(!out.exists());
    }
    fs::write(
        root.join("large.txt"),
        vec![b'a'; packet::MAX_FILE_BYTES + 1],
    )
    .unwrap();
    let out = t.path().join("oversize");
    assert!(packet::create(&root, &["large.txt".into()], &[], &out).is_err());
    assert!(!out.exists());
    assert!(packet::create(&root, &[], &[], &t.path().join("empty")).is_err());
    let fifo = root.join("fifo");
    let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(packet::create(&root, &["fifo".into()], &[], &t.path().join("fifo-output")).is_err());
}

#[test]
fn tampering_manifest_content_or_blob_symlink_is_rejected() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("repo");
    material(&root);
    let packet =
        packet::create(&root, &["README.md".into()], &[], &t.path().join("packet")).unwrap();
    let blob = packet.dir.join(&packet.manifest.files[0].blob);
    let original = fs::read(&blob).unwrap();
    fs::write(&blob, "changed").unwrap();
    assert!(packet.read("README.md").is_err());
    assert!(Packet::load(&packet.dir).is_err());
    fs::write(&blob, &original).unwrap();
    let manifest_path = packet.dir.join("manifest.json");
    let original_manifest = fs::read(&manifest_path).unwrap();
    let mut manifest: Value = serde_json::from_slice(&original_manifest).unwrap();
    manifest["files"][0]["path"] = json!("../outside.txt");
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(packet.verify().is_err());
    fs::write(&manifest_path, original_manifest).unwrap();
    fs::remove_file(&blob).unwrap();
    let outside = t.path().join("outside.txt");
    fs::write(&outside, &original).unwrap();
    symlink(outside, blob).unwrap();
    assert!(packet.read("README.md").is_err());
}

#[test]
fn aggregate_file_count_and_size_limits_fail_before_publication() {
    let t = tempfile::tempdir().unwrap();
    let count_root = t.path().join("count");
    fs::create_dir(&count_root).unwrap();
    for n in 0..=packet::MAX_FILES {
        fs::write(count_root.join(format!("{n:04}.txt")), "x").unwrap();
    }
    let out = t.path().join("count-packet");
    let error = packet::create(&count_root, &[], &[".".into()], &out).unwrap_err();
    assert!(error.to_string().contains("2,000 files"));
    assert!(!out.exists());
    let size_root = t.path().join("size");
    fs::create_dir(&size_root).unwrap();
    for n in 0..17 {
        fs::write(
            size_root.join(format!("{n:04}.txt")),
            vec![b'a'; packet::MAX_FILE_BYTES],
        )
        .unwrap();
    }
    let out = t.path().join("size-packet");
    let error = packet::create(&size_root, &[], &[".".into()], &out).unwrap_err();
    assert!(error.to_string().contains("8 MiB"));
    assert!(!out.exists());
}

#[test]
fn output_lock_conflict_changes_neither_owner_nor_material() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("repo");
    material(&root);
    let output = t.path().join("packet");
    let lock = OutputLock::acquire(&[(output.clone(), true)]).unwrap();
    let error = packet::create(&root, &["README.md".into()], &[], &output).unwrap_err();
    assert!(error.to_string().contains("output_busy"));
    assert!(!output.exists());
    assert!(packet::create(&root, &["README.md".into()], &[], &output.join("nested")).is_err());
    assert!(packet::create(&root, &["README.md".into()], &[], &t.path().join("other")).is_ok());
    drop(lock);
    packet::create(&root, &["README.md".into()], &[], &output).unwrap();
}

#[test]
fn broker_only_reads_packet_and_persists_budget_across_reconnects() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("repo");
    material(&root);
    let packet = packet::create(&root, &[], &["src".into()], &t.path().join("packet")).unwrap();
    let audit = t.path().join("audit.json");
    let broker = ReadOnlyBroker::new(packet.clone(), &audit).unwrap();
    let list = broker.invoke("list", json!({})).unwrap();
    assert_eq!(list["files"][0]["path"], "src/main.rs");
    let search = broker
        .invoke("search", json!({"query":"repair-order-719"}))
        .unwrap();
    assert_eq!(search["matches"][0]["line"], 2);
    let read = broker
        .invoke(
            "read",
            json!({"path":"src/main.rs","start_line":2,"end_line":2}),
        )
        .unwrap();
    assert!(
        read["content"]
            .as_str()
            .unwrap()
            .contains("repair-order-719")
    );
    assert!(
        broker
            .invoke("read", json!({"path":"../repo/README.md"}))
            .is_err()
    );
    assert!(
        broker
            .invoke("shell", json!({"command":"cat /etc/passwd"}))
            .is_err()
    );
    assert!(
        broker
            .invoke("search", json!({"query":"x", "command":"echo"}))
            .is_err()
    );
    assert!(ReadOnlyBroker::new(packet.clone(), &packet.dir.join("audit.json")).is_err());
    let used = broker.audit().unwrap().tool_calls;
    drop(broker);
    let broker = ReadOnlyBroker::new(packet, &audit).unwrap();
    assert_eq!(broker.audit().unwrap().tool_calls, used);
    for _ in used..MAX_TOOL_CALLS {
        broker.invoke("list", json!({})).unwrap();
    }
    assert!(
        broker
            .invoke("list", json!({}))
            .unwrap_err()
            .to_string()
            .contains("budget exhausted")
    );
    assert!(broker.audit().unwrap().exhausted);
}

#[test]
fn broker_rejects_tamper_and_return_bytes_budget_without_truncating() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("repo");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("huge.txt"), "q".repeat(100_000)).unwrap();
    let packet =
        packet::create(&root, &["huge.txt".into()], &[], &t.path().join("packet")).unwrap();
    let broker = ReadOnlyBroker::new(packet.clone(), &t.path().join("audit.json")).unwrap();
    broker.invoke("read", json!({"path":"huge.txt"})).unwrap();
    broker.invoke("read", json!({"path":"huge.txt"})).unwrap();
    assert!(broker.invoke("read", json!({"path":"huge.txt"})).is_err());
    assert!(broker.audit().unwrap().exhausted);
    let other = ReadOnlyBroker::new(packet.clone(), &t.path().join("other.json")).unwrap();
    fs::write(packet.dir.join(&packet.manifest.files[0].blob), "modified").unwrap();
    assert!(other.invoke("list", json!({})).is_err());
}

#[tokio::test]
async fn stdio_mcp_exposes_only_three_tools_and_reads_hidden_fixture() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("repo");
    material(&root);
    let packet = packet::create(&root, &[], &["src".into()], &t.path().join("packet")).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_multi-ai"))
        .arg("_broker")
        .arg("--packet")
        .arg(&packet.dir)
        .arg("--audit")
        .arg(t.path().join("audit.json"))
        .env("HOME", t.path())
        .env("MULTI_AI_STATE_DIR", t.path().join("state"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    async fn request(
        input: &mut tokio::process::ChildStdin,
        lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
        value: Value,
    ) -> Value {
        input
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
        input.flush().await.unwrap();
        let id = value["id"].clone();
        loop {
            let line = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .expect("MCP response");
            let reply: Value = serde_json::from_str(&line).unwrap();
            if reply["id"] == id {
                return reply;
            }
        }
    }
    let init = request(&mut input,&mut lines,json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).await;
    assert!(init.get("error").is_none(), "{init}");
    input
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await
        .unwrap();
    let listed = request(
        &mut input,
        &mut lines,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )
    .await;
    let names = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["list", "search", "read"]);
    let found = request(&mut input,&mut lines,json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search","arguments":{"query":"repair-order-719"}}})).await;
    assert!(
        found["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("repair-order-719")
    );
    let denied = request(&mut input,&mut lines,json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"read","arguments":{"path":"/etc/passwd"}}})).await;
    assert_eq!(denied["result"]["isError"], true);
    drop(input);
    let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
}

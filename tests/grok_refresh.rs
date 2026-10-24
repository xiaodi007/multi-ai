//! Dummy Grok refresh regression: no network or user credentials.
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt, symlink},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

const GROK_VERSION: &str = "grok 1.0.67 (2fe064390d07) [stable]";
const USER_CONFIG: &[u8] = b"[models]\ndefault = \"user-config-must-stay\"\n";

fn auth_value(sequence: u64) -> Value {
    json!({"xai::oauth": {
        "access_token": format!("dummy-grok-access-{sequence}-sensitive"),
        "refresh_token": format!("dummy-grok-refresh-{sequence}-sensitive"),
        "refresh_sequence": sequence
    }})
}

// The shell wrapper re-enters this test binary, avoiding a compiler or scripting
// runtime dependency. Only that wrapper supplies the dummy-root guard.
#[test]
#[ignore = "private subprocess helper for the dummy Grok refresh regression"]
fn fake_grok_refresh_worker() {
    let Ok(root) = std::env::var("MULTI_AI_DUMMY_GROK_ROOT") else {
        return;
    };
    let root = fs::canonicalize(root).unwrap();
    let auth = PathBuf::from(std::env::var("GROK_AUTH_PATH").unwrap());
    assert!(auth.starts_with(&root), "worker must only read dummy auth");
    assert_eq!(auth, fs::canonicalize(&auth).unwrap());
    assert!(
        !fs::symlink_metadata(&auth)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    let vendor = PathBuf::from(std::env::var("GROK_HOME").unwrap());
    assert!(!home.starts_with(&root));
    assert_eq!(fs::canonicalize(vendor.join("auth.json")).unwrap(), auth);
    let parent = auth.parent().unwrap();
    let session_lock_path = parent.join(".multi-ai-session.lock");
    let session_lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&session_lock_path)
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(session_lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        -1,
        "worker started without the shared credential-session guard"
    );
    assert_eq!(
        std::io::Error::last_os_error().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let session_metadata = session_lock.metadata().unwrap();
    let initial: Value = serde_json::from_slice(&fs::read(&auth).unwrap()).unwrap();
    let initial_sequence = initial["xai::oauth"]["refresh_sequence"].as_u64().unwrap();
    let lock_path = parent.join("auth.json.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_path)
        .unwrap();
    let attempt = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(
        attempt, 0,
        "session guard must leave the native refresh flock free"
    );
    thread::sleep(Duration::from_millis(100));
    let old: Value = serde_json::from_slice(&fs::read(&auth).unwrap()).unwrap();
    let sequence = old["xai::oauth"]["refresh_sequence"].as_u64().unwrap() + 1;
    let new = auth_value(sequence);
    let temporary = parent.join(format!("auth.json.tmp.{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .unwrap();
    file.write_all(&serde_json::to_vec(&new).unwrap()).unwrap();
    file.sync_all().unwrap();
    fs::rename(&temporary, &auth).unwrap();
    let metadata = lock.metadata().unwrap();
    fs::write(
        parent.join(format!("receipt-{sequence}.json")),
        serde_json::to_vec(&json!({
            "auth": auth, "lock": lock_path, "home": home, "vendor": vendor,
            "cwd": std::env::current_dir().unwrap(), "initial_sequence": initial_sequence,
            "sequence": sequence,
            "lock_device": metadata.dev(), "lock_inode": metadata.ino(),
            "session_lock": session_lock_path,
            "session_lock_device": session_metadata.dev(),
            "session_lock_inode": session_metadata.ino()
        }))
        .unwrap(),
    )
    .unwrap();
    let echoes = format!(
        "old {} {}; new {} {}",
        old["xai::oauth"]["access_token"].as_str().unwrap(),
        old["xai::oauth"]["refresh_token"].as_str().unwrap(),
        new["xai::oauth"]["access_token"].as_str().unwrap(),
        new["xai::oauth"]["refresh_token"].as_str().unwrap()
    );
    eprintln!("{echoes}");
    // libtest prints the helper's test name without a trailing newline first.
    // Keep every vendor frame on its own NDJSON line.
    println!();
    println!(
        "{}",
        json!({"type":"system","subtype":"init","model":"mock",
        "tools":["search_tool","use_tool"],"skills":[],"slash_commands":[],
        "mcp_servers":[{"name":"multi_ai","status":"connected"}]})
    );
    println!(
        "{}",
        json!({"type":"assistant","message":{"role":"assistant",
        "content":[{"type":"text","text":echoes}]}})
    );
    println!(
        "{}",
        json!({"type":"result","subtype":"success","is_error":false,
        "result":json!({"schema_version":1,"findings":[],"checked_scope":["source.txt"],
            "uncovered_scope":[],"uncertainties":[]}).to_string()})
    );
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn command(root: &Path, home: &Path, auth_alias: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_multi-ai"));
    command
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("GROK_AUTH_PATH", auth_alias)
        .env("MULTI_AI_CONFIG_DIR", root.join("config"))
        .env("MULTI_AI_STATE_DIR", root.join("state"));
    command
}

fn inspect_output_tree(path: &Path, output: &mut String) {
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            inspect_output_tree(&entry.path(), output);
        } else {
            output.push_str(&String::from_utf8_lossy(&fs::read(entry.path()).unwrap()));
        }
    }
}

fn assert_scrubbed(root: &Path, name: &str, out: Output) {
    // This helper intentionally does not read the broker. A successful CLI must
    // still fail publication, proving credential refresh cannot bypass evidence.
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("no audited material reads"));
    let output = root.join(name);
    assert!(!output.join("latest.json").exists());
    let mut logs = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    inspect_output_tree(&output, &mut logs);
    assert!(
        !logs.contains("dummy-grok-"),
        "dummy OAuth values leaked in {name}"
    );
    assert!(logs.matches("[REDACTED]").count() >= 8);
}

fn fixture() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let home = root.join("home");
    let shared = root.join("shared-auth");
    let config = root.join("config");
    for path in [
        home.join(".grok"),
        shared.clone(),
        config.clone(),
        root.join("repo"),
    ] {
        fs::create_dir_all(path).unwrap();
    }
    let auth = shared.join("auth.json");
    fs::write(&auth, serde_json::to_vec(&auth_value(0)).unwrap()).unwrap();
    let auth_alias = root.join("auth-alias.json");
    symlink(&auth, &auth_alias).unwrap();
    symlink(&auth_alias, home.join(".grok/auth.json")).unwrap();
    fs::write(home.join(".grok/config.toml"), USER_CONFIG).unwrap();
    let fake = root.join("fake-grok");
    fs::write(&fake, format!(
        "#!/bin/sh\nif [ \"$1\" = --version ]; then printf '%s\\n' '{GROK_VERSION}'; exit 0; fi\nMULTI_AI_DUMMY_GROK_ROOT={} exec {} --exact fake_grok_refresh_worker --ignored --nocapture --test-threads=1\n",
        shell_quote(root), shell_quote(&std::env::current_exe().unwrap())
    )).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
    let mut routes: Value = serde_json::from_str(include_str!("../config/routes.json")).unwrap();
    for route in routes.as_array_mut().unwrap() {
        if route["id"] == "grok-cli" {
            route["argv"] = json!([fake, "--model", "mock"]);
        }
    }
    fs::write(
        config.join("routes.json"),
        serde_json::to_vec(&routes).unwrap(),
    )
    .unwrap();
    fs::write(root.join("repo/source.txt"), "fixture\n").unwrap();
    fs::write(root.join("task.txt"), "Inspect source.txt").unwrap();
    let packed = command(root, &home, &auth_alias)
        .arg("pack")
        .arg("--root")
        .arg(root.join("repo"))
        .args(["--input", "source.txt"])
        .arg("--outdir")
        .arg(root.join("packet"))
        .output()
        .unwrap();
    assert!(
        packed.status.success(),
        "{}",
        String::from_utf8_lossy(&packed.stderr)
    );
    temp
}

fn invoke(root: &Path, name: &str, timeout: &str) -> Child {
    command(root, &root.join("home"), &root.join("auth-alias.json"))
        .args(["delegate", "investigate", "grok-cli"])
        .arg("--packet")
        .arg(root.join("packet"))
        .arg("--prompt-file")
        .arg(root.join("task.txt"))
        .arg("--outdir")
        .arg(root.join(name))
        .args(["--timeout", timeout, "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

#[test]
fn grok_atomic_refresh_uses_shared_auth_and_lock_and_scrubs_both_generations() {
    let temp = fixture();
    let root = temp.path();
    let home = root.join("home");
    let shared = root.join("shared-auth");
    let auth = shared.join("auth.json");
    let auth_alias = root.join("auth-alias.json");
    for (sequence, name) in [(1, "sequential-one"), (2, "sequential-two")] {
        assert_scrubbed(
            root,
            name,
            invoke(root, name, "10").wait_with_output().unwrap(),
        );
        let value: Value = serde_json::from_slice(&fs::read(&auth).unwrap()).unwrap();
        assert_eq!(value, auth_value(sequence));
    }
    let first = invoke(root, "parallel-one", "10");
    let second = invoke(root, "parallel-two", "10");
    assert_scrubbed(root, "parallel-one", first.wait_with_output().unwrap());
    assert_scrubbed(root, "parallel-two", second.wait_with_output().unwrap());
    let value: Value = serde_json::from_slice(&fs::read(&auth).unwrap()).unwrap();
    assert_eq!(value, auth_value(4), "concurrent refresh lost an update");
    assert_eq!(
        fs::read(home.join(".grok/config.toml")).unwrap(),
        USER_CONFIG
    );
    assert!(
        fs::symlink_metadata(&auth_alias)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        fs::symlink_metadata(home.join(".grok/auth.json"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let auth = fs::canonicalize(&auth).unwrap();
    let lock = auth.parent().unwrap().join("auth.json.lock");
    let metadata = fs::metadata(&lock).unwrap();
    let session_lock = auth.parent().unwrap().join(".multi-ai-session.lock");
    let session_metadata = fs::metadata(&session_lock).unwrap();
    assert_ne!(session_lock, lock);
    for sequence in 1..=4 {
        let receipt: Value = serde_json::from_slice(
            &fs::read(shared.join(format!("receipt-{sequence}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(receipt["auth"], json!(auth));
        assert_eq!(receipt["lock"], json!(lock));
        assert_eq!(receipt["lock_device"], metadata.dev());
        assert_eq!(receipt["lock_inode"], metadata.ino());
        assert_eq!(receipt["sequence"], sequence);
        assert_eq!(receipt["initial_sequence"], sequence - 1);
        assert_eq!(receipt["session_lock"], json!(session_lock));
        assert_eq!(receipt["session_lock_device"], session_metadata.dev());
        assert_eq!(receipt["session_lock_inode"], session_metadata.ino());
        for field in ["home", "vendor", "cwd"] {
            assert!(
                !Path::new(receipt[field].as_str().unwrap()).exists(),
                "private {field} survived cleanup"
            );
        }
    }
    assert_eq!(
        fs::metadata(&auth).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        !fs::read_dir(shared)
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e
                .file_name()
                .to_string_lossy()
                .starts_with("auth.json.tmp."))
    );
}

#[test]
fn occupied_shared_session_guard_times_out_without_starting_worker() {
    let temp = fixture();
    let root = temp.path();
    let shared = root.join("shared-auth");
    let guard = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(shared.join(".multi-ai-session.lock"))
        .unwrap();
    assert_eq!(unsafe { libc::flock(guard.as_raw_fd(), libc::LOCK_EX) }, 0);
    let started = Instant::now();
    let out = invoke(root, "guard-timeout", "1")
        .wait_with_output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "session wait ignored the total budget"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(shared.join("auth.json")).unwrap()).unwrap(),
        auth_value(0)
    );
    assert!(
        !shared.join("auth.json.lock").exists(),
        "worker started despite occupied session guard"
    );
    assert!(
        !fs::read_dir(&shared)
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with("receipt-"))
    );
    assert_eq!(
        fs::read(root.join("home/.grok/config.toml")).unwrap(),
        USER_CONFIG
    );
    assert!(!root.join("guard-timeout/latest.json").exists());
    let mut logs = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    inspect_output_tree(&root.join("guard-timeout"), &mut logs);
    assert!(!logs.contains("dummy-grok-"));
}

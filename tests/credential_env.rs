use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const EVENT_ID: &str = "evt_00000000-0000-4000-8000-000000000001";
const BODY: &str = include_str!("../fixtures/meeting.completed.json");
const FILE_TOKEN: &str = "file-bearer-token-for-isolated-tests-0001";
const PROCESS_TOKEN: &str = "process-bearer-token-for-isolated-tests-02";

struct Sandbox {
    _temp: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
    address: SocketAddr,
}
impl Sandbox {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        // macOS's ambient temp directory may itself be reached through a symlink.
        let root = temp.path().canonicalize().unwrap();
        let selected = root.join("selected");
        fs::create_dir(&selected).unwrap();
        let config = selected.join("config.json");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        fs::write(
            &config,
            serde_json::to_vec(&serde_json::json!({
                "listen": address.to_string(),
                "data_dir": "data",
                "min_free_bytes": 0,
                "destinations": []
            }))
            .unwrap(),
        )
        .unwrap();
        Self {
            _temp: temp,
            root,
            config,
            address,
        }
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aurora-webhook"));
        command
            .env_clear()
            .current_dir(&self.root)
            .arg("--config")
            .arg(&self.config);
        command
    }
    fn env_path(&self) -> PathBuf {
        self.config.parent().unwrap().join(".env")
    }
    fn credentials(&self, path: &Path, key: u8, token: &str) {
        private_file(
            path,
            &format!(
                "# Synthetic receiver test credentials, never app setup\nexport AURORA_WEBHOOK_SECRET='whsec_{}'\nTOKEN_PART={}\nAURORA_BEARER_TOKEN=\"${{TOKEN_PART}}\" # dotenv interpolation\nQUOTED_VALUE=\"line one\\nline two\"\nLITERAL_VALUE='$(not-a-shell-command)'\n",
                STANDARD.encode([key; 32]),
                token
            ),
        );
    }
    fn apply(&self) {
        success(
            self.command()
                .args(["apply", "--approve"])
                .output()
                .unwrap(),
        );
    }
    fn serve(&self, explicit: Option<&Path>, inherited: &[(&str, &str)]) -> Receiver {
        self.apply();
        let mut command = self.command();
        if let Some(path) = explicit {
            command.arg("--env-file").arg(path);
        }
        for (key, value) in inherited {
            command.env(key, value);
        }
        let mut receiver = Receiver {
            child: command
                .arg("serve")
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
            address: self.address,
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = receiver.child.try_wait().unwrap() {
                let mut error = String::new();
                receiver
                    .child
                    .stderr
                    .take()
                    .unwrap()
                    .read_to_string(&mut error)
                    .unwrap();
                panic!("receiver failed startup ({status}): {error}");
            }
            if let Ok(mut stream) =
                TcpStream::connect_timeout(&self.address, Duration::from_millis(100))
            {
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .write_all(
                        b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                    )
                    .unwrap();
                let mut response = String::new();
                stream.read_to_string(&mut response).unwrap();
                if response.starts_with("HTTP/1.1 200 ") {
                    return receiver;
                }
            }
            assert!(Instant::now() < deadline, "receiver startup timed out");
            thread::sleep(Duration::from_millis(20));
        }
    }
}
struct Receiver {
    child: Child,
    address: SocketAddr,
}
impl Receiver {
    fn send(&self, key: u8, bearer: Option<&str>) -> u16 {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string();
        let mut mac = Hmac::<Sha256>::new_from_slice(&[key; 32]).unwrap();
        mac.update(format!("{EVENT_ID}.{timestamp}.{BODY}").as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());
        let auth = bearer
            .map(|value| format!("Authorization: Bearer {value}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "POST /webhooks/aurora HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nwebhook-id: {EVENT_ID}\r\nwebhook-timestamp: {timestamp}\r\nwebhook-signature: v1,{signature}\r\n{auth}Connection: close\r\n\r\n{BODY}",
            BODY.len()
        );
        let mut stream = TcpStream::connect_timeout(&self.address, Duration::from_secs(2)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response.split_whitespace().nth(1).unwrap().parse().unwrap()
    }
}
impl Drop for Receiver {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn private_file(path: &Path, text: &str) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(text.as_bytes()).unwrap();
}
fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn failure(output: Output, secret: &str) {
    assert!(!output.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!text.contains(secret), "diagnostic exposed a source value");
}

#[test]
fn config_adjacent_dotenv_enforces_both_credentials_and_retains_the_event() {
    let sandbox = Sandbox::new();
    sandbox.credentials(&sandbox.env_path(), 41, FILE_TOKEN);
    // Neither the working directory nor its ancestors are dotenv search roots.
    private_file(
        &sandbox.root.join(".env"),
        "THIS FILE IS NOT VALID DOTENV\n",
    );
    let receiver = sandbox.serve(None, &[]);
    assert_eq!(receiver.send(41, None), 401);
    assert_eq!(receiver.send(41, Some(PROCESS_TOKEN)), 401);
    assert_eq!(receiver.send(42, Some(FILE_TOKEN)), 401);
    assert_eq!(receiver.send(41, Some(FILE_TOKEN)), 204);
    drop(receiver);
    let retained = success(sandbox.command().arg("list").output().unwrap());
    let retained: serde_json::Value = serde_json::from_str(&retained).unwrap();
    assert_eq!(retained.as_array().unwrap().len(), 1);
    assert_eq!(retained[0]["event_id"], EVENT_ID);
}

#[test]
fn inherited_credentials_override_dotenv_including_an_empty_optional_bearer() {
    let sandbox = Sandbox::new();
    sandbox.credentials(&sandbox.env_path(), 41, FILE_TOKEN);
    let signing = format!("whsec_{}", STANDARD.encode([42; 32]));
    let receiver = sandbox.serve(
        None,
        &[
            ("AURORA_WEBHOOK_SECRET", &signing),
            ("AURORA_BEARER_TOKEN", PROCESS_TOKEN),
        ],
    );
    assert_eq!(receiver.send(41, Some(FILE_TOKEN)), 401);
    assert_eq!(receiver.send(42, Some(FILE_TOKEN)), 401);
    assert_eq!(receiver.send(41, Some(PROCESS_TOKEN)), 401);
    assert_eq!(receiver.send(42, Some(PROCESS_TOKEN)), 204);
    drop(receiver);
    let receiver = sandbox.serve(
        None,
        &[
            ("AURORA_WEBHOOK_SECRET", &signing),
            ("AURORA_BEARER_TOKEN", ""),
        ],
    );
    assert_eq!(receiver.send(42, None), 204);
}

#[test]
fn explicit_dotenv_replaces_default_selection_for_serve_and_doctor() {
    let sandbox = Sandbox::new();
    private_file(&sandbox.env_path(), "INVALID DEFAULT FILE\n");
    let explicit = sandbox.root.join("chosen.env");
    sandbox.credentials(&explicit, 43, PROCESS_TOKEN);
    let receiver = sandbox.serve(Some(&explicit), &[]);
    assert_eq!(receiver.send(41, Some(FILE_TOKEN)), 401);
    assert_eq!(receiver.send(43, Some(PROCESS_TOKEN)), 204);
    drop(receiver);
    // Relative explicit paths are relative to CWD, not the selected config.
    let doctor = success(
        sandbox
            .command()
            .args(["--env-file", "chosen.env", "doctor"])
            .output()
            .unwrap(),
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&doctor).unwrap()["network_or_inference_calls"],
        0
    );
}

#[test]
fn missing_implicit_file_is_optional_but_missing_explicit_file_is_not() {
    let sandbox = Sandbox::new();
    success(sandbox.command().arg("doctor").output().unwrap());
    for command in ["serve", "doctor"] {
        let output = sandbox
            .command()
            .args(["--env-file", "missing.env", command])
            .output()
            .unwrap();
        failure(output, FILE_TOKEN);
    }
    let signing = format!("whsec_{}", STANDARD.encode([41; 32]));
    let receiver = sandbox.serve(
        None,
        &[
            ("AURORA_WEBHOOK_SECRET", &signing),
            ("AURORA_BEARER_TOKEN", FILE_TOKEN),
        ],
    );
    assert_eq!(receiver.send(41, Some(FILE_TOKEN)), 204);
}

#[test]
fn insecure_symlink_hardlink_and_untrusted_ancestor_files_fail_redacted() {
    let sandbox = Sandbox::new();
    let path = sandbox.env_path();
    sandbox.credentials(&path, 41, FILE_TOKEN);
    for mode in [0o644, 0o400, 0o660] {
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        let output = sandbox.command().arg("doctor").output().unwrap();
        failure(output, FILE_TOKEN);
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let target = sandbox.root.join("target.env");
    fs::rename(&path, &target).unwrap();
    symlink(&target, &path).unwrap();
    failure(
        sandbox.command().arg("doctor").output().unwrap(),
        FILE_TOKEN,
    );
    fs::remove_file(&path).unwrap();
    fs::hard_link(&target, &path).unwrap();
    failure(
        sandbox.command().arg("doctor").output().unwrap(),
        FILE_TOKEN,
    );
    fs::remove_file(&path).unwrap();
    let untrusted = sandbox.root.join("untrusted");
    fs::create_dir(&untrusted).unwrap();
    sandbox.credentials(&untrusted.join("credentials.env"), 41, FILE_TOKEN);
    fs::set_permissions(&untrusted, fs::Permissions::from_mode(0o777)).unwrap();
    failure(
        sandbox
            .command()
            .arg("--env-file")
            .arg(untrusted.join("credentials.env"))
            .arg("doctor")
            .output()
            .unwrap(),
        FILE_TOKEN,
    );
    fs::set_permissions(&untrusted, fs::Permissions::from_mode(0o700)).unwrap();
    let alias = sandbox.root.join("alias");
    symlink(&untrusted, &alias).unwrap();
    failure(
        sandbox
            .command()
            .arg("--env-file")
            .arg(alias.join("credentials.env"))
            .arg("doctor")
            .output()
            .unwrap(),
        FILE_TOKEN,
    );
}

#[test]
fn malformed_oversized_and_invalid_credentials_never_leak_source_values() {
    let sandbox = Sandbox::new();
    let path = sandbox.env_path();
    private_file(
        &path,
        &format!("AURORA_BEARER_TOKEN={FILE_TOKEN}\nBROKEN='unterminated-{FILE_TOKEN}\n"),
    );
    for command in ["serve", "doctor"] {
        failure(sandbox.command().arg(command).output().unwrap(), FILE_TOKEN);
    }
    fs::remove_file(&path).unwrap();
    private_file(&path, &format!("SECRET={FILE_TOKEN}{}", "x".repeat(65536)));
    failure(
        sandbox.command().arg("doctor").output().unwrap(),
        FILE_TOKEN,
    );
    fs::remove_file(&path).unwrap();
    private_file(&path, &format!("AURORA_WEBHOOK_SECRET='{FILE_TOKEN}'\n"));
    failure(sandbox.command().arg("serve").output().unwrap(), FILE_TOKEN);
    fs::remove_file(&path).unwrap();
    private_file(
        &path,
        &format!(
            "AURORA_WEBHOOK_SECRET=whsec_{}\nAURORA_BEARER_TOKEN='short-{FILE_TOKEN} with-space'\n",
            STANDARD.encode([41; 32])
        ),
    );
    failure(sandbox.command().arg("serve").output().unwrap(), FILE_TOKEN);
}

#[test]
fn generators_are_independent_of_config_dotenv_and_malformed_credentials() {
    let sandbox = Sandbox::new();
    private_file(&sandbox.env_path(), "BROKEN='unterminated\n");
    fs::remove_file(&sandbox.config).unwrap();
    for generator in ["keygen", "token-gen"] {
        let generate = || {
            success(
                sandbox
                    .command()
                    .args(["--env-file", "nonexistent.env", generator])
                    .env("AURORA_WEBHOOK_SECRET", "invalid signing key")
                    .env("AURORA_BEARER_TOKEN", "invalid bearer")
                    .output()
                    .unwrap(),
            )
            .trim()
            .to_owned()
        };
        let first = generate();
        let second = generate();
        assert_ne!(first, second, "generators must use fresh randomness");
        if generator == "keygen" {
            assert_eq!(
                STANDARD
                    .decode(first.strip_prefix("whsec_").unwrap())
                    .unwrap()
                    .len(),
                32
            );
        } else {
            assert!(!first.starts_with("whsec_"));
            assert_eq!(first.len(), 43);
            assert_eq!(URL_SAFE_NO_PAD.decode(&first).unwrap().len(), 32);
        }
    }
}

#[test]
fn read_only_admin_ignores_unrelated_dotenv_and_malformed_credentials() {
    let sandbox = Sandbox::new();
    sandbox.credentials(&sandbox.env_path(), 41, FILE_TOKEN);
    let receiver = sandbox.serve(None, &[]);
    assert_eq!(receiver.send(41, Some(FILE_TOKEN)), 204);
    drop(receiver);
    fs::remove_file(sandbox.env_path()).unwrap();
    private_file(&sandbox.env_path(), "BROKEN='unterminated\n");
    let status = success(
        sandbox
            .command()
            .args(["--env-file", "missing.env", "status"])
            .env("AURORA_WEBHOOK_SECRET", "invalid signing key")
            .env("AURORA_BEARER_TOKEN", "invalid bearer")
            .output()
            .unwrap(),
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&status).unwrap()["records"],
        1
    );
}

#[test]
fn signing_stays_required_and_dotenv_never_executes_shell_substitutions() {
    let sandbox = Sandbox::new();
    let marker = sandbox.root.join("must-not-exist");
    private_file(
        &sandbox.env_path(),
        &format!(
            "AURORA_BEARER_TOKEN={FILE_TOKEN}\nUNEXECUTED=\"$(touch {})\"\n",
            marker.display()
        ),
    );
    let output = sandbox.command().arg("serve").output().unwrap();
    failure(output, FILE_TOKEN);
    assert!(!marker.exists());
}

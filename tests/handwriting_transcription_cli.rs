use remarkable_reader_buddy::{
    config::Config,
    storage::{StorePaths, Uuid},
};
use serde_json::{json, Value};
use std::{
    io::Write,
    process::{Command, Stdio},
};
#[test]
fn actual_cli_refuses_strict_input_and_credentials_before_store_provider_or_content_logs() {
    let root = std::env::temp_dir().join(format!("rmb-transcription-cli-{}", Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let config = Config {
        paths: StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        },
        ..Config::default()
    };
    let path = root.join("config.json");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/handwriting-transcription-v1.json")).unwrap();
    let valid = json!({"transcription_version":1,"invocation":Uuid::new_v4(),"document":Uuid::new_v4(),"page":7,"target_png":fixture["target_png"],"context":null});
    for (input, failure) in [
        (b"{PRIVATE_INPUT_CANARY}".to_vec(), "invalid-input"),
        (
            serde_json::to_vec(&valid).unwrap(),
            "credentials-unavailable",
        ),
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
            .arg("--handwriting-transcription")
            .env("REMARKABLE_BUDDIES_CONFIG", &path)
            .env_remove("OPENAI_API_KEY")
            .env_remove("OPENAI_BASE_URL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&input).unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(!result.status.success());
        let report: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(report["failure"], failure);
        assert_eq!(report["provider_attempt"], "not-called");
        assert_eq!(report["confirmed"], false);
        assert!(report["result"].is_null());
        assert!(!config.paths.data.exists());
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!stderr.contains("PRIVATE_INPUT_CANARY"));
        assert!(!stderr.contains("iVBOR"));
    }
    for extra in ["--handwriting", "--writer-note", "--inspect-owned-data"] {
        let result = Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
            .args(["--handwriting-transcription", extra])
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
    }
    let absolute = root.canonicalize().unwrap();
    assert_eq!(
        absolute.parent().unwrap(),
        std::env::temp_dir().canonicalize().unwrap()
    );
    assert!(absolute
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("rmb-transcription-cli-"));
    std::fs::remove_dir_all(absolute).unwrap();
}

/// A real configured request to a loopback fixture, never a live provider.
#[cfg(unix)]
#[test]
fn actual_cli_sigint_cancels_joined_transport_and_releases_unchanged_store() {
    use remarkable_reader_buddy::storage::{Namespace, Store};
    use std::{
        io::Read,
        net::TcpListener,
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };
    let root = std::env::temp_dir().join(format!("rmb-transcription-cancel-{}", Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let config = Config {
        paths: StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        },
        ..Config::default()
    };
    let path = root.join("config.json");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/handwriting-transcription-v1.json")).unwrap();
    let profile = Uuid::new_v4();
    let key = Uuid::new_v4();
    let confirm = json!({"handwriting_version":1,"command":"confirm","profile":profile,"correction":key,"operation":Uuid::new_v4(),"expected_heads":[],"confirmation":"user-confirmed","original_text":"PRIVATE_ORIGINAL","corrected_text":"PRIVATE_CORRECTED","confirmed_ms":1,"provenance":{"kind":"caller-confirmed-sample","description":"Synthetic loopback cancellation test only","source":null},"sample_png_base64":fixture["target_png"]});
    let mut child = Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
        .arg("--handwriting")
        .env("REMARKABLE_BUDDIES_CONFIG", &path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&confirm).unwrap())
        .unwrap();
    let confirmed = child.wait_with_output().unwrap();
    assert!(confirmed.status.success());
    let retained: Value = serde_json::from_slice(&confirmed.stdout).unwrap();
    let revision = retained["facts"][0]["revision"].clone();
    let record: Uuid = serde_json::from_value(retained["facts"][0]["record"].clone()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let (sender, receiver) = mpsc::channel();
    let server = thread::spawn(move || {
        let due = Instant::now() + Duration::from_secs(12);
        let mut socket = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= due {
                        return false;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return false,
            }
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut buffer = [0; 8192];
        let n = socket.read(&mut buffer).unwrap();
        if n == 0 {
            return false;
        }
        let _ = sender.send(());
        // No reply: SIGINT must cancel the real configured client's owned work.
        loop {
            match socket.read(&mut buffer) {
                Ok(0) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    });
    let input = json!({"transcription_version":1,"invocation":Uuid::new_v4(),"document":Uuid::new_v4(),"page":7,"target_png":fixture["target_png"],"context":{"profile":profile,"examples":[{"correction":key,"revision":revision}]}});
    let mut child = Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
        .arg("--handwriting-transcription")
        .env("REMARKABLE_BUDDIES_CONFIG", &path)
        .env("OPENAI_API_KEY", "PRIVATE_DUMMY_LOOPBACK_KEY")
        .env("OPENAI_BASE_URL", format!("http://{address}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&input).unwrap())
        .unwrap();
    if receiver.recv_timeout(Duration::from_secs(12)).is_err() {
        let _ = child.kill();
        let result = child.wait_with_output().unwrap();
        let _ = server.join();
        panic!(
            "loopback request not observed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    assert!(Store::open(config.paths.clone()).is_err());
    assert!(Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap()
        .success());
    let due = Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= due {
            let _ = child.kill();
            let _ = child.wait();
            let _ = server.join();
            panic!("configured cancellation did not finish");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let result = child.wait_with_output().unwrap();
    assert!(
        server.join().unwrap(),
        "owned request connection remained open"
    );
    assert!(!result.status.success());
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["status"], "canceled");
    assert_eq!(report["provider_attempt"], "called");
    assert_eq!(report["failure"], "canceled");
    assert!(report["context"].is_object());
    assert!(report["result"].is_null());
    let stderr = String::from_utf8_lossy(&result.stderr);
    for secret in [
        "PRIVATE_ORIGINAL",
        "PRIVATE_CORRECTED",
        "PRIVATE_DUMMY_LOOPBACK_KEY",
    ] {
        assert!(!stderr.contains(secret));
    }
    let store = Store::open(config.paths.clone()).unwrap();
    let heads = store.heads(Namespace::Handwriting, record).unwrap();
    assert_eq!(heads.len(), 1);
    assert_eq!(json!(heads[0].revision_id), revision);
    drop(store);
    let absolute = root.canonicalize().unwrap();
    assert_eq!(
        absolute.parent().unwrap(),
        std::env::temp_dir().canonicalize().unwrap()
    );
    assert!(absolute
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("rmb-transcription-cancel-"));
    std::fs::remove_dir_all(absolute).unwrap();
}

#[test]
fn actual_cli_reports_cross_process_store_contention_as_busy_before_provider_resolution() {
    use remarkable_reader_buddy::storage::Store;
    let root = std::env::temp_dir().join(format!("rmb-transcription-busy-{}", Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let config = Config {
        paths: StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        },
        ..Config::default()
    };
    let path = root.join("config.json");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let store = Store::open(config.paths.clone()).unwrap();
    let before = std::fs::read(config.paths.data.join("CURRENT")).unwrap();
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/handwriting-transcription-v1.json")).unwrap();
    let input = json!({"transcription_version":1,"invocation":Uuid::new_v4(),"document":Uuid::new_v4(),"page":7,"target_png":fixture["target_png"],"context":{"profile":Uuid::new_v4(),"examples":[{"correction":Uuid::new_v4(),"revision":Uuid::new_v4()}]}});
    let mut child = Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
        .arg("--handwriting-transcription")
        .env("REMARKABLE_BUDDIES_CONFIG", &path)
        .env_remove("OPENAI_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&input).unwrap())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(!result.status.success());
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["failure"], "ownership-busy");
    assert_eq!(report["provider_attempt"], "not-called");
    assert!(report["context"].is_null());
    assert_eq!(
        before,
        std::fs::read(config.paths.data.join("CURRENT")).unwrap()
    );
    drop(store);
    let absolute = root.canonicalize().unwrap();
    assert_eq!(
        absolute.parent().unwrap(),
        std::env::temp_dir().canonicalize().unwrap()
    );
    assert!(absolute
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("rmb-transcription-busy-"));
    std::fs::remove_dir_all(absolute).unwrap();
}

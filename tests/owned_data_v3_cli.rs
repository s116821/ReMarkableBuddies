use remarkable_reader_buddy::storage::{Store, StorePaths, Uuid};
use serde_json::{json, Value};
use std::{
    fs,
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let f = Self(std::env::temp_dir().join(format!("owned-data-v3-cli-{}", Uuid::new_v4())));
        fs::create_dir_all(&f.0).unwrap();
        f
    }
    fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.0.join("data"),
            cache: self.0.join("cache"),
            credentials: self.0.join("credentials"),
        }
    }
    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
        c.current_dir(&self.0)
            .env("REMARKABLE_BUDDIES_CONFIG", self.0.join("config.json"))
            .env_remove("NOTIFY_SOCKET")
            .env_remove("WATCHDOG_USEC")
            .env("OPENAI_API_KEY", "PRIVATE_UNUSED_KEY")
            .env("OPENAI_BASE_URL", "http://127.0.0.1:1");
        c
    }
    fn run(&self) -> Value {
        let mut child = self
            .command()
            .arg("--inspect-owned-data-v3")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let held_stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let reader = std::thread::spawn(move || {
            let mut bytes = vec![];
            stdout.read_to_end(&mut bytes).unwrap();
            bytes
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(s) = child.try_wait().unwrap() {
                break s;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("v3 must exit without stdin EOF or runtime startup");
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        drop(held_stdin);
        let bytes = reader.join().unwrap();
        let mut stderr = vec![];
        child
            .stderr
            .take()
            .unwrap()
            .read_to_end(&mut stderr)
            .unwrap();
        assert!(status.success());
        assert!(stderr.is_empty());
        assert!(bytes.len() <= 32768 && bytes.ends_with(b"\n"));
        assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE_"));
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["report_version"], 3);
        assert_eq!(v.as_object().unwrap().len(), 26);
        v
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn actual_cli_exits_with_open_stdin_and_unavailable_configuration_without_startup() {
    let f = Fixture::new();
    fs::write(
        f.0.join(".env"),
        b"READER_BUDDY_DEBUG_DUMP=PRIVATE_POISON\n",
    )
    .unwrap();
    let before = fs::read_dir(&f.0).unwrap().count();
    let v = f.run();
    assert_eq!(v["status"], "unavailable");
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), before);
    assert!(!f.paths().data.exists());
}

#[test]
fn actual_cli_reports_supported_custody_or_explicit_platform_refusal() {
    let f = Fixture::new();
    fs::write(
        f.0.join("config.json"),
        serde_json::to_vec(&json!({"paths":f.paths()})).unwrap(),
    )
    .unwrap();
    fs::write(
        f.0.join(".env"),
        b"READER_BUDDY_DEBUG_DUMP=PRIVATE_POISON\n",
    )
    .unwrap();
    drop(Store::prepare_notebook(f.paths()).unwrap());
    let current = fs::read(f.paths().data.join("CURRENT")).unwrap();
    let v = f.run();
    #[cfg(target_os = "linux")]
    {
        assert_eq!(v["status"], "complete");
        assert_eq!(v["current_generation_format"], 4);
        assert_eq!(v["lease_identity_check"], "device-inode");
    }
    #[cfg(not(target_os = "linux"))]
    {
        assert_eq!(v["status"], "unsupported");
        assert_eq!(v["lease_identity_check"], "unavailable");
    }
    assert_eq!(fs::read(f.paths().data.join("CURRENT")).unwrap(), current);
}

#[test]
fn actual_cli_rejects_legacy_selector_runtime_and_input_peers_before_opening_roots() {
    let f = Fixture::new();
    for peer in [
        vec!["--inspect-owned-data"],
        vec!["--configuration-schema"],
        vec!["--api-key", "fixture"],
        vec!["--sync-mode-operation", "initialize"],
        vec!["--inspect-owned-data-v3", "operand"],
    ] {
        let output = f
            .command()
            .arg("--inspect-owned-data-v3")
            .args(peer)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!f.paths().data.exists());
    }
}

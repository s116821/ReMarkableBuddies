//! Actual host CLI only. Runtime-negative cases must exit before device/model construction.
use remarkable_reader_buddy::{
    memory,
    mode_maintenance::{MAX_INPUT, MAX_OUTPUT},
    storage::{self, Store, StorePaths, Uuid},
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let value = Self {
            root: std::env::temp_dir().join(format!("sync-mode-cli-{}", Uuid::new_v4())),
        };
        fs::create_dir_all(&value.root).unwrap();
        value.save_config(false);
        fs::write(
            value.root.join(".env"),
            b"OPENAI_API_KEY=PRIVATE_DOTENV_SENTINEL\nREADER_BUDDY_DEBUG_DUMP=poison\n",
        )
        .unwrap();
        value
    }
    fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.root.join("data"),
            cache: self.root.join("cache"),
            credentials: self.root.join("credentials"),
        }
    }
    fn save_config(&self, enabled: bool) {
        fs::write(
            self.root.join("config.json"),
            serde_json::to_vec(&json!({"paths":self.paths(),"sync":{"enabled":enabled}})).unwrap(),
        )
        .unwrap();
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
        cmd.current_dir(&self.root)
            .env("REMARKABLE_BUDDIES_CONFIG", self.root.join("config.json"))
            .env("OPENAI_API_KEY", "PRIVATE_ENV_SENTINEL")
            .env("OPENAI_BASE_URL", "http://127.0.0.1:1")
            .env("RUST_LOG", "trace");
        cmd
    }
    fn output(&self, mut cmd: Command, bytes: Option<&[u8]>) -> Output {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        if bytes.is_some() {
            cmd.stdin(Stdio::piped());
        } else {
            cmd.stdin(Stdio::null());
        }
        let mut child = cmd.spawn().unwrap();
        let writer = bytes.map(|bytes| {
            let mut input = child.stdin.take().unwrap();
            let bytes = bytes.to_vec();
            std::thread::spawn(move || input.write_all(&bytes))
        });
        let deadline = Instant::now() + Duration::from_secs(15);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                let _ = child.wait();
                if let Some(writer) = writer {
                    let _ = writer.join();
                }
                panic!("host command did not complete before the caller deadline");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        if let Some(writer) = writer {
            if let Err(error) = writer.join().unwrap() {
                assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
            }
        }
        child.wait_with_output().unwrap()
    }
    fn run(&self, operation: &str, input: Option<&[u8]>, file: bool) -> Value {
        let mut cmd = self.command();
        cmd.args(["--sync-mode-operation", operation]);
        if let Some(bytes) = input {
            cmd.arg("--sync-mode-input");
            if file {
                fs::write(self.root.join("input.json"), bytes).unwrap();
                cmd.arg(self.root.join("input.json"));
            } else {
                cmd.arg("-");
            }
        }
        let output = self.output(cmd, if file { None } else { input });
        assert!(
            output.status.success(),
            "{:?}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        assert!(output.stdout.len() <= MAX_OUTPUT);
        assert!(output.stdout.ends_with(b"\n"));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["report_version"], 1);
        assert_eq!(value["activation_performed"], false);
        assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE_"));
        value
    }
    fn initialize(&self) -> Value {
        let report = self.run("initialize", None, false);
        assert_eq!(report["status"], "ok");
        assert!(report["data"]["receipt"].is_null());
        report
    }
    fn choose(&self, request: &Value) -> Value {
        self.run("choose", Some(&serde_json::to_vec(request).unwrap()), false)
    }
    fn inventory(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn visit(path: &Path, entries: &mut BTreeMap<PathBuf, Vec<u8>>) {
            if !path.exists() {
                return;
            }
            for item in fs::read_dir(path).unwrap() {
                let path = item.unwrap().path();
                if path.is_dir() {
                    visit(&path, entries);
                } else {
                    // Windows' mandatory byte lock prevents reading this file
                    // while its owner is alive. Preserve its inventory/size;
                    // all actual data and mode files remain exact-byte checks.
                    let bytes = if path.file_name().is_some_and(|name| name == "store.lock") {
                        fs::metadata(&path).unwrap().len().to_le_bytes().to_vec()
                    } else {
                        fs::read(&path).unwrap()
                    };
                    entries.insert(path, bytes);
                }
            }
        }
        let mut entries = BTreeMap::new();
        for path in [
            self.paths().data,
            self.paths().cache,
            self.paths().credentials,
        ] {
            visit(&path, &mut entries);
        }
        entries
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(self.root.parent(), Some(std::env::temp_dir().as_path()));
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn request(snapshot: &Value, choice: Value) -> Value {
    json!({"operation":Uuid::new_v4(),"expected":snapshot["data"]["current_snapshot"]["token"],"choice":choice})
}
fn local() -> Value {
    json!({"kind":"local-only"})
}
fn isolated() -> Value {
    json!({"kind":"isolated-backup","binding_sha256":storage::digest(b"fixture")})
}
fn shared(group: Uuid) -> Value {
    json!({"kind":"shared-group","group":group,"binding_sha256":storage::digest(b"fixture"),"root_id":"fixture-root"})
}

#[test]
fn actual_choice_restart_and_original_historical_retry_preserve_intent() {
    let f = Fixture::new();
    assert_eq!(f.run("inspect", None, false)["status"], "unavailable");
    assert!(!f.paths().data.exists());
    let initial = f.initialize();
    assert!(initial["data"]["current_snapshot"]["mode"].is_null());
    let first_request = request(&initial, isolated());
    let first = f.choose(&first_request);
    assert_eq!(first["status"], "ok");
    assert_eq!(first["data"]["receipt_status"], "current");
    let before = f.inventory();
    assert_eq!(f.choose(&first_request), first);
    assert_eq!(f.inventory(), before);
    let latest = f.choose(&request(&first, local()));
    assert_eq!(latest["status"], "ok");
    let before = f.inventory();
    let historical = f.choose(&first_request);
    assert_eq!(historical["data"]["receipt"], first["data"]["receipt"]);
    assert_eq!(historical["data"]["receipt_status"], "historical");
    assert_eq!(
        historical["data"]["current_snapshot"],
        latest["data"]["current_snapshot"]
    );
    assert_eq!(f.inventory(), before);
    let mut changed = first_request.clone();
    changed["choice"] = local();
    assert_eq!(f.choose(&changed)["status"], "conflict");
    changed["operation"] = json!(Uuid::new_v4());
    assert_eq!(f.choose(&changed)["status"], "conflict");
    changed["expected"]["actor"] = json!(Uuid::new_v4());
    assert_eq!(f.choose(&changed)["status"], "invalid-input");
    assert_eq!(f.inventory(), before);
}

#[test]
fn inclusive_file_stdin_bounds_and_strict_input_refuse_before_store_creation() {
    let f = Fixture::new();
    let initial = f.initialize();
    let req = request(&initial, local());
    let mut bytes = serde_json::to_vec(&req).unwrap();
    bytes.resize(MAX_INPUT, b' ');
    let first = f.run("choose", Some(&bytes), false);
    assert_eq!(first["status"], "ok");
    assert_eq!(f.run("choose", Some(&bytes), true), first);
    let before = f.inventory();
    bytes.push(b' ');
    for file in [false, true] {
        assert_eq!(
            f.run("choose", Some(&bytes), file)["status"],
            "invalid-input"
        );
    }
    assert_eq!(f.inventory(), before);
    for index in 0..7 {
        let fresh = Fixture::new();
        let mut req = req.clone();
        match index {
            0 => req["operation"] = json!(Uuid::nil()),
            1 => req["expected"]["actor"] = json!(Uuid::nil()),
            2 => req["expected"]["sha256"] = json!("PRIVATE_BAD_DIGEST"),
            3 => req["expected"]["extra"] = json!(true),
            4 => req["extra"] = json!(true),
            5 => req["choice"] = shared(Uuid::nil()),
            _ => req["choice"] = json!({"kind":"local-only","extra":true}),
        }
        assert_eq!(fresh.choose(&req)["status"], "invalid-input");
        assert!(!fresh.paths().data.exists());
    }
}

#[test]
fn existing_open_is_nonrepairing_and_busy_never_stops_owner() {
    let f = Fixture::new();
    f.initialize();
    let stage = f
        .paths()
        .data
        .join("sync")
        .join(format!(".stage-{}", Uuid::new_v4()));
    fs::write(&stage, b"private interrupted residue").unwrap();
    let before = f.inventory();
    assert_eq!(f.run("inspect", None, false)["status"], "ok");
    assert_eq!(f.inventory(), before);
    let held = Store::open_existing(f.paths()).unwrap();
    assert_eq!(f.run("inspect", None, false)["status"], "busy");
    assert_eq!(f.inventory(), before);
    drop(held);
    fs::remove_file(f.paths().cache.join("owner.json")).unwrap();
    let before = f.inventory();
    assert_eq!(f.run("inspect", None, false)["status"], "unavailable");
    assert_eq!(f.inventory(), before);
    let unknown = Fixture::new();
    fs::create_dir_all(unknown.paths().data).unwrap();
    fs::write(unknown.paths().data.join("foreign"), b"original").unwrap();
    let before = unknown.inventory();
    assert_eq!(
        unknown.run("initialize", None, false)["status"],
        "unavailable"
    );
    assert_eq!(unknown.inventory(), before);
}

#[test]
fn selected_records_media_and_legacy_admission_survive_mode_inspect_and_choice() {
    let f = Fixture::new();
    let initial = f.initialize();
    let group = Uuid::new_v4();
    assert_eq!(f.choose(&request(&initial, shared(group)))["status"], "ok");
    let store = Arc::new(Store::open(f.paths()).unwrap());
    let bytes = b"owned source evidence preserved by mode maintenance".to_vec();
    let hash = storage::digest(&bytes);
    let envelope = storage::Envelope {
        envelope_version: 1,
        namespace: storage::Namespace::Source,
        domain_schema_version: 1,
        record_id: Uuid::new_v4(),
        revision_id: Uuid::new_v4(),
        parents: BTreeSet::new(),
        operation_id: Uuid::new_v4(),
        actor_id: store.actor_id,
        kind: storage::Kind::Value,
        payload: json!({"fixture":"source"}),
        media_descriptors: vec![storage::Media {
            sha256: hash.clone(),
            bytes: bytes.len() as u64,
            media_type: "application/octet-stream".into(),
        }],
    };
    store
        .commit(vec![envelope], BTreeMap::from([(hash, bytes)]))
        .unwrap();
    let memory = memory::selected::SelectedMemory::new(
        store.clone(),
        memory::catalog::Identity {
            group,
            profile: Uuid::new_v4(),
            binding_sha256: storage::digest(b"fixture"),
        },
    )
    .unwrap();
    memory
        .initialize_selection(memory::selected::InitializeSelection {
            operation: Uuid::new_v4(),
            accepted_base_sha256: storage::digest(b"fixture"),
            registrations: vec![],
        })
        .unwrap();
    drop(memory);
    drop(store);
    assert!(Store::open_existing(f.paths()).is_err());
    let before = f.inventory();
    let view = f.run("inspect", None, false);
    assert_eq!(view["status"], "ok");
    assert_eq!(f.inventory(), before);
    let chosen = f.choose(&request(&view, local()));
    assert_eq!(chosen["status"], "ok");
    let after = f.inventory();
    let mode_path = f.paths().data.join("sync/mode.json");
    assert_ne!(before.get(&mode_path), after.get(&mode_path));
    for (path, bytes) in &before {
        if path != &mode_path {
            assert_eq!(after.get(path), Some(bytes));
        }
    }
    assert_eq!(after.len(), before.len());
    let config_path = f.root.join("config.json");
    let config = fs::read(&config_path).unwrap();
    let mut cmd = f.command();
    cmd.args(["--personalization-operation", "disable"]);
    let output = f.output(
        cmd,
        Some(
            &serde_json::to_vec(
                &json!({"contract_version":1,"expected_config_sha256":storage::digest(&config)}),
            )
            .unwrap(),
        ),
    );
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["status"],
        "unsupported"
    );
    assert_eq!(fs::read(config_path).unwrap(), config);
    assert_eq!(f.inventory(), after);
    assert_ordinary_refuses(&f, "selected storage requires coordinated sync");
}

fn assert_ordinary_refuses(f: &Fixture, expected: &str) {
    // The one-shot commands above must ignore poison dotenv. Ordinary startup
    // legitimately loads it, so provide valid startup configuration for this gate.
    fs::write(f.root.join(".env"), b"READER_BUDDY_DEBUG_DUMP=false\n").unwrap();
    let before = f.inventory();
    let mut cmd = f.command();
    cmd.args(["--api-key", "PRIVATE_FAKE_KEY"]);
    let output = f.output(cmd, None);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(expected), "{stderr}");
    assert!(!stderr.contains("PRIVATE_"));
    assert_eq!(f.inventory(), before);
}

#[test]
fn actual_normal_startup_refuses_nonordinary_mode_before_runtime_constructors() {
    for mode in [isolated(), shared(Uuid::new_v4())] {
        let f = Fixture::new();
        let initial = f.initialize();
        assert_eq!(f.choose(&request(&initial, mode))["status"], "ok");
        assert_ordinary_refuses(&f, "ordinary mode requires absent or LocalOnly choice");
    }
    let f = Fixture::new();
    let initial = f.initialize();
    f.choose(&request(&initial, local()));
    f.save_config(true);
    assert_ordinary_refuses(&f, "explicit local mode requires legacy sync disabled");
}

#[test]
fn mutating_commands_refuse_legacy_sync_without_initializing() {
    let f = Fixture::new();
    f.save_config(true);
    assert_eq!(f.run("initialize", None, false)["status"], "unsupported");
    assert!(!f.paths().data.exists());
    f.save_config(false);
    let initial = f.initialize();
    f.save_config(true);
    let before = f.inventory();
    assert_eq!(
        f.choose(&request(&initial, local()))["status"],
        "unsupported"
    );
    assert_eq!(f.inventory(), before);
    assert_eq!(f.run("inspect", None, false)["status"], "ok");
}

#[test]
fn command_conflicts_do_not_echo_credentials_or_enter_other_routes() {
    let f = Fixture::new();
    for args in [
        vec!["--api-key", "PRIVATE_FAKE_KEY"],
        vec!["--simulate", "missing"],
        vec!["--memory-operation", "query", "--memory-input", "-"],
        vec!["--log-level", "trace"],
    ] {
        let mut cmd = f.command();
        cmd.args(["--sync-mode-operation", "inspect"]).args(args);
        let output = f.output(cmd, None);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE_FAKE_KEY"));
        assert!(!f.paths().data.exists());
    }
}

#[test]
fn relative_or_nonregular_input_refuses_and_choose_requires_its_input() {
    let f = Fixture::new();
    for input in [PathBuf::from("relative.json"), f.root.clone()] {
        let mut cmd = f.command();
        cmd.args(["--sync-mode-operation", "choose", "--sync-mode-input"])
            .arg(input);
        let output = f.output(cmd, None);
        assert!(output.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap()["status"],
            "invalid-input"
        );
        assert!(!f.paths().data.exists());
    }
    let mut cmd = f.command();
    cmd.args(["--sync-mode-operation", "choose"]);
    assert!(!f.output(cmd, None).status.success());
    assert!(!f.paths().data.exists());
}

#[cfg(unix)]
#[test]
fn fifo_lock_and_fifo_request_refuse_before_blocking_open() {
    let f = Fixture::new();
    let initial = f.initialize();
    let input = f.root.join("request-pipe");
    assert!(Command::new("mkfifo")
        .arg(&input)
        .status()
        .unwrap()
        .success());
    let mut cmd = f.command();
    cmd.args(["--sync-mode-operation", "choose", "--sync-mode-input"])
        .arg(input);
    let output = f.output(cmd, None);
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["status"],
        "invalid-input"
    );
    let lock = f.paths().data.join("store.lock");
    fs::remove_file(&lock).unwrap();
    assert!(Command::new("mkfifo")
        .arg(&lock)
        .status()
        .unwrap()
        .success());
    let before = fs::read(f.paths().data.join("CURRENT")).unwrap();
    assert_eq!(f.run("inspect", None, false)["status"], "unsupported");
    assert_eq!(
        f.choose(&request(&initial, local()))["status"],
        "unsupported"
    );
    assert_eq!(fs::read(f.paths().data.join("CURRENT")).unwrap(), before);
}

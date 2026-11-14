//! Actual executable contract. Synthetic originals prove Status correspondence;
//! they do not qualify the provider or authorize Inspect/Edit.
use remarkable_reader_buddy::{
    config::portable::{
        record::{self, Condition, SelectionCondition},
        selected::Artifact,
    },
    storage::{
        self,
        selection::{SelectionChange, SelectionScope},
        sync::mode,
        Store, StorePaths, Uuid,
    },
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("shared-preferences-cli-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let fixture = Self(root);
        fs::write(
            fixture.0.join("config.json"),
            serde_json::to_vec(
                &json!({"paths":fixture.paths(),"personalization":{"text":"PRIVATE LOCAL"}}),
            )
            .unwrap(),
        )
        .unwrap();
        fs::write(fixture.0.join(".env"), b"OPENAI_API_KEY=PRIVATE_DOTENV\n").unwrap();
        fixture
    }
    fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.0.join("data"),
            cache: self.0.join("cache"),
            credentials: self.0.join("credentials"),
        }
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
        command
            .current_dir(&self.0)
            .env("REMARKABLE_BUDDIES_CONFIG", self.0.join("config.json"))
            .env("OPENAI_API_KEY", "PRIVATE_ENV")
            .env("OPENAI_BASE_URL", "http://127.0.0.1:1")
            .env("RUST_LOG", "trace");
        command
    }
    fn output(&self, mut command: Command, bytes: &[u8]) -> Output {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(bytes).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                let _ = child.wait();
                panic!("preference CLI deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
    }
    fn run(&self, operation: &str, bytes: &[u8]) -> Value {
        let mut cmd = self.command();
        cmd.args([
            "--shared-preferences-operation",
            operation,
            "--shared-preferences-input",
            "-",
        ]);
        let output = self.output(cmd, bytes);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        assert!(output.stdout.len() <= 131072);
        assert_eq!(output.stdout.iter().filter(|&&b| b == b'\n').count(), 1);
        assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE"));
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn shared(&self) -> (Arc<Store>, Uuid, String) {
        let store = Store::open(self.paths()).unwrap();
        let group = Uuid::new_v4();
        let binding = storage::digest(b"unqualified command fixture");
        let request = storage::compound_generation::Request {
            operation: Uuid::new_v4(),
            actor: store.actor_id,
            source_generation: serde_json::from_slice(
                &fs::read(self.paths().data.join("CURRENT")).unwrap(),
            )
            .unwrap(),
            group,
            binding_sha256: binding.clone(),
            expected_mode: serde_json::from_value(
                serde_json::to_value(store.sync_mode_snapshot().unwrap().token).unwrap(),
            )
            .unwrap(),
        };
        drop(store);
        let store = Arc::new(Store::prepare_shared_notebook(self.paths(), request).unwrap());
        store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                mode::SyncModeChoice::SharedGroup {
                    group,
                    binding_sha256: binding.clone(),
                    root_id: "unqualified-root".into(),
                },
            )
            .unwrap();
        (store, group, binding)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn strict_stdin_existing_only_and_sealed_qualification_precede_credentials_or_runtime() {
    let fixture = Fixture::new();
    let inspect = br#"{"shared_preferences_version":1,"reveal_text":false}"#;
    for input in [
        br#"{"shared_preferences_version":1,"reveal_text":false,"reveal_text":true}"#.as_slice(),
        br#"{"shared_preferences_version":1,"reveal_text":false,"unexpected":"PRIVATE"}"#,
        b"\xff",
    ] {
        assert_eq!(fixture.run("inspect", input)["status"], "invalid-input");
    }
    assert_eq!(
        fixture.run("inspect", &vec![b' '; 262145])["status"],
        "invalid-input"
    );
    assert!(!fixture.paths().data.exists());
    assert_eq!(fixture.run("inspect", inspect)["status"], "unavailable");
    assert!(!fixture.paths().data.exists());
    let (store, _, _) = fixture.shared();
    drop(store);
    fs::write(
        fixture.paths().credentials.join("drive.json"),
        b"PRIVATE INVALID CREDENTIAL",
    )
    .unwrap();
    let before = fs::read(fixture.paths().data.join("CURRENT")).unwrap();
    let config = fs::read(fixture.0.join("config.json")).unwrap();
    let report = fixture.run("inspect", inspect);
    assert_eq!(report["status"], "unsupported");
    assert!(report.get("guard").is_none());
    assert!(report.get("view").is_none());
    assert_eq!(report["runtime_activation"], "unverified");
    assert_eq!(
        fs::read(fixture.paths().data.join("CURRENT")).unwrap(),
        before
    );
    assert_eq!(fs::read(fixture.0.join("config.json")).unwrap(), config);
    assert_eq!(
        fs::read(fixture.paths().credentials.join("drive.json")).unwrap(),
        b"PRIVATE INVALID CREDENTIAL"
    );
    assert!(!fixture.paths().data.join("sync/state.json").exists());
}

#[test]
fn active_store_lock_refuses_shared_preference_cli_without_mutation() {
    fn snapshot(root: &std::path::Path) -> BTreeMap<PathBuf, Option<(usize, String)>> {
        let mut result = BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let relative = path.strip_prefix(root).unwrap().to_path_buf();
                let kind = entry.file_type().unwrap();
                if kind.is_dir() {
                    result.insert(relative, None);
                    pending.push(path);
                } else {
                    assert!(kind.is_file(), "unexpected fixture entry: {relative:?}");
                    let bytes = fs::read(path).unwrap();
                    result.insert(relative, Some((bytes.len(), storage::digest(&bytes))));
                }
            }
        }
        result
    }

    let fixture = Fixture::new();
    // Keep this real Store owner alive across every child; a decoded guard is
    // not qualification or permission to bypass its exclusive filesystem lock.
    let (store, group, binding) = fixture.shared();
    let operation = Uuid::new_v4();
    let guard = Condition {
        group,
        mode_epoch: store.sync_mode_snapshot().unwrap().epoch.unwrap(),
        binding_sha256: binding,
        store_generation: serde_json::from_slice(
            &fs::read(fixture.paths().data.join("CURRENT")).unwrap(),
        )
        .unwrap(),
        local_config_sha256: storage::digest(&fs::read(fixture.0.join("config.json")).unwrap()),
        selection: SelectionCondition::Absent {},
    };
    guard.validate().unwrap();
    let patch = json!({"portable_configuration_version":1,"settings":{"personalization":{"text":"PRIVATE BUSY PATCH"}}});
    let patch_sha256 = Artifact::from_value(patch.clone()).unwrap().sha256();
    let before = snapshot(&fixture.0);
    for (selector, input) in [
        (
            "inspect",
            json!({"shared_preferences_version":1,"reveal_text":true}),
        ),
        (
            "edit",
            json!({"shared_preferences_version":1,"operation":operation,"guard":guard,"patch":patch}),
        ),
        (
            "status",
            json!({"shared_preferences_version":1,"operation":operation,"guard":guard,"patch_sha256":patch_sha256}),
        ),
    ] {
        // Existing run() checks the real executable's ten-second child bound,
        // single bounded JSON line, empty stderr and private-sentinel exclusion.
        let report = fixture.run(selector, &serde_json::to_vec(&input).unwrap());
        let mut expected = json!({
            "report_version":1,"operation":selector,"status":"busy",
            "disposition":"not-dispatched","runtime_activation":"unverified"
        });
        if selector != "inspect" {
            expected["original_operation"] = input["operation"].clone();
            expected["guard"] = input["guard"].clone();
            expected["patch_sha256"] = json!(patch_sha256);
        }
        assert_eq!(report, expected, "busy {selector} correspondence");
        assert!(report.get("view").is_none());
        assert!(report.get("patch").is_none());
        assert_eq!(
            snapshot(&fixture.0),
            before,
            "busy {selector} mutated fixture"
        );
    }
    drop(store);
}

#[test]
fn original_status_is_an_actual_offline_cli_after_mode_and_local_config_change() {
    let fixture = Fixture::new();
    let (store, group, binding) = fixture.shared();
    let operation = Uuid::new_v4();
    let guard = Condition {
        group,
        mode_epoch: store.sync_mode_snapshot().unwrap().epoch.unwrap(),
        binding_sha256: binding.clone(),
        store_generation: serde_json::from_slice(
            &fs::read(fixture.paths().data.join("CURRENT")).unwrap(),
        )
        .unwrap(),
        local_config_sha256: storage::digest(&fs::read(fixture.0.join("config.json")).unwrap()),
        selection: SelectionCondition::Absent {},
    };
    let patch=Artifact::from_value(json!({"portable_configuration_version":1,"settings":{"personalization":{"text":"PRIVATE RESULT"}}})).unwrap();
    let patch_sha256 = patch.sha256();
    let envelope = record::prepare(store.actor_id, operation, &guard, None, &patch)
        .unwrap()
        .unwrap();
    let bytes = serde_json::to_vec(&envelope).unwrap();
    let hash = storage::digest(&bytes);
    let scope = SelectionScope {
        group,
        key_sha256: record::key_sha256(),
        binding_sha256: binding,
    };
    store
        .initialize_selected(
            &scope,
            SelectionChange {
                operation,
                accepted_base_sha256: storage::digest(
                    b"synthetic original receipt, not live admission",
                ),
                selected: storage::Manifest {
                    format: 1,
                    transaction_id: operation,
                    scope: storage::Scope::SelectedRecords,
                    records: vec![storage::ObjectRef {
                        sha256: hash.clone(),
                        bytes: bytes.len() as u64,
                    }],
                    record_namespaces: BTreeMap::from([(
                        hash.clone(),
                        storage::Namespace::PortableConfiguration,
                    )]),
                    media: vec![],
                    media_coverage: vec![],
                },
                retained: vec![],
            },
            BTreeMap::from([(hash, bytes)]),
        )
        .unwrap();
    store
        .set_sync_mode(
            &store.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            mode::SyncModeChoice::LocalOnly,
        )
        .unwrap();
    drop(store);
    fs::write(
        fixture.0.join("config.json"),
        serde_json::to_vec(
            &json!({"paths":fixture.paths(),"personalization":{"text":"PRIVATE CHANGED CONFIG"}}),
        )
        .unwrap(),
    )
    .unwrap();
    fs::write(
        fixture.paths().credentials.join("drive.json"),
        b"PRIVATE INVALID CREDENTIAL",
    )
    .unwrap();
    let mut query = json!({"shared_preferences_version":1,"operation":operation,"guard":guard,"patch_sha256":patch_sha256});
    let report = fixture.run("status", &serde_json::to_vec(&query).unwrap());
    assert_eq!(report["status"], "ok");
    assert_eq!(report["disposition"], "local-committed");
    assert_eq!(report["guard"], query["guard"]);
    assert_eq!(report["original_operation"], query["operation"]);
    assert_eq!(report["patch_sha256"], query["patch_sha256"]);
    assert!(report.get("view").is_none());
    query["patch_sha256"] = json!(storage::digest(b"wrong original patch"));
    assert_eq!(
        fixture.run("status", &serde_json::to_vec(&query).unwrap())["disposition"],
        "unknown"
    );
    assert!(!fixture.paths().data.join("sync/state.json").exists());
}

#[test]
fn selector_requires_its_stdin_mate_and_conflicts_before_any_other_dispatch() {
    let fixture = Fixture::new();
    for extras in [
        vec!["--sync-mode-operation", "inspect"],
        vec!["--writer-note"],
        vec!["--model", "PRIVATE"],
        vec!["--handwriting"],
        vec!["--configuration-operation", "inspect"],
    ] {
        let mut cmd = fixture.command();
        cmd.args([
            "--shared-preferences-operation",
            "inspect",
            "--shared-preferences-input",
            "-",
        ])
        .args(extras);
        assert!(!fixture.output(cmd, b"").status.success());
    }
    for args in [
        vec!["--shared-preferences-operation", "inspect"],
        vec!["--shared-preferences-input", "-"],
    ] {
        let mut cmd = fixture.command();
        cmd.args(args);
        assert!(!fixture.output(cmd, b"").status.success());
    }
    let mut cmd = fixture.command();
    cmd.args([
        "--shared-preferences-operation",
        "inspect",
        "--shared-preferences-input",
        "PRIVATE_FILE",
    ]);
    let output = fixture.output(cmd, b"");
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["status"],
        "invalid-input"
    );
    assert!(!fixture.paths().data.exists());
}

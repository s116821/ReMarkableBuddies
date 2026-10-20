use remarkable_reader_buddy::{
    config::{
        personalization::{self, Operation, Status, MAX_TEXT_BYTES},
        Config, Personalization,
    },
    storage::{files, Store, StorePaths, Uuid},
};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

struct Fixture {
    root: PathBuf,
    path: PathBuf,
    config: Config,
}
impl Fixture {
    fn new(existing: bool) -> Self {
        let root = std::env::temp_dir().join(format!("buddy-personalization-{}", Uuid::new_v4()));
        files::directory(&root).unwrap();
        let config = Config {
            model: Some("unchanged-model".into()),
            paths: StorePaths {
                data: root.join("data"),
                cache: root.join("cache"),
                credentials: root.join("credentials"),
            },
            ..Default::default()
        };
        if existing {
            drop(Store::open(config.paths.clone()).unwrap());
        }
        let path = root.join("config.json");
        files::atomic_json(&path, &config).unwrap();
        Self { root, path, config }
    }
    fn inspect(&self) -> personalization::Report {
        personalization::run(Operation::Inspect, Some(self.path.clone()), &mut &b""[..])
    }
    fn run(&self, operation: Operation, request: Value) -> personalization::Report {
        personalization::run(
            operation,
            Some(self.path.clone()),
            &mut serde_json::to_vec(&request).unwrap().as_slice(),
        )
    }
    fn request(&self) -> Value {
        json!({"contract_version":1,"expected_config_sha256":self.inspect().config_sha256.unwrap()})
    }
    fn command(&self, operation: &str, raw: &[u8]) -> std::process::Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
            .args(["--personalization-operation", operation])
            .env("REMARKABLE_BUDDIES_CONFIG", &self.path)
            .env_remove("OPENAI_API_KEY")
            .env_remove("OPENAI_BASE_URL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(raw).unwrap();
        child.wait_with_output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn defaults_byte_bounds_versions_and_debug_redaction() {
    let old: Config = serde_json::from_str("{}").unwrap();
    assert_eq!(old.personalization, Personalization::default());
    assert!(old
        .personalization_snapshot()
        .unwrap()
        .effective_text()
        .is_none());
    let mut preference = Personalization {
        enabled: true,
        text: "  Unicode 🦊\r\n第二行\n\t ".into(),
        ..Default::default()
    };
    assert_eq!(
        preference.snapshot().unwrap().effective_text(),
        Some(preference.text.as_str())
    );
    assert!(!format!("{preference:?}").contains("Unicode"));
    preference.text = "x".repeat(MAX_TEXT_BYTES);
    assert!(preference.validate().is_ok());
    preference.text.push('é');
    assert!(preference.validate().is_err());
    preference.text = "\0".into();
    assert!(preference.validate().is_err());
    preference.text.clear();
    preference.schema_version = 2;
    assert!(preference.validate().is_err());
    assert!(
        serde_json::from_str::<Personalization>(r#"{"enabled":true,"enabled":false}"#).is_err()
    );
}

#[test]
fn exact_roundtrip_disable_clear_and_immutable_verified_reload() {
    let fixture = Fixture::new(true);
    let text = "  Respect current request 🦊\r\n第二行\n\t ";
    let before = fixture.config.personalization_snapshot().unwrap();
    let history = fixture.config.paths.data.join("retained-history.fixture");
    fs::write(&history, b"retain history/learning").unwrap();
    let mut request = fixture.request();
    request["enabled"] = json!(true);
    request["text"] = json!(text);
    let saved = fixture.run(Operation::Set, request);
    assert_eq!(saved.status, Status::Ok);
    assert!(saved.disk_reload_verified);
    assert_eq!(saved.runtime_activation, "restart-required");
    let reloaded = Config::load(Some(fixture.path.clone())).unwrap();
    assert_eq!(reloaded.model, fixture.config.model);
    let in_flight = reloaded.personalization_snapshot().unwrap();
    assert_eq!(in_flight.effective_text(), Some(text));
    assert!(before.effective_text().is_none());
    let disabled = fixture.run(Operation::Disable, fixture.request());
    assert_eq!(disabled.status, Status::Ok);
    assert_eq!(disabled.personalization.unwrap().text, text);
    assert!(Config::load(Some(fixture.path.clone()))
        .unwrap()
        .personalization_snapshot()
        .unwrap()
        .effective_text()
        .is_none());
    let cleared = fixture.run(Operation::Clear, fixture.request());
    assert_eq!(cleared.personalization.unwrap(), Personalization::default());
    assert_eq!(in_flight.effective_text(), Some(text));
    assert_eq!(fs::read(history).unwrap(), b"retain history/learning");
}

#[test]
fn revision_conflict_busy_and_missing_store_preserve_config_bytes() {
    let fixture = Fixture::new(true);
    let bytes = fs::read(&fixture.path).unwrap();
    let stale = json!({"contract_version":1,"expected_config_sha256":"0".repeat(64)});
    assert_eq!(
        fixture.run(Operation::Clear, stale).status,
        Status::Conflict
    );
    let active = Store::open(fixture.config.paths.clone()).unwrap();
    assert_eq!(
        fixture.run(Operation::Disable, fixture.request()).status,
        Status::Busy
    );
    let child = fixture.command("clear", &serde_json::to_vec(&fixture.request()).unwrap());
    assert!(child.status.success());
    assert!(child.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&child.stdout).unwrap()["status"],
        "busy"
    );
    drop(active);
    assert_eq!(fs::read(&fixture.path).unwrap(), bytes);
    let missing = Fixture::new(false);
    let bytes = fs::read(&missing.path).unwrap();
    assert_eq!(
        missing.run(Operation::Clear, missing.request()).status,
        Status::Unavailable
    );
    assert!(!missing.config.paths.data.exists());
    assert_eq!(fs::read(&missing.path).unwrap(), bytes);
}

#[test]
fn invalid_stdin_and_unsupported_config_fail_without_echo_or_writes() {
    let fixture = Fixture::new(true);
    let bytes = fs::read(&fixture.path).unwrap();
    for raw in [
        b"\xff".to_vec(),
        b"{\"text\":\"PRIVATE_SENTINEL\",\"text\":\"duplicate\"}".to_vec(),
        vec![b' '; personalization::MAX_IO_BYTES + 1],
    ] {
        let result = personalization::run(
            Operation::Set,
            Some(fixture.path.clone()),
            &mut raw.as_slice(),
        );
        assert_eq!(result.status, Status::InvalidInput);
        let mut report = Vec::new();
        result.write_to(&mut report).unwrap();
        assert!(!String::from_utf8(report)
            .unwrap()
            .contains("PRIVATE_SENTINEL"));
        assert_eq!(fs::read(&fixture.path).unwrap(), bytes);
    }
    let mut request = fixture.request();
    request["enabled"] = json!(false);
    assert_eq!(
        fixture.run(Operation::Disable, request).status,
        Status::InvalidInput
    );
    let mut config = fixture.config.clone();
    config.personalization.schema_version = 2;
    files::atomic_json(&fixture.path, &config).unwrap();
    assert_ne!(fixture.inspect().status, Status::Ok);
}

#[test]
fn actual_cli_set_readback_disable_clear_needs_no_credentials_or_device() {
    let fixture = Fixture::new(true);
    let original = fixture.command("inspect", b"");
    assert!(original.status.success());
    assert!(original.stderr.is_empty());
    let report: Value = serde_json::from_slice(&original.stdout).unwrap();
    let text = "CLI Unicode 🦊\r\nnext line\n ";
    let input = json!({"contract_version":1,"expected_config_sha256":report["config_sha256"],"enabled":true,"text":text});
    let result = fixture.command("set", &serde_json::to_vec(&input).unwrap());
    assert!(result.status.success());
    assert!(result.stderr.is_empty());
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["status"], "ok");
    assert_eq!(report["personalization"]["text"], text);
    for operation in ["disable", "clear"] {
        let input = fixture.request();
        let result = fixture.command(operation, &serde_json::to_vec(&input).unwrap());
        assert!(result.status.success());
        assert!(result.stderr.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout).unwrap()["status"],
            "ok"
        );
    }
    let result = fixture.command("set", b"PRIVATE_SENTINEL");
    assert!(result.stderr.is_empty());
    assert!(!String::from_utf8(result.stdout)
        .unwrap()
        .contains("PRIVATE_SENTINEL"));
}

fn tree_bytes(root: &std::path::Path) -> std::collections::BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut result = std::collections::BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if path.is_dir() {
                result.insert(relative, None);
                pending.push(path);
            } else if path.file_name().is_some_and(|name| name == "store.lock") {
                // Windows locks the file's byte range while the Store is held.
                // These fixtures use an empty lock; metadata still proves that.
                assert_eq!(fs::metadata(&path).unwrap().len(), 0);
                result.insert(relative, Some(Vec::new()));
            } else {
                result.insert(relative, Some(fs::read(path).unwrap()));
            }
        }
    }
    result
}

#[test]
fn incomplete_existing_store_is_not_initialized_or_repaired() {
    for missing in [
        "identity",
        "lock",
        "current",
        "cache-owner",
        "credentials-owner",
        "generation-format",
        "objects",
        "commits",
    ] {
        let fixture = Fixture::new(true);
        let generation: Uuid =
            files::json(&fixture.config.paths.data.join("CURRENT"), 128).unwrap();
        let generation_root = fixture
            .config
            .paths
            .data
            .join("generations")
            .join(generation.to_string());
        let path = match missing {
            "identity" => fixture.config.paths.data.join("identity.json"),
            "lock" => fixture.config.paths.data.join("store.lock"),
            "current" => fixture.config.paths.data.join("CURRENT"),
            "cache-owner" => fixture.config.paths.cache.join("owner.json"),
            "credentials-owner" => fixture.config.paths.credentials.join("owner.json"),
            "generation-format" => generation_root.join("format.json"),
            other => generation_root.join(other),
        };
        if path.is_dir() {
            fs::remove_dir(&path).unwrap();
        } else {
            fs::remove_file(&path).unwrap();
        }
        let before = tree_bytes(&fixture.root);
        assert_eq!(
            fixture.run(Operation::Clear, fixture.request()).status,
            Status::Unavailable,
            "{missing}"
        );
        assert_eq!(tree_bytes(&fixture.root), before, "{missing}");
        assert!(!path.exists(), "{missing}");
    }
}

#[test]
fn invalid_ownership_or_generation_refuses_and_releases_lease() {
    for corrupt in [
        "foreign-owner",
        "unsupported-format",
        "nil-generation",
        "broken-commit",
    ] {
        let fixture = Fixture::new(true);
        let generation: Uuid =
            files::json(&fixture.config.paths.data.join("CURRENT"), 128).unwrap();
        let generation_root = fixture
            .config
            .paths
            .data
            .join("generations")
            .join(generation.to_string());
        let (path, invalid) = match corrupt {
            "foreign-owner" => (
                fixture.config.paths.cache.join("owner.json"),
                json!({"format":1,"actor":Uuid::new_v4()}),
            ),
            "unsupported-format" => (generation_root.join("format.json"), json!(2)),
            "nil-generation" => (
                fixture.config.paths.data.join("CURRENT"),
                json!(Uuid::nil()),
            ),
            _ => (
                generation_root.join("commits").join("invalid.json"),
                json!({"invalid":true}),
            ),
        };
        let original = fs::read(&path).ok();
        fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        let before = tree_bytes(&fixture.root);
        assert_eq!(
            fixture.run(Operation::Clear, fixture.request()).status,
            Status::Unsupported,
            "{corrupt}"
        );
        assert_eq!(tree_bytes(&fixture.root), before, "{corrupt}");
        if let Some(bytes) = original {
            fs::write(&path, bytes).unwrap();
        } else {
            fs::remove_file(&path).unwrap();
        }
        // Refusal must not leak the lease. A new OS process can now mutate.
        let child = fixture.command("clear", &serde_json::to_vec(&fixture.request()).unwrap());
        assert!(child.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&child.stdout).unwrap()["status"],
            "ok",
            "{corrupt}"
        );
    }
}

#[test]
fn existing_store_open_only_reads_and_holds_one_lease() {
    let fixture = Fixture::new(true);
    let staging = fixture.config.paths.data.join(".stage-retained-fixture");
    fs::write(staging, b"maintenance does not clean or initialize").unwrap();
    let before = tree_bytes(&fixture.root);
    let store = Store::open_existing(fixture.config.paths.clone()).unwrap();
    assert_eq!(tree_bytes(&fixture.root), before);
    assert!(matches!(
        Store::open_existing(fixture.config.paths.clone()),
        Err(remarkable_reader_buddy::storage::maintenance::Refusal::Busy)
    ));
    assert!(Store::open(fixture.config.paths.clone()).is_err());
    assert_eq!(tree_bytes(&fixture.root), before);
    drop(store);
    assert_eq!(
        fixture.run(Operation::Clear, fixture.request()).status,
        Status::Ok
    );
}

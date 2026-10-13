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

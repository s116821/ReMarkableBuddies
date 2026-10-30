//! Three saved declarations through the existing conditional configuration writer.
use super::basic::{checked_document, Custody, Ordered};
pub use super::basic::{Operation, Status};
use super::{inspection, Config};
use crate::storage::{
    digest, files, maintenance::Refusal, valid_digest, ConfigPublication, Store, Uuid, MAX_RECORD,
};
use anyhow::{ensure, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::PathBuf,
    sync::Arc,
};

pub const MAX_IO: usize = 16_384;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Outcome {
    ReadOnly,
    Prepared,
    NoChange,
    Verified,
    DesiredFileObserved,
    Unresolved,
    NotDispatched,
    Unknown,
}
#[derive(Clone, PartialEq, Eq, Serialize)]
struct Field {
    origin: &'static str,
    value: Value,
}
type Fields = BTreeMap<&'static str, Field>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Setting {
    ExportDestination,
    ReaderEnabled,
    WriterEnabled,
}
impl Setting {
    fn name(self) -> &'static str {
        match self {
            Self::ReaderEnabled => "reader_enabled",
            Self::WriterEnabled => "writer_enabled",
            Self::ExportDestination => "export_destination",
        }
    }
}
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
enum Change {
    Set { value: Value },
    Clear {},
}
type Changes = BTreeMap<Setting, Change>;
fn valid_changes(changes: &Changes) -> bool {
    !changes.is_empty()
        && changes.iter().all(|(setting, change)| match change {
            Change::Clear {} => true,
            Change::Set { value } => match setting {
                Setting::ReaderEnabled | Setting::WriterEnabled => value.is_boolean(),
                Setting::ExportDestination => value
                    .as_str()
                    .is_some_and(|v| ["mem", "disabled"].contains(&v)),
            },
        })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepare {
    product_settings_version: i128,
    operation: Uuid,
    base_sha256: String,
    config_path_sha256: String,
    changes: Changes,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    product_settings_version: i128,
    operation: Uuid,
    base_sha256: String,
    target_sha256: String,
    config_path_sha256: String,
    changes: Changes,
}
impl Intent {
    fn validate(&self) -> std::result::Result<(), Status> {
        if self.product_settings_version != 1 {
            return Err(Status::Unsupported);
        }
        if self.operation.is_nil()
            || !valid_digest(&self.base_sha256)
            || !valid_digest(&self.target_sha256)
            || !valid_digest(&self.config_path_sha256)
            || self.base_sha256 == self.target_sha256
            || !valid_changes(&self.changes)
        {
            return Err(Status::InvalidInput);
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IntentRequest {
    intent: Intent,
}
#[derive(Serialize)]
#[serde(untagged)]
enum Data {
    Inspection {
        schema_version: u32,
        base_sha256: String,
        config_path_sha256: String,
        current: Fields,
    },
    Prepared {
        before: Fields,
        after: Fields,
        intent: Intent,
    },
    NoChange {
        current: Fields,
        base_sha256: String,
    },
    Applied {
        current: Fields,
        target_sha256: String,
    },
    Observed {
        current: Fields,
        observed_sha256: String,
    },
}
#[derive(Serialize)]
pub struct Report {
    report_version: u32,
    operation: Operation,
    pub status: Status,
    outcome: Outcome,
    disk_readback_verified: bool,
    runtime_activation: &'static str,
    data: Option<Data>,
    #[serde(skip)]
    _custody: Option<Custody>,
}
impl Report {
    fn refusal(operation: Operation, status: Status) -> Self {
        Self {
            report_version: 1,
            operation,
            status,
            outcome: if matches!(operation, Operation::Inspect | Operation::Status) {
                Outcome::ReadOnly
            } else {
                Outcome::NotDispatched
            },
            disk_readback_verified: false,
            runtime_activation: "unverified",
            data: None,
            _custody: None,
        }
    }
    fn success(operation: Operation, outcome: Outcome, data: Data) -> Self {
        let mut report = Self::refusal(operation, Status::Ok);
        report.outcome = outcome;
        report.data = Some(data);
        report
    }
    /// Consumes any apply custody only after this single write completes or fails.
    pub fn write_to(self, writer: &mut impl Write) -> Result<()> {
        let mut bytes = serde_json::to_vec(&self)?;
        bytes.push(b'\n');
        ensure!(
            bytes.len() <= MAX_IO,
            "product settings report exceeds bound"
        );
        writer.write_all(&bytes)?;
        Ok(())
    }
}
pub fn invalid_input(operation: Operation) -> Report {
    Report::refusal(operation, Status::InvalidInput)
}
pub fn unsupported_input(operation: Operation) -> Report {
    Report::refusal(operation, Status::Unsupported)
}

struct Document {
    path: PathBuf,
    path_hash: String,
    bytes: Vec<u8>,
    hash: String,
    config: Config,
    raw: Value,
}
impl Document {
    fn load(path: Option<PathBuf>) -> std::result::Result<Self, Status> {
        let path = path
            .filter(|p| p.is_absolute())
            .ok_or(Status::Unavailable)?;
        let utf8 = path.to_str().ok_or(Status::Unavailable)?;
        let path_hash =
            digest(format!("urn:remarkable-buddies:product-settings-path:v1:{utf8}").as_bytes());
        let bytes = files::read(&path, MAX_RECORD as u64).map_err(|_| Status::Unavailable)?;
        let (config, raw) = checked_document(&bytes)?;
        Ok(Self {
            path,
            path_hash,
            hash: digest(&bytes),
            bytes,
            config,
            raw,
        })
    }
    fn fields(&self) -> Fields {
        fields(&self.config, &self.raw)
    }
}
fn fields(config: &Config, raw: &Value) -> Fields {
    let effective = config.product_settings.clone().unwrap_or_default();
    [
        ("reader_enabled", json!(effective.reader_enabled)),
        ("writer_enabled", json!(effective.writer_enabled)),
        ("export_destination", json!(effective.export_destination)),
    ]
    .into_iter()
    .map(|(name, value)| {
        let origin = if raw
            .get("product_settings")
            .and_then(|p| p.get(name))
            .is_some()
        {
            "declared"
        } else {
            "default"
        };
        (name, Field { origin, value })
    })
    .collect()
}
fn parse<T: DeserializeOwned>(bytes: &[u8]) -> std::result::Result<T, Status> {
    let value = serde_json::from_slice(bytes).map_err(|_| Status::InvalidInput)?;
    let raw = inspection::strict_json(bytes).map_err(|_| Status::InvalidInput)?;
    if !raw.is_object() || raw.get("intent").is_some_and(|v| !v.is_object()) {
        return Err(Status::InvalidInput);
    }
    if let Some(changes) = raw
        .get("changes")
        .or_else(|| raw.pointer("/intent/changes"))
    {
        if !changes.is_object()
            || changes
                .as_object()
                .is_some_and(|m| m.values().any(|v| !v.is_object()))
        {
            return Err(Status::InvalidInput);
        }
    }
    let body = raw.get("intent").unwrap_or(&raw);
    if body
        .get("operation")
        .and_then(Value::as_str)
        .is_none_or(|s| Uuid::parse_str(s).map_or(true, |u| u.is_nil() || u.to_string() != s))
    {
        return Err(Status::InvalidInput);
    }
    Ok(value)
}

struct Target {
    bytes: Vec<u8>,
    config: Config,
    raw: Value,
    changed: bool,
}
fn target(document: &Document, changes: &Changes) -> std::result::Result<Target, Status> {
    if !valid_changes(changes) {
        return Err(Status::InvalidInput);
    }
    let mut raw = document.raw.clone();
    let object = raw.as_object_mut().ok_or(Status::InvalidInput)?;
    for (key, change) in changes {
        match change {
            Change::Set { value } => {
                let parent = object.entry("product_settings").or_insert(Value::Null);
                if parent.is_null() {
                    *parent = json!({"schema_version":1});
                }
                parent
                    .as_object_mut()
                    .ok_or(Status::InvalidInput)?
                    .insert(key.name().into(), value.clone());
            }
            Change::Clear {} => {
                if let Some(parent) = object
                    .get_mut("product_settings")
                    .and_then(Value::as_object_mut)
                {
                    parent.remove(key.name());
                }
            }
        }
    }
    let changed = raw != document.raw;
    let bytes = if changed {
        serde_json::to_vec(&Ordered(&raw)).map_err(|_| Status::InvalidInput)?
    } else {
        document.bytes.clone()
    };
    if bytes.len() > MAX_RECORD {
        return Err(Status::InvalidInput);
    }
    let (config, raw) = checked_document(&bytes)?;
    Ok(Target {
        bytes,
        config,
        raw,
        changed,
    })
}

/// Has no stdin argument and never opens an existing Store.
pub fn inspect(path: Option<PathBuf>) -> Report {
    let doc = match Document::load(path) {
        Ok(v) => v,
        Err(status) => return Report::refusal(Operation::Inspect, status),
    };
    let current = doc.fields();
    Report::success(
        Operation::Inspect,
        Outcome::ReadOnly,
        Data::Inspection {
            schema_version: 1,
            base_sha256: doc.hash,
            config_path_sha256: doc.path_hash,
            current,
        },
    )
}

pub fn run_stdin(operation: Operation, path: Option<PathBuf>, stdin: &mut impl Read) -> Report {
    if operation == Operation::Inspect {
        return invalid_input(operation);
    }
    let mut bytes = Vec::new();
    if stdin
        .take((MAX_IO + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > MAX_IO
    {
        return invalid_input(operation);
    }
    match operation {
        Operation::Prepare => {
            let request: Prepare = match parse(&bytes) {
                Ok(v) => v,
                Err(status) => return Report::refusal(operation, status),
            };
            if request.product_settings_version != 1 {
                return Report::refusal(operation, Status::Unsupported);
            }
            if request.operation.is_nil()
                || !valid_digest(&request.base_sha256)
                || !valid_digest(&request.config_path_sha256)
                || !valid_changes(&request.changes)
            {
                return invalid_input(operation);
            }
            let doc = match Document::load(path) {
                Ok(v) => v,
                Err(status) => return Report::refusal(operation, status),
            };
            if doc.hash != request.base_sha256 || doc.path_hash != request.config_path_sha256 {
                return Report::refusal(operation, Status::Conflict);
            }
            let next = match target(&doc, &request.changes) {
                Ok(v) => v,
                Err(status) => return Report::refusal(operation, status),
            };
            if !next.changed {
                return Report::success(
                    operation,
                    Outcome::NoChange,
                    Data::NoChange {
                        current: doc.fields(),
                        base_sha256: doc.hash,
                    },
                );
            }
            let intent = Intent {
                product_settings_version: 1,
                operation: request.operation,
                base_sha256: doc.hash.clone(),
                target_sha256: digest(&next.bytes),
                config_path_sha256: doc.path_hash.clone(),
                changes: request.changes,
            };
            Report::success(
                operation,
                Outcome::Prepared,
                Data::Prepared {
                    before: doc.fields(),
                    after: fields(&next.config, &next.raw),
                    intent,
                },
            )
        }
        Operation::Apply | Operation::Status => {
            let request: IntentRequest = match parse(&bytes) {
                Ok(v) => v,
                Err(status) => return Report::refusal(operation, status),
            };
            if let Err(status) = request.intent.validate() {
                return Report::refusal(operation, status);
            }
            let doc = match Document::load(path) {
                Ok(v) => v,
                Err(status) => return Report::refusal(operation, status),
            };
            if doc.path_hash != request.intent.config_path_sha256 {
                return Report::refusal(operation, Status::Conflict);
            }
            if operation == Operation::Status {
                let outcome = if doc.hash == request.intent.target_sha256 {
                    Outcome::DesiredFileObserved
                } else {
                    Outcome::Unresolved
                };
                let current = doc.fields();
                return Report::success(
                    operation,
                    outcome,
                    Data::Observed {
                        observed_sha256: doc.hash,
                        current,
                    },
                );
            }
            apply(doc, request.intent)
        }
        Operation::Inspect => unreachable!("inspect refused before input"),
    }
}

fn apply(doc: Document, intent: Intent) -> Report {
    apply_with(doc, intent, Store::replace_config_document)
}

fn apply_with(
    doc: Document,
    intent: Intent,
    publish: impl FnOnce(&Store, &std::path::Path, &str, &[u8]) -> ConfigPublication,
) -> Report {
    let operation = Operation::Apply;
    if doc.hash != intent.base_sha256 {
        return Report::refusal(operation, Status::Conflict);
    }
    let next = match target(&doc, &intent.changes) {
        Ok(v) => v,
        Err(status) => return Report::refusal(operation, status),
    };
    if !next.changed || digest(&next.bytes) != intent.target_sha256 {
        return invalid_input(operation);
    }
    if doc.config.validate().is_err() {
        return Report::refusal(operation, Status::Unsupported);
    }
    let store = match Store::open_existing_for_config(doc.config.paths.clone()) {
        Ok(v) => Arc::new(v),
        Err(refusal) => {
            return Report::refusal(
                operation,
                match refusal {
                    Refusal::Busy => Status::Busy,
                    Refusal::Unavailable => Status::Unavailable,
                    Refusal::Unsupported => Status::Unsupported,
                },
            )
        }
    };
    let custody = match Custody::acquire(store.clone()) {
        Ok(v) => v,
        Err(_) => return Report::refusal(operation, Status::Unsupported),
    };
    let mut report = match publish(&store, &doc.path, &intent.base_sha256, &next.bytes) {
        ConfigPublication::Verified(hash) if hash == intent.target_sha256 => {
            let mut report = Report::success(
                operation,
                Outcome::Verified,
                Data::Applied {
                    target_sha256: hash,
                    current: fields(&next.config, &next.raw),
                },
            );
            report.disk_readback_verified = true;
            report.runtime_activation = "restart-required";
            report
        }
        ConfigPublication::Conflict => Report::refusal(operation, Status::Conflict),
        ConfigPublication::Refused => Report::refusal(operation, Status::Failed),
        ConfigPublication::Verified(_) | ConfigPublication::Unknown => {
            let mut report = Report::refusal(operation, Status::Failed);
            report.outcome = Outcome::Unknown;
            report
        }
    };
    report._custody = Some(custody);
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{ConfigFault, StorePaths};
    use std::{fs, io};

    struct Fixture {
        root: PathBuf,
        config: PathBuf,
        paths: StorePaths,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("buddy-product-{}", Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let paths = StorePaths {
                data: root.join("data"),
                cache: root.join("cache"),
                credentials: root.join("credentials"),
            };
            drop(Store::open(paths.clone()).unwrap());
            let config = root.join("config.json");
            fs::write(&config,serde_json::to_vec_pretty(&json!({"paths":paths,"model":"PRIVATE_MODEL","personalization":{"text":"private\r\nα"}})).unwrap()).unwrap();
            Self {
                root,
                config,
                paths,
            }
        }
        fn run(&self, op: Operation, input: &Value) -> Report {
            run_stdin(
                op,
                Some(self.config.clone()),
                &mut serde_json::to_vec(input).unwrap().as_slice(),
            )
        }
        fn prepare(&self) -> Intent {
            let doc = Document::load(Some(self.config.clone())).unwrap();
            let request = json!({"product_settings_version":1,"operation":Uuid::new_v4(),"base_sha256":doc.hash,"config_path_sha256":doc.path_hash,"changes":{"reader_enabled":{"action":"set","value":false}}});
            let report = finish(self.run(Operation::Prepare, &request));
            assert_eq!(report["outcome"], "prepared");
            serde_json::from_value(report["data"]["intent"].clone()).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                let _ = fs::remove_dir_all(&self.root);
            }
        }
    }
    fn finish(report: Report) -> Value {
        let mut bytes = Vec::new();
        report.write_to(&mut bytes).unwrap();
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert!(bytes.len() <= MAX_IO);
        assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE_MODEL"));
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn real_reports_match_six_shared_literal_frames_with_only_captured_identity_substitutions() {
        let corpus: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/product-settings-contract-v1.json"
        ))
        .unwrap();
        let f = Fixture::new();
        let intent = f.prepare();
        let doc = Document::load(Some(f.config.clone())).unwrap();
        let check = |name: &str, report: Report| {
            let case = corpus["raw_protocol"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["id"] == name)
                .unwrap();
            let reference = &corpus["reference"];
            let mut expected = case["raw_utf8"].as_str().unwrap().to_owned();
            for (key, value) in [
                ("base_sha256", &intent.base_sha256),
                ("config_path_sha256", &intent.config_path_sha256),
                ("target_sha256", &intent.target_sha256),
                ("operation", &intent.operation.to_string()),
            ] {
                expected = expected.replace(reference[key].as_str().unwrap(), value);
            }
            let mut actual = Vec::new();
            report.write_to(&mut actual).unwrap();
            assert_eq!(actual, expected.as_bytes(), "{name}");
        };
        check("inspect-defaults", inspect(Some(f.config.clone())));
        let request = json!({"product_settings_version":1,"operation":intent.operation,"base_sha256":doc.hash,"config_path_sha256":doc.path_hash,"changes":intent.changes});
        check("prepare-reader-false", f.run(Operation::Prepare, &request));
        let mut clear = request.clone();
        clear["changes"] = json!({"writer_enabled":{"action":"clear"}});
        check("prepare-no-change", f.run(Operation::Prepare, &clear));
        let input = json!({"intent":intent});
        check("status-unresolved", f.run(Operation::Status, &input));
        check("apply-restart-required", f.run(Operation::Apply, &input));
        check("status-desired", f.run(Operation::Status, &input));
    }

    #[test]
    fn shared_storage_vectors_execute_real_document_and_target_recipe() {
        let corpus: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/product-settings-contract-v1.json"
        ))
        .unwrap();
        assert_eq!(
            digest(include_bytes!(
                "../../tests/fixtures/product-settings-contract-v1.json"
            )),
            "183db82898d9f94fc0f9a8ec853362b3ec1b37062466cee9486850fa52a541fe"
        );
        for case in corpus["storage"].as_array().unwrap() {
            let bytes = case["original_utf8"].as_str().unwrap().as_bytes().to_vec();
            let result = checked_document(&bytes).and_then(|(config, raw)| {
                let doc = Document {
                    path: PathBuf::new(),
                    path_hash: String::new(),
                    hash: digest(&bytes),
                    bytes,
                    config,
                    raw,
                };
                let changes: Changes = serde_json::from_value(case["changes"].clone())
                    .map_err(|_| Status::InvalidInput)?;
                target(&doc, &changes)
            });
            match result {
                Ok(next) => {
                    assert_eq!(case["expected_status"], "ok", "{}", case["id"]);
                    assert_eq!(
                        next.bytes,
                        case["expected_target_utf8"].as_str().unwrap().as_bytes(),
                        "{}",
                        case["id"]
                    );
                    assert_eq!(
                        next.changed,
                        case["expected_changed"].as_bool().unwrap(),
                        "{}",
                        case["id"]
                    );
                }
                Err(status) => assert_eq!(json!(status), case["expected_status"], "{}", case["id"]),
            }
        }
        let reference = &corpus["reference"];
        assert_eq!(
            digest(reference["target_utf8"].as_str().unwrap().as_bytes()),
            reference["target_sha256"]
        );
        assert_eq!(
            digest(
                format!(
                    "urn:remarkable-buddies:product-settings-path:v1:{}",
                    reference["config_path"].as_str().unwrap()
                )
                .as_bytes()
            ),
            reference["config_path_sha256"]
        );
    }

    #[test]
    fn actual_apply_preserves_private_siblings_and_status_never_claims_activation() {
        let f = Fixture::new();
        let before: Value = serde_json::from_slice(&fs::read(&f.config).unwrap()).unwrap();
        let intent = f.prepare();
        let input = json!({"intent":intent});
        let unresolved = finish(f.run(Operation::Status, &input));
        assert_eq!(unresolved["outcome"], "unresolved");
        assert_eq!(unresolved["data"]["observed_sha256"], intent.base_sha256);
        let applied = finish(f.run(Operation::Apply, &input));
        assert_eq!(applied["outcome"], "verified");
        assert_eq!(applied["disk_readback_verified"], true);
        assert_eq!(applied["runtime_activation"], "restart-required");
        let after: Value = serde_json::from_slice(&fs::read(&f.config).unwrap()).unwrap();
        for (k, v) in before.as_object().unwrap() {
            assert_eq!(&after[k], v);
        }
        assert_eq!(
            after["product_settings"],
            json!({"schema_version":1,"reader_enabled":false})
        );
        let status = finish(f.run(Operation::Status, &input));
        assert_eq!(status["outcome"], "desired-file-observed");
        assert_eq!(status["disk_readback_verified"], false);
        assert_eq!(status["runtime_activation"], "unverified");
        assert_eq!(
            finish(f.run(Operation::Apply, &input))["status"],
            "conflict"
        );
    }

    #[test]
    fn busy_stale_path_and_reconstructed_target_refuse_without_writes() {
        let f = Fixture::new();
        let intent = f.prepare();
        let original = fs::read(&f.config).unwrap();
        let owner = Store::open(f.paths.clone()).unwrap();
        assert_eq!(
            finish(f.run(Operation::Apply, &json!({"intent":intent})))["status"],
            "busy"
        );
        assert_eq!(
            finish(f.run(Operation::Status, &json!({"intent":intent})))["outcome"],
            "unresolved"
        );
        drop(owner);
        let mut changed = intent.clone();
        changed.config_path_sha256 = "0".repeat(64);
        assert_eq!(
            finish(f.run(Operation::Apply, &json!({"intent":changed})))["status"],
            "conflict"
        );
        changed = intent.clone();
        changed.target_sha256 = "0".repeat(64);
        assert_eq!(
            finish(f.run(Operation::Apply, &json!({"intent":changed})))["status"],
            "invalid-input"
        );
        assert_eq!(fs::read(&f.config).unwrap(), original);
        let mut newer = original;
        newer.push(b' ');
        fs::write(&f.config, &newer).unwrap();
        assert_eq!(
            finish(f.run(Operation::Apply, &json!({"intent":intent})))["status"],
            "conflict"
        );
        assert_eq!(fs::read(&f.config).unwrap(), newer);
    }

    #[test]
    fn publication_faults_retain_unknown_and_original_only_observation() {
        for fault in [
            ConfigFault::AtomicCall,
            ConfigFault::AfterPublication,
            ConfigFault::ChangedReadback,
        ] {
            let f = Fixture::new();
            let intent = f.prepare();
            let doc = Document::load(Some(f.config.clone())).unwrap();
            let report = apply_with(doc, intent.clone(), |store, path, base, bytes| {
                store.replace_config_document_fault(path, base, bytes, fault)
            });
            let report = finish(report);
            assert_eq!(report["outcome"], "unknown");
            assert_eq!(report["data"], Value::Null);
            let observed = fs::read(&f.config).unwrap();
            let status = finish(f.run(Operation::Status, &json!({"intent":intent})));
            assert_eq!(
                status["outcome"],
                if digest(&observed) == intent.target_sha256 {
                    "desired-file-observed"
                } else {
                    "unresolved"
                }
            );
            assert_eq!(fs::read(&f.config).unwrap(), observed);
        }
    }

    #[test]
    fn report_write_failure_releases_existing_writer_custody_after_one_attempt() {
        let f = Fixture::new();
        let intent = f.prepare();
        let report = f.run(Operation::Apply, &json!({"intent":intent}));
        assert_eq!(report.status, Status::Ok);
        assert!(matches!(
            Store::open_existing(f.paths.clone()),
            Err(Refusal::Busy)
        ));
        struct Prefix(usize);
        impl Write for Prefix {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0 += 1;
                if self.0 == 1 {
                    Ok(bytes.len().min(17))
                } else {
                    Err(io::Error::other("stopped"))
                }
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut output = Prefix(0);
        assert!(report.write_to(&mut output).is_err());
        assert_eq!(output.0, 2);
        drop(Store::open_existing(f.paths.clone()).unwrap());
        assert_eq!(digest(&fs::read(&f.config).unwrap()), intent.target_sha256);
    }

    #[test]
    fn strict_input_refuses_before_file_access_and_inspect_never_reads() {
        struct Never;
        impl Read for Never {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                panic!("unexpected stdin")
            }
        }
        assert_eq!(
            run_stdin(Operation::Inspect, None, &mut Never).status,
            Status::InvalidInput
        );
        let valid = json!({"product_settings_version":1,"operation":"11111111-1111-4111-8111-111111111111","base_sha256":"0".repeat(64),"config_path_sha256":"1".repeat(64),"changes":{"reader_enabled":{"action":"set","value":false}}});
        for (path, value) in [
            ("/changes", json!({})),
            ("/changes", json!([])),
            ("/changes", json!({"reader_enabled":["clear"]})),
            (
                "/changes",
                json!({"reader_enabled":{"action":"clear","value":null}}),
            ),
            (
                "/changes",
                json!({"reader_enabled":{"action":"set","value":null}}),
            ),
            ("/operation", json!("11111111111141118111111111111111")),
            ("/base_sha256", json!("A".repeat(64))),
            ("/product_settings_version", json!(1.0)),
        ] {
            let mut value_request = valid.clone();
            *value_request.pointer_mut(path).unwrap() = value;
            assert_eq!(
                run_stdin(
                    Operation::Prepare,
                    None,
                    &mut serde_json::to_vec(&value_request).unwrap().as_slice()
                )
                .status,
                Status::InvalidInput,
                "{value_request}"
            );
        }
        let bytes = serde_json::to_vec(&valid).unwrap();
        for malformed in [
            b"[]".to_vec(),
            [bytes.clone(), b" {}".to_vec()].concat(),
            vec![b' '; MAX_IO + 1],
            String::from_utf8(bytes)
                .unwrap()
                .replace(
                    "\"product_settings_version\":1",
                    "\"product_settings_version\":1,\"product_settings_version\":1",
                )
                .into_bytes(),
        ] {
            assert_eq!(
                run_stdin(Operation::Prepare, None, &mut malformed.as_slice()).status,
                Status::InvalidInput
            );
        }
        let mut future = valid;
        future["product_settings_version"] = json!(2);
        assert_eq!(
            run_stdin(
                Operation::Prepare,
                None,
                &mut serde_json::to_vec(&future).unwrap().as_slice()
            )
            .status,
            Status::Unsupported
        );
    }

    #[test]
    fn multifield_intent_and_changes_use_the_frozen_lexical_order() {
        let changes:Changes=serde_json::from_str(r#"{"writer_enabled":{"action":"clear"},"reader_enabled":{"action":"set","value":false},"export_destination":{"action":"set","value":"disabled"}}"#).unwrap();
        let intent = Intent {
            product_settings_version: 1,
            operation: Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
            base_sha256: "0".repeat(64),
            target_sha256: "1".repeat(64),
            config_path_sha256: "2".repeat(64),
            changes,
        };
        assert_eq!(
            serde_json::to_string(&intent).unwrap(),
            format!(
                r#"{{"product_settings_version":1,"operation":"11111111-1111-4111-8111-111111111111","base_sha256":"{}","target_sha256":"{}","config_path_sha256":"{}","changes":{{"export_destination":{{"action":"set","value":"disabled"}},"reader_enabled":{{"action":"set","value":false}},"writer_enabled":{{"action":"clear"}}}}}}"#,
                "0".repeat(64),
                "1".repeat(64),
                "2".repeat(64)
            )
        );
    }

    #[test]
    fn existing_snapshot_export_restore_keeps_valid_settings_inactive() {
        let source = Fixture::new();
        let config:Config=serde_json::from_value(json!({"paths":source.paths,"product_settings":{"schema_version":1,"reader_enabled":false,"writer_enabled":true,"export_destination":"disabled"}})).unwrap();
        let store = Store::open(source.paths.clone()).unwrap();
        store.snapshot_config(&config).unwrap();
        let snapshot = fs::read(source.paths.data.join("config.snapshot.json")).unwrap();
        let archive = source.root.join("archive");
        store.export(&archive).unwrap();
        drop(store);
        let backup: Value =
            serde_json::from_slice(&fs::read(archive.join("backup.json")).unwrap()).unwrap();
        assert_eq!(
            backup["config_snapshot"]["product_settings"],
            json!({"schema_version":1,"reader_enabled":false,"writer_enabled":true,"export_destination":"disabled"})
        );
        let target = Fixture::new();
        let live_config = fs::read(&target.config).unwrap();
        let restored = Store::open(target.paths.clone()).unwrap();
        restored.restore(&archive).unwrap();
        drop(restored);
        let current: Uuid =
            serde_json::from_slice(&fs::read(target.paths.data.join("CURRENT")).unwrap()).unwrap();
        let bytes = fs::read(
            target
                .paths
                .data
                .join("generations")
                .join(current.to_string())
                .join("restored-config.json"),
        )
        .unwrap();
        assert_eq!(bytes, snapshot);
        let saved: Config = serde_json::from_slice(&bytes).unwrap();
        assert!(!saved.reader_enabled());
        assert!(saved.admits_writer(false));
        assert!(!saved.admits_writer(true));
        assert_eq!(fs::read(&target.config).unwrap(), live_config);
    }
}

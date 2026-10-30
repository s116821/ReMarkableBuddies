//! Three saved declarations through the existing conditional configuration writer.
use super::{inspection, Config};
use crate::storage::{
    digest, files,
    maintenance::Refusal,
    sync::mode::{OrdinaryModeLease, SyncMode, SyncModeLease},
    valid_digest, ConfigPublication, Store, Uuid, MAX_RECORD,
};
use anyhow::{ensure, Result};
use serde::{
    de::DeserializeOwned,
    ser::{SerializeMap, SerializeSeq},
    Deserialize, Serialize,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::PathBuf,
    sync::Arc,
};

pub const MAX_IO: usize = 16_384;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    Inspect,
    Prepare,
    Apply,
    Status,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Ok,
    InvalidInput,
    Unsupported,
    Unavailable,
    Busy,
    Conflict,
    Failed,
}
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
    DebugDump,
    LogLevel,
    TriggerCorner,
}
impl Setting {
    fn name(self) -> &'static str {
        match self {
            Self::DebugDump => "debug_dump",
            Self::LogLevel => "log_level",
            Self::TriggerCorner => "trigger_corner",
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
                Setting::DebugDump => value.is_boolean(),
                Setting::TriggerCorner => value
                    .as_str()
                    .is_some_and(|v| ["UR", "UL", "LR", "LL"].contains(&v)),
                Setting::LogLevel => value.as_str().is_some_and(|v| {
                    ["off", "error", "warn", "info", "debug", "trace"].contains(&v)
                }),
            },
        })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepare {
    basic_version: u32,
    operation: Uuid,
    base_sha256: String,
    config_path_sha256: String,
    changes: Changes,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    basic_version: u32,
    operation: Uuid,
    base_sha256: String,
    target_sha256: String,
    config_path_sha256: String,
    changes: Changes,
}
impl Intent {
    fn validate(&self) -> std::result::Result<(), Status> {
        if self.basic_version != 1 {
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
        target_sha256: String,
        current: Fields,
    },
    Observed {
        observed_sha256: String,
        current: Fields,
    },
}
pub(super) struct Custody {
    _store: Arc<Store>,
    _ordinary: Option<OrdinaryModeLease>,
    _explicit: Option<SyncModeLease>,
}
impl Custody {
    pub(super) fn acquire(store: Arc<Store>) -> Result<Self> {
        let current = store.sync_mode_snapshot()?;
        let (ordinary, explicit) = if matches!(current.mode, None | Some(SyncMode::LocalOnly)) {
            (Some(store.ordinary_mode_lease(&current.token)?), None)
        } else {
            (None, Some(store.sync_mode_lease(&current.token)?))
        };
        Ok(Self {
            _store: store,
            _ordinary: ordinary,
            _explicit: explicit,
        })
    }
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
        ensure!(bytes.len() <= MAX_IO, "basic settings report exceeds bound");
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
            digest(format!("urn:remarkable-buddies:basic-settings-path:v1:{utf8}").as_bytes());
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

pub(super) fn checked_document(bytes: &[u8]) -> std::result::Result<(Config, Value), Status> {
    let (config, raw) = inspection::document(bytes).map_err(|status| match status {
        inspection::Status::Unsupported => Status::Unsupported,
        inspection::Status::Unavailable => Status::Unavailable,
        _ => Status::InvalidInput,
    })?;
    Ok((config, raw))
}
fn fields(config: &Config, raw: &Value) -> Fields {
    [
        ("trigger_corner", json!(config.trigger_corner)),
        ("log_level", json!(config.log_level)),
        ("debug_dump", json!(config.debug_dump)),
    ]
    .into_iter()
    .map(|(name, value)| {
        let origin = match raw.get(name) {
            None => "default",
            Some(Value::Null) => "declared-null",
            Some(_) => "declared",
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
    Ok(value)
}

/// Explicitly ordered recursive JSON, independent of serde_json Map features.
pub(super) struct Ordered<'a>(pub(super) &'a Value);
impl Serialize for Ordered<'_> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match self.0 {
            Value::Object(values) => {
                let sorted: BTreeMap<_, _> = values.iter().collect();
                let mut map = serializer.serialize_map(Some(sorted.len()))?;
                for (key, value) in sorted {
                    map.serialize_entry(key, &Ordered(value))?;
                }
                map.end()
            }
            Value::Array(values) => {
                let mut seq = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    seq.serialize_element(&Ordered(value))?;
                }
                seq.end()
            }
            value => value.serialize(serializer),
        }
    }
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
                object.insert(key.name().into(), value.clone());
            }
            Change::Clear {} => {
                object.remove(key.name());
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
            if request.basic_version != 1 {
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
                basic_version: 1,
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
            let root = std::env::temp_dir().join(format!("buddy-basic-{}", Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let paths = StorePaths {
                data: root.join("data"),
                cache: root.join("cache"),
                credentials: root.join("credentials"),
            };
            drop(Store::open(paths.clone()).unwrap());
            let config = root.join("config.json");
            let value = json!({"paths":paths,"model":null,"base_url":"https://private-url-sentinel.invalid/?key=private-token-sentinel","debug_dump":null,"log_level":null,"memory":{},"personalization":{"text":" exact\r\n\u{03b1} ","enabled":false}});
            fs::write(&config, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
            Self {
                root,
                config,
                paths,
            }
        }
        fn run(&self, operation: Operation, input: &Value) -> Report {
            run_stdin(
                operation,
                Some(self.config.clone()),
                &mut serde_json::to_vec(input).unwrap().as_slice(),
            )
        }
        fn prepare(&self, changes: Value) -> Intent {
            let current = finish(inspect(Some(self.config.clone())));
            let report = self.run(Operation::Prepare, &json!({"basic_version":1,"operation":Uuid::new_v4(),"base_sha256":current["data"]["base_sha256"],"config_path_sha256":current["data"]["config_path_sha256"],"changes":changes}));
            let value = finish(report);
            assert_eq!(value["outcome"], "prepared");
            serde_json::from_value(value["data"]["intent"].clone()).unwrap()
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
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn target_recipe_has_independent_pinned_utf8_bytes_and_digest() {
        let bytes = br#"{ "personalization": {"text":"keep\r\n\u03b1"}, "model": null, "debug_dump": null }"#.to_vec();
        let (config, raw) = checked_document(&bytes).unwrap();
        let doc = Document {
            path: PathBuf::from("/synthetic/config/file.json"),
            path_hash: "0".repeat(64),
            hash: digest(&bytes),
            bytes,
            config,
            raw,
        };
        let changes =
            serde_json::from_value(json!({"debug_dump":{"action":"set","value":true}})).unwrap();
        let next = target(&doc, &changes).unwrap();
        assert_eq!(next.bytes, "{\"debug_dump\":true,\"model\":null,\"personalization\":{\"text\":\"keep\\r\\n\u{03b1}\"}}".as_bytes());
        assert_eq!(
            digest(&next.bytes),
            "d933a7fcd13d0efc92bda2b88c8343286e093a2f60edc13c23da1a8df2a44bcb"
        );
    }

    #[test]
    fn oversized_target_refuses_without_changing_a_limit_sized_document() {
        let mut value = json!({"paths":{"data":"","cache":"/synthetic/cache","credentials":"/synthetic/credentials"}});
        let overhead = serde_json::to_vec(&value).unwrap().len();
        value["paths"]["data"] = json!("x".repeat(MAX_RECORD - overhead));
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(bytes.len(), MAX_RECORD);
        let (config, raw) = checked_document(&bytes).unwrap();
        let doc = Document {
            path: PathBuf::from("/synthetic/config/file.json"),
            path_hash: "0".repeat(64),
            hash: digest(&bytes),
            bytes,
            config,
            raw,
        };
        let changes =
            serde_json::from_value(json!({"debug_dump":{"action":"set","value":true}})).unwrap();
        assert!(matches!(target(&doc, &changes), Err(Status::InvalidInput)));
        assert_eq!(doc.bytes.len(), MAX_RECORD);
    }

    #[test]
    fn exact_patch_preserves_unrelated_declarations_and_byte_identical_noop() {
        let f = Fixture::new();
        let original = fs::read(&f.config).unwrap();
        let before: Value = serde_json::from_slice(&original).unwrap();
        let current = finish(inspect(Some(f.config.clone())));
        let no_op = f.run(Operation::Prepare, &json!({"basic_version":1,"operation":Uuid::new_v4(),"base_sha256":current["data"]["base_sha256"],"config_path_sha256":current["data"]["config_path_sha256"],"changes":{"trigger_corner":{"action":"clear"}}}));
        assert_eq!(finish(no_op)["outcome"], "no-change");
        assert_eq!(fs::read(&f.config).unwrap(), original);
        let intent = f.prepare(
            json!({"trigger_corner":{"action":"set","value":"LL"},"log_level":{"action":"clear"}}),
        );
        let prepared_input = json!({"intent":intent});
        assert_eq!(
            finish(f.run(Operation::Status, &prepared_input))["outcome"],
            "unresolved"
        );
        let applied = finish(f.run(Operation::Apply, &prepared_input));
        assert_eq!(applied["outcome"], "verified");
        assert_eq!(applied["disk_readback_verified"], true);
        assert_eq!(applied["runtime_activation"], "restart-required");
        let after: Value = serde_json::from_slice(&fs::read(&f.config).unwrap()).unwrap();
        for (key, value) in before.as_object().unwrap() {
            if key != "log_level" {
                assert_eq!(&after[key], value);
            }
        }
        assert!(after.get("log_level").is_none());
        assert_eq!(after["trigger_corner"], "LL");
        assert_eq!(after["debug_dump"], Value::Null);
        assert!(after.get("config_schema_version").is_none());
        assert!(after.get("sync").is_none());
        assert!(after["personalization"].get("schema_version").is_none());
        assert_eq!(
            finish(f.run(Operation::Status, &prepared_input))["outcome"],
            "desired-file-observed"
        );
        assert_eq!(
            finish(f.run(Operation::Apply, &prepared_input))["status"],
            "conflict"
        );
    }

    #[test]
    fn prepublication_conditions_busy_and_path_target_changes_do_not_write() {
        let f = Fixture::new();
        let intent = f.prepare(json!({"debug_dump":{"action":"set","value":true}}));
        let original = fs::read(&f.config).unwrap();
        let owner = Store::open(f.paths.clone()).unwrap();
        let result = finish(f.run(Operation::Apply, &json!({"intent":intent})));
        assert_eq!(result["status"], "busy");
        assert_eq!(result["outcome"], "not-dispatched");
        assert_eq!(
            finish(f.run(Operation::Status, &json!({"intent":intent})))["outcome"],
            "unresolved"
        );
        drop(owner);
        let mut changed = intent.clone();
        changed.target_sha256 = "0".repeat(64);
        assert_eq!(
            finish(f.run(Operation::Apply, &json!({"intent":changed})))["status"],
            "invalid-input"
        );
        changed = intent.clone();
        changed.config_path_sha256 = "0".repeat(64);
        assert_eq!(
            finish(f.run(Operation::Apply, &json!({"intent":changed})))["status"],
            "conflict"
        );
        let mut newer = original.clone();
        newer.push(b' ');
        fs::write(&f.config, &newer).unwrap();
        let result = finish(f.run(Operation::Apply, &json!({"intent":intent})));
        assert_eq!(result["status"], "conflict");
        assert_eq!(result["outcome"], "not-dispatched");
        assert_eq!(fs::read(&f.config).unwrap(), newer);
    }

    #[test]
    fn actual_atomic_faults_and_changed_readback_are_unknown_not_rollback() {
        for fault in [
            ConfigFault::AtomicCall,
            ConfigFault::AfterPublication,
            ConfigFault::ChangedReadback,
        ] {
            let f = Fixture::new();
            let intent = f.prepare(json!({"debug_dump":{"action":"set","value":true}}));
            let old = fs::read(&f.config).unwrap();
            let doc = Document::load(Some(f.config.clone())).unwrap();
            let result = apply_with(doc, intent.clone(), |store, path, expected, bytes| {
                store.replace_config_document_fault(path, expected, bytes, fault)
            });
            assert_eq!(result.status, Status::Failed);
            assert!(matches!(result.outcome, Outcome::Unknown));
            assert!(!result.disk_readback_verified);
            let result = finish(result);
            assert_eq!(result["data"], Value::Null);
            let observed = fs::read(&f.config).unwrap();
            let status = finish(f.run(Operation::Status, &json!({"intent":intent})));
            if digest(&observed) == intent.target_sha256 {
                assert_ne!(observed, old);
                assert_eq!(status["outcome"], "desired-file-observed");
            } else {
                assert_eq!(status["outcome"], "unresolved");
            }
            assert_eq!(status["runtime_activation"], "unverified");
            assert_eq!(fs::read(&f.config).unwrap(), observed);
        }
    }

    #[test]
    fn report_prefix_failure_releases_custody_after_no_second_report() {
        let f = Fixture::new();
        let intent = f.prepare(json!({"debug_dump":{"action":"set","value":true}}));
        let report = f.run(Operation::Apply, &json!({"intent":intent}));
        assert_eq!(report.status, Status::Ok);
        assert!(matches!(
            Store::open_existing(f.paths.clone()),
            Err(Refusal::Busy)
        ));
        let custody = report._custody.as_ref().unwrap();
        let snapshot = custody._store.sync_mode_snapshot().unwrap();
        assert!(custody
            ._store
            .set_sync_mode(
                &snapshot.token,
                Uuid::new_v4(),
                crate::storage::sync::mode::SyncModeChoice::LocalOnly
            )
            .is_err());
        struct Prefix {
            bytes: Vec<u8>,
            calls: usize,
        }
        impl Write for Prefix {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.calls += 1;
                if self.calls > 1 {
                    return Err(io::Error::other("prefix stopped"));
                }
                self.bytes.extend_from_slice(&bytes[..17]);
                Ok(17)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut output = Prefix {
            bytes: Vec::new(),
            calls: 0,
        };
        assert!(report.write_to(&mut output).is_err());
        assert_eq!(output.calls, 2);
        assert_eq!(output.bytes.len(), 17);
        assert!(serde_json::from_slice::<Value>(&output.bytes).is_err());
        drop(Store::open_existing(f.paths.clone()).unwrap());
        assert_eq!(digest(&fs::read(&f.config).unwrap()), intent.target_sha256);
    }

    #[test]
    fn strict_request_and_change_shapes_refuse_without_config_or_stdin_for_inspect() {
        struct MustNotRead;
        impl Read for MustNotRead {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                panic!("inspect acquired stdin");
            }
        }
        assert_eq!(
            run_stdin(Operation::Inspect, None, &mut MustNotRead).status,
            Status::InvalidInput
        );
        for value in [
            json!([]),
            json!({"intent":[]}),
            json!({"intent":{"basic_version":1}}),
            json!({"changes":{"debug_dump":["clear"]}}),
        ] {
            let result = run_stdin(
                Operation::Apply,
                None,
                &mut serde_json::to_vec(&value).unwrap().as_slice(),
            );
            assert_eq!(result.status, Status::InvalidInput);
        }
        let result = run_stdin(
            Operation::Prepare,
            None,
            &mut vec![b' '; MAX_IO + 1].as_slice(),
        );
        assert_eq!(result.status, Status::InvalidInput);
    }
}

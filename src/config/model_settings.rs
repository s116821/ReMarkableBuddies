//! Private model/endpoint declarations; no provider/runtime activation.
use super::{
    basic::{checked_document, Custody, Ordered, Status},
    inspection, Config,
};
use crate::storage::{
    digest, files, maintenance::Refusal, valid_digest, ConfigPublication, Store, Uuid, MAX_RECORD,
};
use anyhow::{ensure, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
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
    present: bool,
}
type Fields = BTreeMap<&'static str, Field>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Setting {
    Model,
    BaseUrl,
}
impl Setting {
    fn name(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::BaseUrl => "base_url",
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
            Change::Set { value } => value.as_str().is_some_and(|v| {
                !v.trim().is_empty()
                    && v.len()
                        <= match setting {
                            Setting::Model => 256,
                            Setting::BaseUrl => 2048,
                        }
            }),
        })
}
fn patch_hash(changes: &Changes) -> String {
    let raw = serde_json::to_value(changes).expect("fixed changes serialize");
    let mut bytes = b"urn:remarkable-buddies:model-settings-patch:v1:".to_vec();
    serde_json::to_writer(&mut bytes, &Ordered(&raw)).expect("fixed changes serialize");
    digest(&bytes)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepare {
    model_settings_version: u32,
    operation: Uuid,
    base_sha256: String,
    config_path_sha256: String,
    changes: Changes,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    model_settings_version: u32,
    operation: Uuid,
    base_sha256: String,
    target_sha256: String,
    config_path_sha256: String,
    changes_sha256: String,
}
impl Intent {
    fn validate(&self) -> std::result::Result<(), Status> {
        if self.model_settings_version != 1 {
            return Err(Status::Unsupported);
        }
        if self.operation.is_nil()
            || !valid_digest(&self.base_sha256)
            || !valid_digest(&self.target_sha256)
            || !valid_digest(&self.config_path_sha256)
            || self.base_sha256 == self.target_sha256
            || !valid_digest(&self.changes_sha256)
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
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyRequest {
    intent: Intent,
    changes: Changes,
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
        changes_sha256: String,
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
        ensure!(bytes.len() <= MAX_IO, "model settings report exceeds bound");
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
            digest(format!("urn:remarkable-buddies:model-settings-path:v1:{utf8}").as_bytes());
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
    [
        ("model", config.model.is_some()),
        ("base_url", config.base_url.is_some()),
    ]
    .into_iter()
    .map(|(name, present)| {
        let origin = match raw.get(name) {
            None => "default",
            Some(Value::Null) => "declared-null",
            Some(_) => "declared",
        };
        (name, Field { origin, present })
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
            if request.model_settings_version != 1 {
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
                        changes_sha256: patch_hash(&request.changes),
                    },
                );
            }
            let intent = Intent {
                model_settings_version: 1,
                operation: request.operation,
                base_sha256: doc.hash.clone(),
                target_sha256: digest(&next.bytes),
                config_path_sha256: doc.path_hash.clone(),
                changes_sha256: patch_hash(&request.changes),
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
        Operation::Apply => {
            let request: ApplyRequest = match parse(&bytes) {
                Ok(v) => v,
                Err(s) => return Report::refusal(operation, s),
            };
            if let Err(s) = request.intent.validate() {
                return Report::refusal(operation, s);
            }
            if !valid_changes(&request.changes)
                || patch_hash(&request.changes) != request.intent.changes_sha256
            {
                return invalid_input(operation);
            }
            let doc = match Document::load(path) {
                Ok(v) => v,
                Err(s) => return Report::refusal(operation, s),
            };
            if doc.path_hash != request.intent.config_path_sha256 {
                return Report::refusal(operation, Status::Conflict);
            }
            apply(doc, request.intent, request.changes)
        }
        Operation::Status => {
            let request: IntentRequest = match parse(&bytes) {
                Ok(v) => v,
                Err(s) => return Report::refusal(operation, s),
            };
            if let Err(s) = request.intent.validate() {
                return Report::refusal(operation, s);
            }
            let doc = match Document::load(path) {
                Ok(v) => v,
                Err(s) => return Report::refusal(operation, s),
            };
            if doc.path_hash != request.intent.config_path_sha256 {
                return Report::refusal(operation, Status::Conflict);
            }
            if doc.hash != request.intent.target_sha256 {
                let mut report = Report::refusal(operation, Status::Ok);
                report.outcome = Outcome::Unresolved;
                return report;
            }
            let current = doc.fields();
            Report::success(
                operation,
                Outcome::DesiredFileObserved,
                Data::Observed {
                    observed_sha256: doc.hash,
                    current,
                },
            )
        }
        Operation::Inspect => unreachable!("inspect refused before input"),
    }
}

fn apply(doc: Document, intent: Intent, changes: Changes) -> Report {
    apply_with(doc, intent, changes, Store::replace_config_document)
}

fn apply_with(
    doc: Document,
    intent: Intent,
    changes: Changes,
    publish: impl FnOnce(&Store, &std::path::Path, &str, &[u8]) -> ConfigPublication,
) -> Report {
    let operation = Operation::Apply;
    if doc.hash != intent.base_sha256 {
        return Report::refusal(operation, Status::Conflict);
    }
    let next = match target(&doc, &changes) {
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
    use serde_json::json;
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
            let report = self.run(Operation::Prepare, &json!({"model_settings_version":1,"operation":Uuid::new_v4(),"base_sha256":current["data"]["base_sha256"],"config_path_sha256":current["data"]["config_path_sha256"],"changes":changes}));
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
        assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE_"));
        assert!(!String::from_utf8_lossy(&bytes).contains("private-url-sentinel"));
        serde_json::from_slice(&bytes).unwrap()
    }

    fn patch() -> Value {
        json!({"model":{"action":"set","value":" PRIVATE_MODEL\r\n α "},"base_url":{"action":"set","value":"https://PRIVATE_USER:PRIVATE_PASSWORD@invalid/?token=PRIVATE_QUERY#PRIVATE_FRAGMENT\r\n "}})
    }
    #[test]
    fn independent_patch_vector_and_escaped_reordered_equivalence() {
        let raw = r#"{"model":{"value":"\u6a21\u578b\r\n \u03b1\t ","action":"set"},"base_url":{"value":"https://fixture-user:fixture-pass@example.invalid/v1?token=fixture-token#fragment\r\n \t","action":"set"}}"#;
        let changes: Changes = serde_json::from_str(raw).unwrap();
        assert_eq!(
            patch_hash(&changes),
            "7b8b312e53ce731e251614648218ebf89f994b4d03b047e67278ae0b8ff2c226"
        );
        let val = serde_json::to_value(&changes).unwrap();
        let bytes = serde_json::to_vec(&Ordered(&val)).unwrap();
        assert_eq!(bytes.len(), 180);
        assert_eq!(
            patch_hash(&serde_json::from_value(val).unwrap()),
            patch_hash(&changes)
        );
    }
    #[test]
    fn exact_opaque_patch_private_reports_noop_clear_and_no_reapply() {
        let f = Fixture::new();
        let original = fs::read(&f.config).unwrap();
        let before: Value = serde_json::from_slice(&original).unwrap();
        let intent = f.prepare(patch());
        let status_input = json!({"intent":intent});
        let r = finish(f.run(Operation::Status, &status_input));
        assert_eq!(r["outcome"], "unresolved");
        assert!(r["data"].is_null());
        let applied = finish(f.run(
            Operation::Apply,
            &json!({"intent":intent,"changes":patch()}),
        ));
        let output = serde_json::to_string(&applied).unwrap();
        assert!(!output.contains("PRIVATE_"));
        assert!(!output.contains("private-url-sentinel"));
        assert_eq!(applied["outcome"], "verified");
        assert_eq!(applied["runtime_activation"], "restart-required");
        let saved = fs::read(&f.config).unwrap();
        let after: Value = serde_json::from_slice(&saved).unwrap();
        for (key, v) in before.as_object().unwrap() {
            if key != "model" && key != "base_url" {
                assert_eq!(&after[key], v);
            }
        }
        assert_eq!(after["model"], patch()["model"]["value"]);
        assert_eq!(after["base_url"], patch()["base_url"]["value"]);
        let observed = finish(f.run(Operation::Status, &status_input));
        assert_eq!(observed["outcome"], "desired-file-observed");
        assert_eq!(observed["disk_readback_verified"], false);
        assert_eq!(
            finish(f.run(
                Operation::Apply,
                &json!({"intent":intent,"changes":patch()})
            ))["status"],
            "conflict"
        );
        let current = finish(inspect(Some(f.config.clone())));
        let noop=finish(f.run(Operation::Prepare,&json!({"model_settings_version":1,"operation":Uuid::new_v4(),"base_sha256":current["data"]["base_sha256"],"config_path_sha256":current["data"]["config_path_sha256"],"changes":patch()})));
        assert_eq!(noop["outcome"], "no-change");
        assert!(noop["data"].get("intent").is_none());
        assert_eq!(fs::read(&f.config).unwrap(), saved);
        let clear = json!({"model":{"action":"clear"},"base_url":{"action":"clear"}});
        let ci = f.prepare(clear.clone());
        assert_eq!(
            finish(f.run(Operation::Apply, &json!({"intent":ci,"changes":clear})))["outcome"],
            "verified"
        );
        let cleared: Value = serde_json::from_slice(&fs::read(&f.config).unwrap()).unwrap();
        assert!(cleared.get("model").is_none());
        assert!(cleared.get("base_url").is_none());
    }
    #[test]
    fn forged_patch_target_path_base_and_busy_preserve_file() {
        let f = Fixture::new();
        let intent = f.prepare(patch());
        let original = fs::read(&f.config).unwrap();
        let mut altered = patch();
        altered["model"]["value"] = json!("OTHER_PRIVATE");
        assert_eq!(
            finish(f.run(
                Operation::Apply,
                &json!({"intent":intent,"changes":altered})
            ))["status"],
            "invalid-input"
        );
        for field in [
            "target_sha256",
            "config_path_sha256",
            "base_sha256",
            "changes_sha256",
        ] {
            let mut bad = serde_json::to_value(&intent).unwrap();
            bad[field] = json!("0".repeat(64));
            assert_ne!(
                finish(f.run(Operation::Apply, &json!({"intent":bad,"changes":patch()})))["status"],
                "ok"
            );
            assert_eq!(fs::read(&f.config).unwrap(), original);
        }
        let owner = Store::open(f.paths.clone()).unwrap();
        assert_eq!(
            finish(f.run(
                Operation::Apply,
                &json!({"intent":intent,"changes":patch()})
            ))["status"],
            "busy"
        );
        assert_eq!(
            finish(f.run(Operation::Status, &json!({"intent":intent})))["status"],
            "ok"
        );
        drop(owner);
        let mut newer = original.clone();
        newer.push(b' ');
        fs::write(&f.config, &newer).unwrap();
        assert_eq!(
            finish(f.run(
                Operation::Apply,
                &json!({"intent":intent,"changes":patch()})
            ))["status"],
            "conflict"
        );
        assert_eq!(fs::read(&f.config).unwrap(), newer);
    }
    #[test]
    fn same_atomic_faults_and_readback_unknown_are_reconciled_without_patch() {
        for fault in [
            ConfigFault::AtomicCall,
            ConfigFault::AfterPublication,
            ConfigFault::ChangedReadback,
        ] {
            let f = Fixture::new();
            let intent = f.prepare(patch());
            let doc = Document::load(Some(f.config.clone())).unwrap();
            let changes = serde_json::from_value(patch()).unwrap();
            let report = apply_with(
                doc,
                intent.clone(),
                changes,
                |store, path, expected, bytes| {
                    store.replace_config_document_fault(path, expected, bytes, fault)
                },
            );
            let r = finish(report);
            assert_eq!(r["outcome"], "unknown");
            assert!(r["data"].is_null());
            let actual = fs::read(&f.config).unwrap();
            let r = finish(f.run(Operation::Status, &json!({"intent":intent})));
            assert_eq!(
                r["outcome"],
                if digest(&actual) == intent.target_sha256 {
                    "desired-file-observed"
                } else {
                    "unresolved"
                }
            );
            assert_eq!(fs::read(&f.config).unwrap(), actual);
        }
    }
    #[test]
    fn prefix_failure_holds_store_until_consuming_output_finishes() {
        let f = Fixture::new();
        let intent = f.prepare(patch());
        let report = f.run(
            Operation::Apply,
            &json!({"intent":intent,"changes":patch()}),
        );
        assert!(matches!(
            Store::open_existing(f.paths.clone()),
            Err(Refusal::Busy)
        ));
        struct Prefix {
            calls: usize,
            bytes: Vec<u8>,
            paths: StorePaths,
        }
        impl Write for Prefix {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                assert!(matches!(
                    Store::open_existing(self.paths.clone()),
                    Err(Refusal::Busy)
                ));
                self.calls += 1;
                if self.calls > 1 {
                    return Err(io::Error::other("stopped"));
                }
                self.bytes.extend_from_slice(&b[..17]);
                Ok(17)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut out = Prefix {
            calls: 0,
            bytes: vec![],
            paths: f.paths.clone(),
        };
        assert!(report.write_to(&mut out).is_err());
        assert_eq!(out.bytes.len(), 17);
        assert_eq!(out.calls, 2);
        drop(Store::open_existing(f.paths.clone()).unwrap());
    }
    #[test]
    fn strict_shapes_caps_duplicates_cross_kind_and_inspect_no_read() {
        struct NoRead;
        impl Read for NoRead {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                panic!("stdin acquired")
            }
        }
        assert_eq!(
            run_stdin(Operation::Inspect, None, &mut NoRead).status,
            Status::InvalidInput
        );
        for mut input in [
            br#"[]"#.as_slice(),
            br#"{"intent":[]}"#,
            br#"{"intent":{},"intent":{}}"#,
            br#"{"basic_version":1}"#,
            br#"{"changes":{"model":["clear"]}}"#,
        ] {
            assert_eq!(
                run_stdin(Operation::Apply, None, &mut input).status,
                Status::InvalidInput
            );
        }
        assert_eq!(
            run_stdin(
                Operation::Prepare,
                None,
                &mut vec![b' '; MAX_IO + 1].as_slice()
            )
            .status,
            Status::InvalidInput
        );
        for changes in [
            json!({}),
            json!({"debug_dump":{"action":"set","value":true}}),
            json!({"model":{"action":"set","value":null}}),
            json!({"model":{"action":"set","value":" \r\n "}}),
            json!({"model":{"action":"set","value":"α".repeat(129)}}),
        ] {
            if let Ok(changes) = serde_json::from_value::<Changes>(changes) {
                assert!(!valid_changes(&changes));
            }
        }
    }
    #[test]
    fn origins_null_clear_invalid_whole_document_and_target_cap() {
        let f = Fixture::new();
        let view = finish(inspect(Some(f.config.clone())));
        assert_eq!(
            view["data"]["current"]["model"],
            json!({"origin":"declared-null","present":false})
        );
        let cleared = json!({"model":{"action":"clear"}});
        let intent = f.prepare(cleared.clone());
        assert_eq!(
            finish(f.run(
                Operation::Apply,
                &json!({"intent":intent,"changes":cleared})
            ))["outcome"],
            "verified"
        );
        assert_eq!(
            finish(inspect(Some(f.config.clone())))["data"]["current"]["model"],
            json!({"origin":"default","present":false})
        );
        fs::write(&f.config, br#"{"model":"   "}"#).unwrap();
        assert_eq!(
            finish(inspect(Some(f.config.clone())))["status"],
            "invalid-input"
        );
        let mut value = json!({"paths":{"data":"","cache":"/synthetic/cache","credentials":"/synthetic/credentials"}});
        let overhead = serde_json::to_vec(&value).unwrap().len();
        value["paths"]["data"] = json!("x".repeat(MAX_RECORD - overhead));
        let bytes = serde_json::to_vec(&value).unwrap();
        let (config, raw) = checked_document(&bytes).unwrap();
        let doc = Document {
            path: PathBuf::from("/synthetic/config.json"),
            path_hash: "0".repeat(64),
            hash: digest(&bytes),
            bytes,
            config,
            raw,
        };
        let changes =
            serde_json::from_value(json!({"model":{"action":"set","value":"synthetic"}})).unwrap();
        assert!(matches!(target(&doc, &changes), Err(Status::InvalidInput)));
    }
}

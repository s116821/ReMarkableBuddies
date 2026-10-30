//! Saved memory declarations only; no profile construction or runtime activation.
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
#[serde(untagged)]
enum Field {
    Enabled { origin: &'static str, value: bool },
    Profile { origin: &'static str, present: bool },
}
type Fields = BTreeMap<&'static str, Field>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Setting {
    Enabled,
    Profile,
}
impl Setting {
    fn name(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Profile => "profile",
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
                Setting::Enabled => value.is_boolean(),
                Setting::Profile => value
                    .as_str()
                    .is_some_and(|s| Uuid::parse_str(s).is_ok_and(|id| !id.is_nil())),
            },
        })
}
fn patch_hash(changes: &Changes) -> String {
    let raw = serde_json::to_value(changes).expect("fixed changes serialize");
    let mut bytes = b"urn:remarkable-buddies:memory-settings-patch:v1:".to_vec();
    serde_json::to_writer(&mut bytes, &Ordered(&raw)).expect("fixed changes serialize");
    digest(&bytes)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepare {
    memory_settings_version: u32,
    operation: Uuid,
    base_sha256: String,
    config_path_sha256: String,
    changes: Changes,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    memory_settings_version: u32,
    operation: Uuid,
    base_sha256: String,
    target_sha256: String,
    config_path_sha256: String,
    changes_sha256: String,
}
impl Intent {
    fn validate(&self) -> std::result::Result<(), Status> {
        if self.memory_settings_version != 1 {
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
        ensure!(
            bytes.len() <= MAX_IO,
            "memory settings report exceeds bound"
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
            digest(format!("urn:remarkable-buddies:memory-settings-path:v1:{utf8}").as_bytes());
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
    let origin = |name: &str| match raw.get("memory").and_then(|m| m.get(name)) {
        None => "default",
        Some(Value::Null) => "declared-null",
        Some(_) => "declared",
    };
    BTreeMap::from([
        (
            "enabled",
            Field::Enabled {
                origin: origin("enabled"),
                value: config.memory.enabled,
            },
        ),
        (
            "profile",
            Field::Profile {
                origin: origin("profile"),
                present: config.memory.profile.is_some(),
            },
        ),
    ])
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
    let has_set = changes.values().any(|c| matches!(c, Change::Set { .. }));
    if raw.get("memory").is_none() && has_set {
        raw.as_object_mut()
            .ok_or(Status::InvalidInput)?
            .insert("memory".into(), serde_json::json!({}));
    }
    if let Some(memory) = raw.get_mut("memory") {
        let object = memory.as_object_mut().ok_or(Status::InvalidInput)?;
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
            if request.memory_settings_version != 1 {
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
                memory_settings_version: 1,
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
            let root =
                std::env::temp_dir().join(format!("buddy-memory-settings-{}", Uuid::new_v4()));
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
            let report = self.run(Operation::Prepare, &json!({"memory_settings_version":1,"operation":Uuid::new_v4(),"base_sha256":current["data"]["base_sha256"],"config_path_sha256":current["data"]["config_path_sha256"],"changes":changes}));
            let value = finish(report);
            assert_eq!(value["outcome"], "prepared");
            serde_json::from_value(value["data"]["intent"].clone()).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                assert_eq!(self.root.parent(), Some(std::env::temp_dir().as_path()));
                assert!(self
                    .root
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("buddy-memory-settings-"));
                let _ = fs::remove_dir_all(&self.root);
            }
        }
    }
    fn finish(report: Report) -> Value {
        let mut bytes = Vec::new();
        report.write_to(&mut bytes).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE_"));
        assert!(!String::from_utf8_lossy(&bytes).contains("private-url-sentinel"));
        assert!(!String::from_utf8_lossy(&bytes).contains("123E4567-E89B-12D3-A456-426614174000"));
        serde_json::from_slice(&bytes).unwrap()
    }

    const ID: &str = "123E4567-E89B-12D3-A456-426614174000";
    fn patch() -> Value {
        json!({"enabled":{"action":"set","value":true},"profile":{"action":"set","value":ID}})
    }
    fn prep(f: &Fixture, changes: Value) -> Value {
        let current = finish(inspect(Some(f.config.clone())));
        finish(f.run(Operation::Prepare,&json!({"memory_settings_version":1,"operation":Uuid::new_v4(),"base_sha256":current["data"]["base_sha256"],"config_path_sha256":current["data"]["config_path_sha256"],"changes":changes})))
    }
    fn memory(f: &Fixture, value: Option<Value>) {
        let mut raw: Value = serde_json::from_slice(&fs::read(&f.config).unwrap()).unwrap();
        match value {
            Some(v) => {
                raw["memory"] = v;
            }
            None => {
                raw.as_object_mut().unwrap().remove("memory");
            }
        }
        fs::write(&f.config, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
    }
    #[test]
    fn independent_patch_vector_and_escaped_reordered_input_match() {
        let raw = r#"{"profile":{"value":"\u003123E4567-E89B-12D3-A456-426614174000","action":"set"},"enabled":{"value":true,"action":"set"}}"#;
        let changes: Changes = serde_json::from_str(raw).unwrap();
        assert!(valid_changes(&changes));
        assert_eq!(
            patch_hash(&changes),
            "eb760e429bd11cdebf432a7dbb2f43588474e78112b042d244fd671d188dec0d"
        );
        let val = serde_json::to_value(&changes).unwrap();
        assert_eq!(serde_json::to_vec(&Ordered(&val)).unwrap().len(), 115);
    }
    #[test]
    fn absent_parent_clear_is_exact_noop_and_existing_last_child_keeps_empty_object() {
        let f = Fixture::new();
        memory(&f, None);
        let original = fs::read(&f.config).unwrap();
        assert_eq!(
            prep(
                &f,
                json!({"enabled":{"action":"clear"},"profile":{"action":"clear"}})
            )["outcome"],
            "no-change"
        );
        assert_eq!(fs::read(&f.config).unwrap(), original);
        let change = json!({"profile":{"action":"set","value":ID}});
        let intent = f.prepare(change.clone());
        let applied = finish(f.run(Operation::Apply, &json!({"intent":intent,"changes":change})));
        assert_eq!(applied["outcome"], "verified");
        assert_eq!(applied["runtime_activation"], "unverified");
        let raw: Value = serde_json::from_slice(&fs::read(&f.config).unwrap()).unwrap();
        assert_eq!(raw["memory"], json!({"profile":ID}));
        let change = json!({"profile":{"action":"clear"}});
        let i = f.prepare(change.clone());
        assert_eq!(
            finish(f.run(Operation::Apply, &json!({"intent":i,"changes":change})))["outcome"],
            "verified"
        );
        let raw: Value = serde_json::from_slice(&fs::read(&f.config).unwrap()).unwrap();
        assert_eq!(raw["memory"], json!({}));
        let bytes = fs::read(&f.config).unwrap();
        assert_eq!(
            prep(&f, json!({"profile":{"action":"clear"}}))["outcome"],
            "no-change"
        );
        assert_eq!(fs::read(&f.config).unwrap(), bytes);
    }
    #[test]
    fn null_origin_clear_and_exact_uuid_spelling_preserve_unrelated_values() {
        let cases = [
            ID.to_owned(),
            ID.to_ascii_lowercase(),
            ID.replace('-', ""),
            format!("urn:uuid:{ID}"),
            format!("{{{ID}}}"),
        ];
        for spelling in cases {
            let f = Fixture::new();
            memory(&f, Some(json!({"profile":null,"selection":null})));
            let before: Value = serde_json::from_slice(&fs::read(&f.config).unwrap()).unwrap();
            assert_eq!(
                finish(inspect(Some(f.config.clone())))["data"]["current"]["profile"],
                json!({"origin":"declared-null","present":false})
            );
            let ch = json!({"profile":{"action":"set","value":spelling},"enabled":{"action":"set","value":true}});
            let i = f.prepare(ch.clone());
            let report = finish(f.run(Operation::Apply, &json!({"intent":i,"changes":ch})));
            assert_eq!(report["outcome"], "verified");
            assert_eq!(report["runtime_activation"], "unverified");
            let after: Value = serde_json::from_slice(&fs::read(&f.config).unwrap()).unwrap();
            assert_eq!(after["memory"]["profile"], spelling);
            assert_eq!(after["memory"]["selection"], Value::Null);
            for (k, v) in before.as_object().unwrap() {
                if k != "memory" {
                    assert_eq!(&after[k], v);
                }
            }
            assert_eq!(prep(&f, patch_for(&spelling))["outcome"], "no-change");
        }
        let f = Fixture::new();
        memory(&f, Some(json!({"profile":null})));
        let ch = json!({"profile":{"action":"clear"}});
        let i = f.prepare(ch.clone());
        finish(f.run(Operation::Apply, &json!({"intent":i,"changes":ch})));
        assert_eq!(
            finish(inspect(Some(f.config.clone())))["data"]["current"]["profile"],
            json!({"origin":"default","present":false})
        );
    }
    fn patch_for(id: &str) -> Value {
        json!({"enabled":{"action":"set","value":true},"profile":{"action":"set","value":id}})
    }
    #[test]
    fn required_profile_selected_consistency_and_legacy_sync_constraints_refuse_without_repair() {
        let f = Fixture::new();
        let before = fs::read(&f.config).unwrap();
        assert_eq!(
            prep(&f, json!({"enabled":{"action":"set","value":true}}))["status"],
            "invalid-input"
        );
        assert_eq!(fs::read(&f.config).unwrap(), before);
        let identity = json!({"group":Uuid::new_v4(),"profile":ID,"binding_sha256":digest(b"fixture binding")});
        memory(
            &f,
            Some(json!({"enabled":false,"profile":ID,"selection":identity})),
        );
        let before = fs::read(&f.config).unwrap();
        assert_eq!(
            prep(
                &f,
                json!({"profile":{"action":"set","value":Uuid::new_v4()}})
            )["status"],
            "invalid-input"
        );
        assert_eq!(
            prep(&f, json!({"profile":{"action":"clear"}}))["status"],
            "invalid-input"
        );
        assert_eq!(fs::read(&f.config).unwrap(), before);
        let ch = json!({"enabled":{"action":"set","value":true}});
        let i = f.prepare(ch.clone());
        let r = finish(f.run(Operation::Apply, &json!({"intent":i,"changes":ch})));
        assert_eq!(r["outcome"], "verified");
        assert_eq!(r["runtime_activation"], "unverified");
        let after: Value = serde_json::from_slice(&fs::read(&f.config).unwrap()).unwrap();
        assert_eq!(after["memory"]["selection"], identity);
        memory(&f, Some(json!({"profile":ID})));
        let mut raw: Value = serde_json::from_slice(&fs::read(&f.config).unwrap()).unwrap();
        raw["sync"] = json!({"enabled":true,"namespaces":["subject-memory"]});
        fs::write(&f.config, serde_json::to_vec(&raw).unwrap()).unwrap();
        assert_eq!(
            prep(&f, json!({"enabled":{"action":"set","value":true}}))["status"],
            "invalid-input"
        );
        for invalid in [
            Value::Null,
            json!([]),
            json!({"enabled":null}),
            json!({"profile":Uuid::nil()}),
            json!({"enabled":true}),
        ] {
            memory(&f, Some(invalid));
            let old = fs::read(&f.config).unwrap();
            assert_eq!(
                finish(inspect(Some(f.config.clone())))["status"],
                "invalid-input"
            );
            let report=f.run(Operation::Prepare,&json!({"memory_settings_version":1,"operation":Uuid::new_v4(),"base_sha256":digest(&old),"config_path_sha256":"0".repeat(64),"changes":{"profile":{"action":"clear"},"enabled":{"action":"clear"}}}));
            assert_eq!(finish(report)["status"], "invalid-input");
            assert_eq!(fs::read(&f.config).unwrap(), old);
        }
    }
    #[test]
    fn original_patch_file_path_target_and_busy_refusals_preserve_bytes() {
        let f = Fixture::new();
        let intent = f.prepare(patch());
        let old = fs::read(&f.config).unwrap();
        let mut changed = patch();
        changed["profile"]["value"] = json!(Uuid::new_v4());
        assert_eq!(
            finish(f.run(
                Operation::Apply,
                &json!({"intent":intent,"changes":changed})
            ))["status"],
            "invalid-input"
        );
        for key in [
            "base_sha256",
            "target_sha256",
            "config_path_sha256",
            "changes_sha256",
        ] {
            let mut bad = serde_json::to_value(&intent).unwrap();
            bad[key] = json!("0".repeat(64));
            assert_ne!(
                finish(f.run(Operation::Apply, &json!({"intent":bad,"changes":patch()})))["status"],
                "ok"
            );
            assert_eq!(fs::read(&f.config).unwrap(), old);
        }
        let owner = Store::open_existing(f.paths.clone()).unwrap();
        assert_eq!(
            finish(f.run(
                Operation::Apply,
                &json!({"intent":intent,"changes":patch()})
            ))["status"],
            "busy"
        );
        let r = finish(f.run(Operation::Status, &json!({"intent":intent})));
        assert_eq!(r["outcome"], "unresolved");
        assert!(r["data"].is_null());
        drop(owner);
    }
    #[test]
    fn actual_atomic_faults_stay_unknown_and_status_needs_no_patch() {
        for fault in [
            ConfigFault::AtomicCall,
            ConfigFault::AfterPublication,
            ConfigFault::ChangedReadback,
        ] {
            let f = Fixture::new();
            let intent = f.prepare(patch());
            let doc = Document::load(Some(f.config.clone())).unwrap();
            let changes = serde_json::from_value(patch()).unwrap();
            let report = apply_with(doc, intent.clone(), changes, |store, path, hash, bytes| {
                store.replace_config_document_fault(path, hash, bytes, fault)
            });
            let r = finish(report);
            assert_eq!(r["outcome"], "unknown");
            assert_eq!(r["runtime_activation"], "unverified");
            assert!(r["data"].is_null());
            let bytes = fs::read(&f.config).unwrap();
            let r = finish(f.run(Operation::Status, &json!({"intent":intent})));
            assert_eq!(
                r["outcome"],
                if digest(&bytes) == intent.target_sha256 {
                    "desired-file-observed"
                } else {
                    "unresolved"
                }
            );
            assert_eq!(fs::read(&f.config).unwrap(), bytes);
        }
    }
    #[test]
    fn consuming_prefix_holds_real_store_until_failure_and_emits_no_second_report() {
        let f = Fixture::new();
        let i = f.prepare(patch());
        let report = f.run(Operation::Apply, &json!({"intent":i,"changes":patch()}));
        assert_eq!(report.status, Status::Ok);
        struct Prefix {
            paths: StorePaths,
            calls: usize,
            bytes: Vec<u8>,
        }
        impl Write for Prefix {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                assert!(matches!(
                    Store::open_existing(self.paths.clone()),
                    Err(Refusal::Busy)
                ));
                self.calls += 1;
                if self.calls > 1 {
                    return Err(io::Error::other("prefix stopped"));
                }
                self.bytes.extend_from_slice(&b[..17]);
                Ok(17)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut out = Prefix {
            paths: f.paths.clone(),
            calls: 0,
            bytes: vec![],
        };
        assert!(report.write_to(&mut out).is_err());
        assert_eq!(out.calls, 2);
        assert_eq!(out.bytes.len(), 17);
        drop(Store::open_existing(f.paths.clone()).unwrap());
    }
    #[test]
    fn strict_shapes_caps_uuid_types_no_inspect_read_and_target_bound() {
        struct NoRead;
        impl Read for NoRead {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                panic!("inspect acquired stdin")
            }
        }
        assert_eq!(
            run_stdin(Operation::Inspect, None, &mut NoRead).status,
            Status::InvalidInput
        );
        for mut raw in [
            br#"[]"#.as_slice(),
            br#"{"intent":[]}"#,
            br#"{"changes":{"profile":["clear"]}}"#,
            br#"{"memory_settings_version":1,"memory_settings_version":1}"#,
            br#"{"model_settings_version":1}"#,
        ] {
            assert_eq!(
                run_stdin(Operation::Apply, None, &mut raw).status,
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
        for value in [
            json!({}),
            json!({"profile":{"action":"set","value":null}}),
            json!({"enabled":{"action":"set","value":null}}),
            json!({"profile":{"action":"set","value":Uuid::nil()}}),
            json!({"profile":{"action":"set","value":format!(" {ID}")}}),
            json!({"profile":{"action":"set","value":"PRIVATE_UUID_SENTINEL"}}),
            json!({"profile":{"action":"set","value":vec![0u8;16]}}),
        ] {
            if let Ok(c) = serde_json::from_value::<Changes>(value) {
                assert!(!valid_changes(&c));
            }
        }
        let mut raw = json!({"paths":{"data":"","cache":"/synthetic/cache","credentials":"/synthetic/credentials"}});
        let len = serde_json::to_vec(&raw).unwrap().len();
        raw["paths"]["data"] = json!("x".repeat(MAX_RECORD - len));
        let bytes = serde_json::to_vec(&raw).unwrap();
        let (config, raw) = checked_document(&bytes).unwrap();
        let doc = Document {
            path: PathBuf::from("/synthetic/config.json"),
            path_hash: "0".repeat(64),
            hash: digest(&bytes),
            bytes,
            config,
            raw,
        };
        assert!(matches!(
            target(&doc, &serde_json::from_value(patch()).unwrap()),
            Err(Status::InvalidInput)
        ));
    }
}

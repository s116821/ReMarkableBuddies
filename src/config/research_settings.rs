//! Saved research policy only; no acquisition or runtime activation.
use super::{
    basic::{checked_document, Custody, Ordered, Status},
    inspection, Config,
};
use crate::conversation::receiving::memory::research::ResearchPolicy;
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

/// Called only for a present field, including explicit null; missing stays None.
pub(super) fn present_policy<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Option<ResearchPolicy>>, D::Error> {
    Option::<ResearchPolicy>::deserialize(deserializer).map(Some)
}

/// Captured saved policy/default, not disk or runtime authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResearchPolicySnapshot {
    policy: ResearchPolicy,
}
impl ResearchPolicySnapshot {
    pub(super) fn capture(config: &Config) -> Result<Self> {
        config.validate_values()?;
        Ok(Self {
            policy: config.research_policy.flatten().unwrap_or_default(),
        })
    }
    pub fn policy(&self) -> ResearchPolicy {
        self.policy
    }
}

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
    value: ResearchPolicy,
}
type Fields = BTreeMap<&'static str, Field>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Setting {
    ResearchPolicy,
}
impl Setting {
    fn name(self) -> &'static str {
        "research_policy"
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
        && changes.values().all(|change| match change {
            Change::Clear {} => true,
            Change::Set { value } => matches!(value.as_str(), Some("required" | "disabled")),
        })
}
fn patch_hash(changes: &Changes) -> String {
    let raw = serde_json::to_value(changes).expect("fixed changes serialize");
    let mut bytes = b"urn:remarkable-buddies:research-settings-patch:v1:".to_vec();
    serde_json::to_writer(&mut bytes, &Ordered(&raw)).expect("fixed changes serialize");
    digest(&bytes)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepare {
    research_settings_version: u32,
    operation: Uuid,
    base_sha256: String,
    config_path_sha256: String,
    changes: Changes,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    research_settings_version: u32,
    operation: Uuid,
    base_sha256: String,
    target_sha256: String,
    config_path_sha256: String,
    changes_sha256: String,
}
impl Intent {
    fn validate(&self) -> std::result::Result<(), Status> {
        if self.research_settings_version != 1 {
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
            "research settings report exceeds bound"
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
            digest(format!("urn:remarkable-buddies:research-settings-path:v1:{utf8}").as_bytes());
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
    let origin = match raw.get("research_policy") {
        None => "default",
        Some(Value::Null) => "declared-null",
        Some(_) => "declared",
    };
    BTreeMap::from([(
        "research_policy",
        Field {
            origin,
            value: config.research_policy.flatten().unwrap_or_default(),
        },
    )])
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
            if request.research_settings_version != 1 {
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
                research_settings_version: 1,
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
#[path = "research_settings/tests.rs"]
mod tests;

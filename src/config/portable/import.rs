//! Private, conditional present-declaration import. No runtime activation.
use super::{inspection, SETTINGS};
use crate::{
    config::{
        basic::{self, Custody},
        Config,
    },
    restore_maintenance::overlap,
    storage::{
        digest, files, maintenance::Refusal, sync::mode::SyncMode, valid_digest, ConfigPublication,
        Store, Uuid, MAX_RECORD,
    },
};
use anyhow::{anyhow, ensure, Result};
pub use basic::{Operation, Status};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::Arc,
};

pub const MAX_REQUEST: usize = 256 * 1024;
pub const MAX_REPORT: usize = 16 * 1024;

// No Debug: even digests belong to the private maintenance transport.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    portable_import_version: u32,
    operation: Uuid,
    base_sha256: String,
    target_sha256: String,
    config_path_sha256: String,
    artifact_sha256: String,
}
impl Intent {
    fn validate(&self) -> std::result::Result<(), Status> {
        version(self.portable_import_version)?;
        if self.operation.is_nil()
            || self.base_sha256 == self.target_sha256
            || [
                &self.base_sha256,
                &self.target_sha256,
                &self.config_path_sha256,
                &self.artifact_sha256,
            ]
            .iter()
            .any(|v| !valid_digest(v))
        {
            return Err(Status::InvalidInput);
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepare {
    portable_import_version: u32,
    operation: Uuid,
    base_sha256: String,
    config_path_sha256: String,
    artifact: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Apply {
    intent: Intent,
    artifact: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Observe {
    intent: Intent,
}

#[derive(Serialize)]
pub struct Report {
    report_version: u32,
    operation: Operation,
    status: Status,
    outcome: &'static str,
    disk_readback_verified: bool,
    runtime_activation: &'static str,
    data: Option<Value>,
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
                "read-only"
            } else {
                "not-dispatched"
            },
            disk_readback_verified: false,
            runtime_activation: "unverified",
            data: None,
            _custody: None,
        }
    }
    fn success(operation: Operation, outcome: &'static str, data: Value) -> Self {
        let mut report = Self::refusal(operation, Status::Ok);
        report.outcome = outcome;
        report.data = Some(data);
        report
    }
    /// Consume original custody through one frame, without flush or replacement.
    pub fn write_to(self, writer: &mut impl Write) -> Result<()> {
        let mut bytes = serde_json::to_vec(&self).map_err(|_| anyhow!("output-failure"))?;
        bytes.push(b'\n');
        ensure!(bytes.len() <= MAX_REPORT, "output-failure");
        writer
            .write_all(&bytes)
            .map_err(|_| anyhow!("output-failure"))
    }
}
pub fn invalid_input(operation: Operation) -> Report {
    Report::refusal(operation, Status::InvalidInput)
}
pub fn unsupported_input(operation: Operation) -> Report {
    Report::refusal(operation, Status::Unsupported)
}
fn version(value: u32) -> std::result::Result<(), Status> {
    if value == 1 {
        Ok(())
    } else {
        Err(Status::Unsupported)
    }
}
fn parse<T: DeserializeOwned>(bytes: &[u8]) -> std::result::Result<T, Status> {
    let raw = inspection::strict_json(bytes).map_err(|_| Status::InvalidInput)?;
    serde_json::from_value(raw).map_err(|_| Status::InvalidInput)
}
fn artifact(value: &Value) -> std::result::Result<String, Status> {
    let bytes = serde_json::to_vec(value).map_err(|_| Status::InvalidInput)?;
    if bytes.len() > super::MAX_IO {
        return Err(Status::InvalidInput);
    }
    super::validate(&bytes).map_err(|s| match s {
        inspection::Status::Unsupported => Status::Unsupported,
        _ => Status::InvalidInput,
    })?;
    Ok(digest(&bytes))
}
struct Document {
    path: PathBuf,
    path_hash: String,
    hash: String,
    config: Config,
    raw: Value,
}
impl Document {
    fn load(path: Option<PathBuf>) -> std::result::Result<Self, Status> {
        let path = path
            .filter(|p| p.is_absolute())
            .ok_or(Status::Unavailable)?;
        let text = path.to_str().ok_or(Status::Unavailable)?;
        let path_hash = digest(
            [
                b"remarkable-buddies-portable-import-path-v1\0".as_slice(),
                text.as_bytes(),
            ]
            .concat()
            .as_slice(),
        );
        let bytes = files::read(&path, MAX_RECORD as u64).map_err(|_| Status::Unavailable)?;
        let (config, raw) = basic::checked_document(&bytes)?;
        Ok(Self {
            path,
            path_hash,
            hash: digest(&bytes),
            config,
            raw,
        })
    }
    /// Canonical checks are separate from the exact supplied-path digest.
    fn identities(&self) -> std::result::Result<Vec<PathBuf>, Status> {
        self.config.validate().map_err(|_| Status::Unsupported)?;
        let mut paths = Vec::new();
        for path in [
            &self.path,
            &self.config.paths.data,
            &self.config.paths.cache,
            &self.config.paths.credentials,
        ] {
            files::safe_path(path).map_err(|_| Status::Unsupported)?;
            paths.push(path.canonicalize().map_err(|_| Status::Unavailable)?);
        }
        for (i, path) in paths.iter().enumerate() {
            for other in &paths[i + 1..] {
                if overlap(path, other).map_err(|_| Status::Unsupported)? {
                    return Err(Status::Unsupported);
                }
            }
        }
        Ok(paths)
    }
}
fn origins(raw: &Value) -> Value {
    SETTINGS
        .into_iter()
        .map(|name| {
            (
                name.to_owned(),
                json!(match raw.get(name) {
                    None => "absent",
                    Some(Value::Null) => "declared-null",
                    Some(_) => "declared",
                }),
            )
        })
        .collect()
}
struct Target {
    bytes: Vec<u8>,
    raw: Value,
    changed: Vec<&'static str>,
}
fn target(doc: &Document, artifact: &Value) -> std::result::Result<Target, Status> {
    let mut raw = doc.raw.clone();
    super::merge_settings(
        raw.as_object_mut().ok_or(Status::InvalidInput)?,
        artifact["settings"]
            .as_object()
            .ok_or(Status::InvalidInput)?,
    );
    let mut changed: Vec<_> = SETTINGS
        .into_iter()
        .filter(|name| raw.get(*name) != doc.raw.get(*name))
        .collect();
    changed.sort_unstable();
    let bytes = serde_json::to_vec(&raw).map_err(|_| Status::InvalidInput)?;
    if bytes.len() > MAX_RECORD {
        return Err(Status::InvalidInput);
    }
    basic::checked_document(&bytes)?;
    Ok(Target {
        bytes,
        raw,
        changed,
    })
}
pub fn inspect(path: Option<PathBuf>) -> Report {
    match Document::load(path) {
        Ok(doc) => Report::success(
            Operation::Inspect,
            "read-only",
            json!({"base_sha256":doc.hash,
            "config_path_sha256":doc.path_hash,"current":origins(&doc.raw)}),
        ),
        Err(s) => Report::refusal(Operation::Inspect, s),
    }
}
pub fn run_stdin(operation: Operation, path: Option<PathBuf>, input: &mut impl Read) -> Report {
    if operation == Operation::Inspect {
        return invalid_input(operation);
    }
    let mut bytes = Vec::new();
    if input
        .take((MAX_REQUEST + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > MAX_REQUEST
    {
        return invalid_input(operation);
    }
    match dispatch(operation, path, &bytes) {
        Ok(report) => report,
        Err(s) => Report::refusal(operation, s),
    }
}
fn dispatch(
    operation: Operation,
    path: Option<PathBuf>,
    bytes: &[u8],
) -> std::result::Result<Report, Status> {
    match operation {
        Operation::Prepare => {
            let req: Prepare = parse(bytes)?;
            version(req.portable_import_version)?;
            if req.operation.is_nil()
                || !valid_digest(&req.base_sha256)
                || !valid_digest(&req.config_path_sha256)
            {
                return Err(Status::InvalidInput);
            }
            let artifact_hash = artifact(&req.artifact)?;
            let doc = Document::load(path)?;
            if doc.hash != req.base_sha256 || doc.path_hash != req.config_path_sha256 {
                return Err(Status::Conflict);
            }
            let next = target(&doc, &req.artifact)?;
            if next.changed.is_empty() {
                return Ok(Report::success(
                    operation,
                    "no-change",
                    json!({"base_sha256":doc.hash,
                    "artifact_sha256":artifact_hash,"changed_settings":[]}),
                ));
            }
            let intent = Intent {
                portable_import_version: 1,
                operation: req.operation,
                base_sha256: doc.hash.clone(),
                target_sha256: digest(&next.bytes),
                config_path_sha256: doc.path_hash,
                artifact_sha256: artifact_hash,
            };
            Ok(Report::success(
                operation,
                "prepared",
                json!({"intent":intent,"changed_settings":next.changed,
                "before":origins(&doc.raw),"after":origins(&next.raw)}),
            ))
        }
        Operation::Apply => {
            let req: Apply = parse(bytes)?;
            req.intent.validate()?;
            if artifact(&req.artifact)? != req.intent.artifact_sha256 {
                return Err(Status::Conflict);
            }
            let doc = Document::load(path)?;
            if doc.path_hash != req.intent.config_path_sha256 || doc.hash != req.intent.base_sha256
            {
                return Err(Status::Conflict);
            }
            let next = target(&doc, &req.artifact)?;
            if next.changed.is_empty() || digest(&next.bytes) != req.intent.target_sha256 {
                return Err(Status::Conflict);
            }
            apply_with(doc, req.intent, next, Store::replace_config_document)
        }
        Operation::Status => {
            let req: Observe = parse(bytes)?;
            req.intent.validate()?;
            let doc = Document::load(path)?;
            if doc.path_hash != req.intent.config_path_sha256 {
                return Err(Status::Conflict);
            }
            Ok(Report::success(
                operation,
                if doc.hash == req.intent.target_sha256 {
                    "desired-file-observed"
                } else {
                    "unresolved"
                },
                json!({"observed_sha256":doc.hash}),
            ))
        }
        Operation::Inspect => Err(Status::InvalidInput),
    }
}
fn apply_with(
    doc: Document,
    intent: Intent,
    next: Target,
    publish: impl FnOnce(&Store, &std::path::Path, &str, &[u8]) -> ConfigPublication,
) -> std::result::Result<Report, Status> {
    let identities = doc.identities()?;
    let store = Arc::new(
        Store::open_existing_for_portable_preferences(doc.config.paths.clone()).map_err(
            |r| match r {
                Refusal::Busy => Status::Busy,
                Refusal::Unavailable => Status::Unavailable,
                Refusal::Unsupported => Status::Unsupported,
            },
        )?,
    );
    // Legacy1 needs this explicit restriction too; Custody also supports other callers' explicit modes.
    let mode = store
        .sync_mode_snapshot()
        .map_err(|_| Status::Unsupported)?;
    if !matches!(mode.mode, None | Some(SyncMode::LocalOnly)) {
        return Err(Status::Unsupported);
    }
    let custody = Custody::acquire(store.clone()).map_err(|_| Status::Unsupported)?;
    let result = (|| {
        if doc.identities()? != identities {
            return Err(Status::Conflict);
        }
        let current = files::read(&doc.path, MAX_RECORD as u64).map_err(|_| Status::Unavailable)?;
        if digest(&current) != intent.base_sha256 {
            return Err(Status::Conflict);
        }
        Ok(publish(&store, &doc.path, &intent.base_sha256, &next.bytes))
    })();
    let mut report = match result {
        Ok(ConfigPublication::Verified(hash)) if hash == intent.target_sha256 => {
            let mut r = Report::success(
                Operation::Apply,
                "verified",
                json!({"target_sha256":hash,"changed_settings":next.changed}),
            );
            r.disk_readback_verified = true;
            r
        }
        Ok(ConfigPublication::Conflict) => Report::refusal(Operation::Apply, Status::Conflict),
        Ok(ConfigPublication::Refused) => Report::refusal(Operation::Apply, Status::Failed),
        Ok(ConfigPublication::Unknown | ConfigPublication::Verified(_)) => {
            let mut r = Report::refusal(Operation::Apply, Status::Failed);
            r.outcome = "unknown";
            r
        }
        Err(s) => Report::refusal(Operation::Apply, s),
    };
    report._custody = Some(custody);
    Ok(report)
}

#[cfg(test)]
mod tests;

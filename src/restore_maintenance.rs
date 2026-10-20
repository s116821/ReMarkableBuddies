//! Explicit ordinary restore. No initialization, service, provider or native path.
use crate::{
    config::{inspection, Config},
    storage::{
        digest, files,
        maintenance::Refusal,
        ordinary_restore::{Conditions, Plan, Preview, Publication, Refused, Summary},
        sync::mode::{OrdinaryModeLease, SyncMode, SyncModeToken},
        valid_digest, Store, Uuid, MAX_RECORD,
    },
};
use anyhow::{ensure, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

pub const MAX_IO: usize = 16_384;
#[derive(Clone, Copy, Debug, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    Prepare,
    Apply,
    Status,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Ok,
    InvalidInput,
    Unavailable,
    Unsupported,
    Busy,
    Conflict,
    Failed,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Outcome {
    Prepared,
    Restored,
    DesiredGenerationObserved,
    Unresolved,
    NotDispatched,
    NotActivated,
    Unknown,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepare {
    restore_version: u32,
    operation: Uuid,
    source: PathBuf,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    restore_version: u32,
    operation: Uuid,
    config_path_sha256: String,
    config_sha256: String,
    source_path_sha256: String,
    archive_sha256: String,
    actor: Uuid,
    mode_sha256: String,
    base_generation: Uuid,
    base_history_sha256: String,
    target_generation: Uuid,
    target_history_sha256: String,
    staged_configuration_sha256: Option<String>,
    complete_media: bool,
}
impl Intent {
    fn validate(&self) -> std::result::Result<(), Status> {
        if self.restore_version != 1 {
            return Err(Status::Unsupported);
        }
        if [
            self.operation,
            self.actor,
            self.base_generation,
            self.target_generation,
        ]
        .iter()
        .any(Uuid::is_nil)
            || self.base_generation == self.target_generation
            || [
                &self.config_path_sha256,
                &self.config_sha256,
                &self.source_path_sha256,
                &self.archive_sha256,
                &self.mode_sha256,
                &self.base_history_sha256,
                &self.target_history_sha256,
            ]
            .iter()
            .any(|h| !valid_digest(h))
            || self
                .staged_configuration_sha256
                .as_ref()
                .is_some_and(|h| !valid_digest(h))
        {
            return Err(Status::InvalidInput);
        }
        Ok(())
    }
    fn conditions(&self) -> Conditions {
        Conditions {
            archive_sha256: self.archive_sha256.clone(),
            base_generation: self.base_generation,
            base_history_sha256: self.base_history_sha256.clone(),
            target_generation: self.target_generation,
            target_history_sha256: self.target_history_sha256.clone(),
            staged_configuration_sha256: self.staged_configuration_sha256.clone(),
            complete_media: self.complete_media,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IntentRequest {
    source: PathBuf,
    intent: Intent,
}
#[derive(Serialize)]
#[serde(untagged)]
enum Data {
    Prepared {
        preview: Preview,
        intent: Box<Intent>,
    },
    Observed {
        inspection: Summary,
        configuration_applied: bool,
    },
}
struct Custody {
    store: Arc<Store>,
    _mode: OrdinaryModeLease,
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
            outcome: Outcome::NotDispatched,
            disk_readback_verified: false,
            runtime_activation: "unverified",
            data: None,
            _custody: None,
        }
    }
    pub fn write_to(self, writer: &mut impl Write) -> Result<()> {
        let mut bytes = serde_json::to_vec(&self)?;
        bytes.push(b'\n');
        ensure!(bytes.len() <= MAX_IO, "restore report exceeds bound");
        writer.write_all(&bytes)?;
        Ok(())
    }
}
struct Document {
    path: PathBuf,
    path_hash: String,
    hash: String,
    config: Config,
}
fn lexical(path: &Path) -> std::result::Result<&str, Status> {
    let s = path.to_str().ok_or(Status::InvalidInput)?;
    if !path.is_absolute()
        || path.parent().is_none()
        || path.file_name().is_none()
        || s.len() > 4096
        || s.is_empty()
        || path.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(Status::InvalidInput);
    }
    Ok(s)
}
fn path_hash(kind: &str, path: &Path) -> std::result::Result<String, Status> {
    Ok(digest(
        format!(
            "urn:remarkable-buddies:ordinary-restore-{kind}-path:v1:{}",
            lexical(path)?
        )
        .as_bytes(),
    ))
}
#[cfg(not(windows))]
fn component_equal(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> std::result::Result<bool, Status> {
    Ok(a == b)
}
#[cfg(windows)]
fn component_equal(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> std::result::Result<bool, Status> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        #[link_name = "CompareStringOrdinal"]
        fn compare_string_ordinal(
            a: *const u16,
            a_len: i32,
            b: *const u16,
            b_len: i32,
            ignore_case: i32,
        ) -> i32;
    }
    let a: Vec<_> = a.encode_wide().collect();
    let b: Vec<_> = b.encode_wide().collect();
    let a_len = i32::try_from(a.len()).map_err(|_| Status::InvalidInput)?;
    let b_len = i32::try_from(b.len()).map_err(|_| Status::InvalidInput)?;
    // SAFETY: both live UTF16 buffers and their checked explicit lengths cover
    // the entire call. Windows ordinal case comparison is CPU-only: no source
    // lookup, locale-dependent folding or lossy path conversion is performed.
    let result = unsafe { compare_string_ordinal(a.as_ptr(), a_len, b.as_ptr(), b_len, 1) };
    match result {
        2 => Ok(true),
        1 | 3 => Ok(false),
        _ => Err(Status::InvalidInput),
    }
}
fn path_prefix(path: &Path, parent: &Path) -> std::result::Result<bool, Status> {
    let mut parts = path.components();
    for expected in parent.components() {
        let Some(actual) = parts.next() else {
            return Ok(false);
        };
        if !component_equal(actual.as_os_str(), expected.as_os_str())? {
            return Ok(false);
        }
    }
    Ok(true)
}
fn overlap(a: &Path, b: &Path) -> std::result::Result<bool, Status> {
    Ok(path_prefix(a, b)? || path_prefix(b, a)?)
}
impl Document {
    fn load(path: Option<PathBuf>) -> std::result::Result<Self, Status> {
        let path = path.ok_or(Status::Unavailable)?;
        let path_hash = path_hash("config", &path)?;
        let bytes = files::read(&path, MAX_RECORD as u64).map_err(|_| Status::Unavailable)?;
        let (config, _) = inspection::document(&bytes).map_err(|s| match s {
            inspection::Status::Unsupported => Status::Unsupported,
            _ => Status::InvalidInput,
        })?;
        if config.sync.enabled {
            return Err(Status::Unsupported);
        }
        Ok(Self {
            path,
            path_hash,
            hash: digest(&bytes),
            config,
        })
    }
    fn original(&self) -> Result<()> {
        let bytes = files::read(&self.path, MAX_RECORD as u64).map_err(|_| Refused::Unavailable)?;
        if digest(&bytes) != self.hash {
            return Err(Refused::Conflict.into());
        }
        Ok(())
    }
    fn source(&self, path: &Path, probe: bool) -> std::result::Result<String, Status> {
        let hash = path_hash("source", path)?;
        for root in [
            &self.config.paths.data,
            &self.config.paths.cache,
            &self.config.paths.credentials,
        ] {
            if overlap(path, root)? {
                return Err(Status::InvalidInput);
            }
        }
        if probe {
            files::safe_path(path).map_err(|_| Status::InvalidInput)?;
            let source = std::fs::canonicalize(path).map_err(|_| Status::Unavailable)?;
            for root in [
                &self.config.paths.data,
                &self.config.paths.cache,
                &self.config.paths.credentials,
            ] {
                files::safe_path(root).map_err(|_| Status::InvalidInput)?;
                let owned = std::fs::canonicalize(root).map_err(|_| Status::Unavailable)?;
                if overlap(&source, &owned)? {
                    return Err(Status::InvalidInput);
                }
            }
        }
        Ok(hash)
    }
}
#[derive(Deserialize)]
struct Token {
    actor: Uuid,
    sha256: String,
}
fn acquire(
    document: &Document,
    expected: Option<&Intent>,
) -> std::result::Result<(Custody, Token), Status> {
    let store = Arc::new(Store::open_existing(document.config.paths.clone()).map_err(
        |r| match r {
            Refusal::Unavailable => Status::Unavailable,
            Refusal::Unsupported => Status::Unsupported,
            Refusal::Busy => Status::Busy,
        },
    )?);
    let current = store
        .sync_mode_snapshot()
        .map_err(|_| Status::Unsupported)?;
    if !matches!(current.mode, None | Some(SyncMode::LocalOnly)) {
        return Err(Status::Unsupported);
    }
    let token: Token =
        serde_json::from_value(serde_json::to_value(&current.token).map_err(|_| Status::Failed)?)
            .map_err(|_| Status::Failed)?;
    let mode_token = match expected {
        Some(i) => {
            if i.actor != store.actor_id || i.actor != token.actor || i.mode_sha256 != token.sha256
            {
                return Err(Status::Conflict);
            }
            SyncModeToken::from_maintenance_request(i.actor, i.mode_sha256.clone())
                .map_err(|_| Status::InvalidInput)?
        }
        None => current.token,
    };
    let mode = store
        .ordinary_mode_lease(&mode_token)
        .map_err(|_| Status::Conflict)?;
    Ok((Custody { store, _mode: mode }, token))
}
fn parse<T: DeserializeOwned>(bytes: &[u8]) -> std::result::Result<T, Status> {
    let raw = inspection::strict_json(bytes).map_err(|_| Status::InvalidInput)?;
    if !raw.is_object() || raw.get("intent").is_some_and(|v| !v.is_object()) {
        return Err(Status::InvalidInput);
    }
    if raw
        .get("intent")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|i| !i.contains_key("staged_configuration_sha256"))
    {
        return Err(Status::InvalidInput);
    }
    serde_json::from_slice(bytes).map_err(|_| Status::InvalidInput)
}
fn error_status(error: &anyhow::Error) -> Status {
    match error.downcast_ref::<Refused>() {
        Some(Refused::InvalidInput) => Status::InvalidInput,
        Some(Refused::Unsupported) => Status::Unsupported,
        Some(Refused::Unavailable) => Status::Unavailable,
        Some(Refused::Conflict) => Status::Conflict,
        None => Status::Failed,
    }
}
fn prepare(document: &Document, bytes: &[u8]) -> std::result::Result<Report, Status> {
    let request: Prepare = parse(bytes)?;
    if request.restore_version != 1 {
        return Err(Status::Unsupported);
    }
    if request.operation.is_nil() {
        return Err(Status::InvalidInput);
    }
    let source_hash = document.source(&request.source, true)?;
    let (custody, token) = acquire(document, None)?;
    let plan = custody
        .store
        .prepare_ordinary_restore(&request.source, || document.original());
    let Plan {
        conditions: c,
        preview,
    } = match plan {
        Ok(plan) => plan,
        Err(e) => {
            let mut report = Report::refusal(Operation::Prepare, error_status(&e));
            report._custody = Some(custody);
            return Ok(report);
        }
    };
    let intent = Intent {
        restore_version: 1,
        operation: request.operation,
        config_path_sha256: document.path_hash.clone(),
        config_sha256: document.hash.clone(),
        source_path_sha256: source_hash,
        archive_sha256: c.archive_sha256,
        actor: token.actor,
        mode_sha256: token.sha256,
        base_generation: c.base_generation,
        base_history_sha256: c.base_history_sha256,
        target_generation: c.target_generation,
        target_history_sha256: c.target_history_sha256,
        staged_configuration_sha256: c.staged_configuration_sha256,
        complete_media: c.complete_media,
    };
    let mut report = Report::refusal(Operation::Prepare, Status::Ok);
    report.outcome = Outcome::Prepared;
    report.data = Some(Data::Prepared {
        preview,
        intent: Box::new(intent),
    });
    report._custody = Some(custody);
    Ok(report)
}
fn intent_operation(
    operation: Operation,
    document: &Document,
    bytes: &[u8],
) -> std::result::Result<Report, Status> {
    intent_operation_with(
        operation,
        document,
        bytes,
        |store, source, conditions, document| {
            store.apply_ordinary_restore(source, conditions, || document.original())
        },
    )
}
fn intent_operation_with(
    operation: Operation,
    document: &Document,
    bytes: &[u8],
    apply: impl FnOnce(&Store, &Path, &Conditions, &Document) -> Result<Publication>,
) -> std::result::Result<Report, Status> {
    let request: IntentRequest = parse(bytes)?;
    let i = &request.intent;
    i.validate()?;
    let source_hash = document.source(&request.source, matches!(operation, Operation::Apply))?;
    if i.config_path_sha256 != document.path_hash
        || i.config_sha256 != document.hash
        || i.source_path_sha256 != source_hash
    {
        return Err(Status::Conflict);
    }
    let (custody, _) = acquire(document, Some(i))?;
    let conditions = i.conditions();
    let mut report = Report::refusal(operation, Status::Ok);
    match operation {
        Operation::Apply => {
            let publication = apply(&custody.store, &request.source, &conditions, document);
            match publication {
                Err(e) => report.status = error_status(&e),
                Ok(Publication::Verified(inspection)) => {
                    report.outcome = Outcome::Restored;
                    report.disk_readback_verified = true;
                    report.data = Some(Data::Observed {
                        inspection,
                        configuration_applied: false,
                    });
                }
                Ok(Publication::NotActivated) => {
                    report.status = Status::Failed;
                    report.outcome = Outcome::NotActivated;
                }
                Ok(Publication::Unknown) => {
                    report.status = Status::Failed;
                    report.outcome = Outcome::Unknown;
                }
            }
        }
        Operation::Status => {
            match custody
                .store
                .observe_ordinary_restore(&conditions, || document.original())
            {
                Err(e) => report.status = error_status(&e),
                Ok(Some(inspection)) => {
                    report.outcome = Outcome::DesiredGenerationObserved;
                    report.data = Some(Data::Observed {
                        inspection,
                        configuration_applied: false,
                    });
                }
                Ok(None) => report.outcome = Outcome::Unresolved,
            }
        }
        Operation::Prepare => unreachable!(),
    }
    report._custody = Some(custody);
    Ok(report)
}
pub fn run(
    operation: Operation,
    input: &Path,
    config: Option<PathBuf>,
    reader: &mut impl Read,
) -> Report {
    let execute = || -> std::result::Result<Report, Status> {
        if input != Path::new("-") {
            return Err(Status::InvalidInput);
        }
        let mut bytes = Vec::new();
        reader
            .take((MAX_IO + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| Status::InvalidInput)?;
        if bytes.len() > MAX_IO {
            return Err(Status::InvalidInput);
        }
        let document = Document::load(config)?;
        match operation {
            Operation::Prepare => prepare(&document, &bytes),
            _ => intent_operation(operation, &document, &bytes),
        }
    };
    match execute() {
        Ok(report) => report,
        Err(status) => Report::refusal(operation, status),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{ordinary_restore::tests::Fixture, sync::mode::SyncModeChoice, Fault};
    use serde_json::{json, Value};
    use std::fs;

    fn value(report: Report) -> Value {
        let mut bytes = Vec::new();
        report.write_to(&mut bytes).unwrap();
        assert!(bytes.len() <= MAX_IO && bytes.ends_with(b"\n"));
        assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE_"));
        serde_json::from_slice(&bytes).unwrap()
    }
    fn prepared(target: &Fixture, source: &Path) -> Vec<u8> {
        let bytes = serde_json::to_vec(
            &json!({"restore_version":1,"operation":Uuid::new_v4(),"source":source}),
        )
        .unwrap();
        let report = value(run(
            Operation::Prepare,
            Path::new("-"),
            Some(target.root.join("config.json")),
            &mut bytes.as_slice(),
        ));
        assert_eq!(report["outcome"], "prepared");
        serde_json::to_vec(&json!({"source":source,"intent":report["data"]["intent"]})).unwrap()
    }
    #[test]
    fn actual_reports_distinguish_retained_staging_activation_and_changed_readback() {
        let source = Fixture::new();
        let archive = source.archive();
        for phase in 0..3 {
            let target = Fixture::new();
            drop(target.open());
            let bytes = prepared(&target, &archive);
            let document = Document::load(Some(target.root.join("config.json")))
                .unwrap_or_else(|_| panic!("fixture document"));
            let report = intent_operation_with(
                Operation::Apply,
                &document,
                &bytes,
                |store, path, conditions, doc| {
                    if phase == 2 {
                        store.ordinary_restore_changed_readback(path, conditions, || doc.original())
                    } else {
                        store.set_fault(if phase == 0 {
                            Fault::BeforeActivation
                        } else {
                            Fault::AfterActivation
                        })?;
                        store.apply_ordinary_restore(path, conditions, || doc.original())
                    }
                },
            )
            .unwrap_or_else(|_| panic!("fixture admission"));
            assert!(matches!(
                Store::open_existing(target.paths()),
                Err(Refusal::Busy)
            ));
            let report = value(report);
            assert_eq!(report["status"], "failed");
            assert_eq!(report["disk_readback_verified"], false);
            assert!(report["data"].is_null());
            assert_eq!(
                report["outcome"],
                if phase == 0 {
                    "not-activated"
                } else {
                    "unknown"
                }
            );
            let observed = value(run(
                Operation::Status,
                Path::new("-"),
                Some(target.root.join("config.json")),
                &mut bytes.as_slice(),
            ));
            assert_eq!(
                observed["outcome"],
                if phase == 1 {
                    "desired-generation-observed"
                } else {
                    "unresolved"
                }
            );
        }
    }
    #[test]
    fn report_prefix_keeps_real_store_mode_custody_then_releases_without_second_report() {
        let source = Fixture::new();
        let archive = source.archive();
        let target = Fixture::new();
        drop(target.open());
        let bytes = prepared(&target, &archive);
        let report = run(
            Operation::Apply,
            Path::new("-"),
            Some(target.root.join("config.json")),
            &mut bytes.as_slice(),
        );
        assert!(matches!(report.outcome, Outcome::Restored));
        let store = &report._custody.as_ref().unwrap().store;
        assert!(matches!(
            Store::open_existing(target.paths()),
            Err(Refusal::Busy)
        ));
        assert!(store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                SyncModeChoice::LocalOnly
            )
            .is_err());
        struct Prefix {
            bytes: Vec<u8>,
        }
        impl Write for Prefix {
            fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
                if !self.bytes.is_empty() {
                    return Err(std::io::Error::other("deliberate prefix failure"));
                }
                let n = input.len().min(17);
                self.bytes.extend_from_slice(&input[..n]);
                Ok(n)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = Prefix { bytes: Vec::new() };
        assert!(report.write_to(&mut writer).is_err());
        assert_eq!(writer.bytes.len(), 17);
        assert!(Store::open_existing(target.paths()).is_ok());
        let observed = value(run(
            Operation::Status,
            Path::new("-"),
            Some(target.root.join("config.json")),
            &mut bytes.as_slice(),
        ));
        assert_eq!(observed["outcome"], "desired-generation-observed");
        assert_eq!(observed["disk_readback_verified"], false);
    }
    #[test]
    fn original_configuration_is_rechecked_inside_actual_publication_mutex() {
        let source = Fixture::new();
        let archive = source.archive();
        let target = Fixture::new();
        drop(target.open());
        let bytes = prepared(&target, &archive);
        let document = Document::load(Some(target.root.join("config.json")))
            .unwrap_or_else(|_| panic!("fixture document"));
        let request: IntentRequest = parse(&bytes).unwrap_or_else(|_| panic!("fixture packet"));
        let report = intent_operation_with(
            Operation::Apply,
            &document,
            &bytes,
            |store, path, conditions, doc| {
                let mut current = fs::read(&doc.path)?;
                current.push(b' ');
                fs::write(&doc.path, current)?;
                store.apply_ordinary_restore(path, conditions, || doc.original())
            },
        )
        .unwrap_or_else(|_| panic!("fixture admission"));
        let report = value(report);
        assert_eq!(report["status"], "conflict");
        assert_eq!(report["outcome"], "not-dispatched");
        assert!(!target
            .paths()
            .data
            .join("generations")
            .join(request.intent.target_generation.to_string())
            .exists());
    }
}

//! Explicit original-bound ordinary export. No initialization or runtime/provider path.
use crate::{
    config::{inspection, Config},
    storage::{
        digest, files,
        maintenance::Refusal,
        ordinary_export::{self, Conditions, Plan, Publication, Refused, Summary},
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
pub const MAX_IO: usize = 16384;
#[derive(Clone, Copy, Debug, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    Prepare,
    Apply,
    Status,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
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
    Verified,
    DesiredArchiveObserved,
    Unresolved,
    NotDispatched,
    NotPublished,
    ReadOnly,
    Unknown,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepare {
    export_version: u32,
    operation: Uuid,
    destination: PathBuf,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    export_version: u32,
    operation: Uuid,
    config_path_sha256: String,
    config_sha256: String,
    destination_path_sha256: String,
    actor: Uuid,
    mode_sha256: String,
    base_generation: Uuid,
    base_history_sha256: String,
    configuration_snapshot_sha256: Option<String>,
    archive_sha256: String,
    complete_media: bool,
}
impl Intent {
    fn validate(&self) -> std::result::Result<(), Status> {
        if self.export_version != 1 {
            return Err(Status::Unsupported);
        }
        if [self.operation, self.actor, self.base_generation]
            .iter()
            .any(Uuid::is_nil)
            || [
                &self.config_path_sha256,
                &self.config_sha256,
                &self.destination_path_sha256,
                &self.mode_sha256,
                &self.base_history_sha256,
                &self.archive_sha256,
            ]
            .iter()
            .any(|h| !valid_digest(h))
            || self
                .configuration_snapshot_sha256
                .as_ref()
                .is_some_and(|h| !valid_digest(h))
        {
            return Err(Status::InvalidInput);
        }
        Ok(())
    }
    fn conditions(&self) -> Conditions {
        Conditions {
            base_generation: self.base_generation,
            base_history_sha256: self.base_history_sha256.clone(),
            configuration_snapshot_sha256: self.configuration_snapshot_sha256.clone(),
            archive_sha256: self.archive_sha256.clone(),
            complete_media: self.complete_media,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IntentRequest {
    destination: PathBuf,
    intent: Intent,
}
#[derive(Serialize)]
#[serde(untagged)]
enum Data {
    Prepared {
        source: Summary,
        intent: Box<Intent>,
    },
    Observed {
        archive_sha256: String,
        inspection: Summary,
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
            outcome: if matches!(operation, Operation::Status) {
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
    fn success(operation: Operation, outcome: Outcome, data: Option<Data>) -> Self {
        let mut r = Self::refusal(operation, Status::Ok);
        r.outcome = outcome;
        r.data = data;
        r
    }
    pub fn write_to(self, out: &mut impl Write) -> Result<()> {
        let mut b = serde_json::to_vec(&self)?;
        b.push(b'\n');
        ensure!(b.len() <= MAX_IO, "ordinary export report bound");
        out.write_all(&b)?;
        Ok(())
    }
}
fn path_hash(path: &Path, config: bool) -> std::result::Result<String, Status> {
    if !path.is_absolute() {
        return Err(Status::InvalidInput);
    }
    let value = path.to_str().ok_or(Status::InvalidInput)?;
    let prefix = if config {
        "urn:remarkable-buddies:ordinary-export-config:v1:"
    } else {
        "urn:remarkable-buddies:ordinary-export-path:v1:"
    };
    Ok(digest(format!("{prefix}{value}").as_bytes()))
}
struct Document {
    path: PathBuf,
    path_hash: String,
    hash: String,
    config: Config,
}
impl Document {
    fn load(path: Option<PathBuf>) -> std::result::Result<Self, Status> {
        let path = path.ok_or(Status::Unavailable)?;
        let path_hash = path_hash(&path, true)?;
        let bytes = files::read(&path, MAX_RECORD as u64).map_err(|_| Status::Unavailable)?;
        let (config, _) = inspection::document(&bytes).map_err(|s| match s {
            inspection::Status::Unsupported => Status::Unsupported,
            _ => Status::InvalidInput,
        })?;
        if config.sync.enabled || config.validate().is_err() {
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
        let b = files::read(&self.path, MAX_RECORD as u64).map_err(|_| Refused::Unavailable)?;
        if digest(&b) != self.hash {
            return Err(Refused::Conflict.into());
        }
        Ok(())
    }
    fn destination(&self, path: &Path) -> std::result::Result<String, Status> {
        let hash = path_hash(path, false)?;
        files::safe_path(path).map_err(|_| Status::InvalidInput)?;
        match std::fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err(Status::InvalidInput),
        }
        let parent = path.parent().ok_or(Status::InvalidInput)?;
        if !std::fs::symlink_metadata(parent)
            .map_err(|_| Status::Unavailable)?
            .is_dir()
        {
            return Err(Status::InvalidInput);
        }
        let parent = std::fs::canonicalize(parent).map_err(|_| Status::Unavailable)?;
        let canonical = parent.join(path.file_name().ok_or(Status::InvalidInput)?);
        for owned in [
            &self.config.paths.data,
            &self.config.paths.cache,
            &self.config.paths.credentials,
        ] {
            if crate::restore_maintenance::overlap(path, owned).map_err(|_| Status::InvalidInput)? {
                return Err(Status::InvalidInput);
            }
            files::safe_path(owned).map_err(|_| Status::InvalidInput)?;
            let root = std::fs::canonicalize(owned).map_err(|_| Status::Unavailable)?;
            if crate::restore_maintenance::overlap(&canonical, &root)
                .map_err(|_| Status::InvalidInput)?
            {
                return Err(Status::InvalidInput);
            }
        }
        Ok(hash)
    }
    fn binding(&self, path: &Path, expected_hash: &str) -> Result<()> {
        self.original()?;
        let hash = self.destination(path).map_err(|s| match s {
            Status::Unavailable => Refused::Unavailable,
            _ => Refused::InvalidInput,
        })?;
        if hash != expected_hash {
            return Err(Refused::Conflict.into());
        }
        Ok(())
    }
}
#[derive(Deserialize)]
struct Token {
    actor: Uuid,
    sha256: String,
}
fn acquire(
    doc: &Document,
    expected: Option<&Intent>,
) -> std::result::Result<(Custody, Token), Status> {
    let store = Arc::new(
        Store::open_existing(doc.config.paths.clone()).map_err(|r| match r {
            Refusal::Busy => Status::Busy,
            Refusal::Unavailable => Status::Unavailable,
            Refusal::Unsupported => Status::Unsupported,
        })?,
    );
    let current = store
        .sync_mode_snapshot()
        .map_err(|_| Status::Unsupported)?;
    if !matches!(current.mode, None | Some(SyncMode::LocalOnly)) {
        return Err(Status::Unsupported);
    }
    let token: Token =
        serde_json::from_value(serde_json::to_value(&current.token).map_err(|_| Status::Failed)?)
            .map_err(|_| Status::Failed)?;
    let mode = match expected {
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
    let lease = store
        .ordinary_mode_lease(&mode)
        .map_err(|_| Status::Conflict)?;
    Ok((
        Custody {
            store,
            _mode: lease,
        },
        token,
    ))
}
fn parse<T: DeserializeOwned>(bytes: &[u8]) -> std::result::Result<T, Status> {
    let raw = inspection::strict_json(bytes).map_err(|_| Status::InvalidInput)?;
    if !raw.is_object()
        || raw
            .get("intent")
            .is_some_and(|i| !i.is_object() || i.get("configuration_snapshot_sha256").is_none())
    {
        return Err(Status::InvalidInput);
    }
    serde_json::from_slice(bytes).map_err(|_| Status::InvalidInput)
}
fn error_status(error: &anyhow::Error) -> Status {
    match error.downcast_ref::<Refused>() {
        Some(Refused::InvalidInput) => Status::InvalidInput,
        Some(Refused::Unavailable) => Status::Unavailable,
        Some(Refused::Unsupported) => Status::Unsupported,
        Some(Refused::Conflict) => Status::Conflict,
        None => Status::Failed,
    }
}
fn prepare(request: Prepare, path: Option<PathBuf>) -> Report {
    let op = Operation::Prepare;
    if request.export_version != 1 {
        return Report::refusal(op, Status::Unsupported);
    }
    if request.operation.is_nil() {
        return Report::refusal(op, Status::InvalidInput);
    }
    let doc = match Document::load(path) {
        Ok(v) => v,
        Err(s) => return Report::refusal(op, s),
    };
    let destination_hash = match doc.destination(&request.destination) {
        Ok(v) => v,
        Err(s) => return Report::refusal(op, s),
    };
    let (custody, token) = match acquire(&doc, None) {
        Ok(v) => v,
        Err(s) => return Report::refusal(op, s),
    };
    let plan = custody
        .store
        .prepare_ordinary_export(|| doc.binding(&request.destination, &destination_hash));
    let Plan {
        conditions: c,
        source,
    } = match plan {
        Ok(v) => v,
        Err(e) => return Report::refusal(op, error_status(&e)),
    };
    let intent = Intent {
        export_version: 1,
        operation: request.operation,
        config_path_sha256: doc.path_hash,
        config_sha256: doc.hash,
        destination_path_sha256: destination_hash,
        actor: token.actor,
        mode_sha256: token.sha256,
        base_generation: c.base_generation,
        base_history_sha256: c.base_history_sha256,
        configuration_snapshot_sha256: c.configuration_snapshot_sha256,
        archive_sha256: c.archive_sha256,
        complete_media: c.complete_media,
    };
    let mut report = Report::success(
        op,
        Outcome::Prepared,
        Some(Data::Prepared {
            source,
            intent: Box::new(intent),
        }),
    );
    report._custody = Some(custody);
    report
}
fn apply(request: IntentRequest, path: Option<PathBuf>) -> Report {
    let op = Operation::Apply;
    let i = &request.intent;
    let doc = match Document::load(path) {
        Ok(v) => v,
        Err(s) => return Report::refusal(op, s),
    };
    if doc.hash != i.config_sha256 || doc.path_hash != i.config_path_sha256 {
        return Report::refusal(op, Status::Conflict);
    }
    let destination_hash = match doc.destination(&request.destination) {
        Ok(v) => v,
        Err(s) => return Report::refusal(op, s),
    };
    if destination_hash != i.destination_path_sha256 {
        return Report::refusal(op, Status::Conflict);
    }
    let (custody, _) = match acquire(&doc, Some(i)) {
        Ok(v) => v,
        Err(s) => return Report::refusal(op, s),
    };
    let result = custody
        .store
        .apply_ordinary_export(&request.destination, &i.conditions(), || {
            doc.binding(&request.destination, &destination_hash)
        });
    let mut report = match result {
        Ok(Publication::Verified(inspection)) => {
            let mut r = Report::success(
                op,
                Outcome::Verified,
                Some(Data::Observed {
                    archive_sha256: i.archive_sha256.clone(),
                    inspection,
                }),
            );
            r.disk_readback_verified = true;
            r
        }
        Ok(Publication::NotPublished) => {
            let mut r = Report::refusal(op, Status::Failed);
            r.outcome = Outcome::NotPublished;
            r
        }
        Ok(Publication::Unknown) => {
            let mut r = Report::refusal(op, Status::Failed);
            r.outcome = Outcome::Unknown;
            r
        }
        Err(e) => Report::refusal(op, error_status(&e)),
    };
    report._custody = Some(custody);
    report
}
fn status(request: IntentRequest) -> Report {
    let op = Operation::Status;
    match path_hash(&request.destination, false) {
        Ok(h) if h == request.intent.destination_path_sha256 => {}
        Ok(_) => return Report::refusal(op, Status::Conflict),
        Err(s) => return Report::refusal(op, s),
    }
    match ordinary_export::observe(&request.destination, &request.intent.conditions()) {
        Ok(Some(inspection)) => Report::success(
            op,
            Outcome::DesiredArchiveObserved,
            Some(Data::Observed {
                archive_sha256: request.intent.archive_sha256,
                inspection,
            }),
        ),
        Ok(None) => Report::success(op, Outcome::Unresolved, None),
        Err(e) => Report::refusal(op, error_status(&e)),
    }
}
pub fn run(
    operation: Operation,
    input: &Path,
    path: Option<PathBuf>,
    stdin: &mut impl Read,
) -> Report {
    if input != Path::new("-") {
        return Report::refusal(operation, Status::Unsupported);
    }
    let mut bytes = vec![];
    if stdin
        .take((MAX_IO + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > MAX_IO
    {
        return Report::refusal(operation, Status::InvalidInput);
    }
    match operation {
        Operation::Prepare => match parse::<Prepare>(&bytes) {
            Ok(r) => prepare(r, path),
            Err(s) => Report::refusal(operation, s),
        },
        Operation::Apply | Operation::Status => {
            let request: IntentRequest = match parse(&bytes) {
                Ok(v) => v,
                Err(s) => return Report::refusal(operation, s),
            };
            if let Err(s) = request.intent.validate() {
                return Report::refusal(operation, s);
            }
            if matches!(operation, Operation::Apply) {
                apply(request, path)
            } else {
                status(request)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::ordinary_restore::tests::Fixture;
    use serde_json::{json, Value};
    use std::{fs, io};
    fn run(f: &Fixture, op: Operation, input: &Value) -> Report {
        super::run(
            op,
            Path::new("-"),
            Some(f.root.join("config.json")),
            &mut serde_json::to_vec(input).unwrap().as_slice(),
        )
    }
    fn finish(report: Report) -> Value {
        let mut b = vec![];
        report.write_to(&mut b).unwrap();
        assert!(!String::from_utf8_lossy(&b).contains("PRIVATE_"));
        serde_json::from_slice(&b).unwrap()
    }
    fn prepare(f: &Fixture) -> Value {
        let dest = f.root.join("export");
        let r = finish(run(
            f,
            Operation::Prepare,
            &json!({"export_version":1,"operation":Uuid::new_v4(),"destination":dest}),
        ));
        assert_eq!(r["outcome"], "prepared");
        json!({"destination":dest,"intent":r["data"]["intent"]})
    }
    #[test]
    fn verified_report_and_pure_status_ignore_deleted_config_and_source() {
        let f = Fixture::new();
        drop(f.open());
        let input = prepare(&f);
        assert_eq!(
            finish(run(&f, Operation::Status, &input))["outcome"],
            "unresolved"
        );
        assert_eq!(
            finish(run(&f, Operation::Apply, &input))["outcome"],
            "verified"
        );
        fs::rename(f.paths().data, f.root.join("moved-data")).unwrap();
        fs::remove_file(f.root.join("config.json")).unwrap();
        let r = finish(run(&f, Operation::Status, &input));
        assert_eq!(r["outcome"], "desired-archive-observed");
        assert_eq!(r["disk_readback_verified"], false);
        assert_eq!(r["runtime_activation"], "unverified");
    }
    #[test]
    fn original_config_mode_actor_and_destination_changes_refuse_without_output() {
        let f = Fixture::new();
        drop(f.open());
        let input = prepare(&f);
        let mut forged_actor = input.clone();
        forged_actor["intent"]["actor"] = json!(Uuid::new_v4());
        assert_eq!(
            finish(run(&f, Operation::Apply, &forged_actor))["status"],
            "conflict"
        );
        let mut forged = input.clone();
        forged["destination"] = json!(f.root.join("different"));
        assert_eq!(
            finish(run(&f, Operation::Apply, &forged))["status"],
            "conflict"
        );
        let path = f.root.join("config.json");
        let mut b = fs::read(&path).unwrap();
        b.push(b' ');
        fs::write(&path, &b).unwrap();
        assert_eq!(
            finish(run(&f, Operation::Apply, &input))["status"],
            "conflict"
        );
        let input = prepare(&f);
        let store = f.open();
        store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                crate::storage::sync::mode::SyncModeChoice::LocalOnly,
            )
            .unwrap();
        drop(store);
        assert_eq!(
            finish(run(&f, Operation::Apply, &input))["status"],
            "conflict"
        );
        assert!(!f.root.join("export").exists());
    }
    #[test]
    fn all_root_ancestry_parent_and_windows_case_aliases_refuse_before_creation() {
        let f = Fixture::new();
        drop(f.open());
        for dest in [
            f.paths().data.join("new"),
            f.paths().cache.join("new"),
            f.paths().credentials.join("new"),
            f.root.clone(),
            f.root.join("missing-parent/new"),
        ] {
            let r = finish(run(
                &f,
                Operation::Prepare,
                &json!({"export_version":1,"operation":Uuid::new_v4(),"destination":dest}),
            ));
            assert_ne!(r["status"], "ok");
            assert!(!dest.join("backup.json").exists());
        }
        #[cfg(windows)]
        for owned in [f.paths().data, f.paths().cache, f.paths().credentials] {
            let path = owned
                .to_str()
                .unwrap()
                .chars()
                .map(|c| {
                    if c.is_ascii_lowercase() {
                        c.to_ascii_uppercase()
                    } else if c.is_ascii_uppercase() {
                        c.to_ascii_lowercase()
                    } else {
                        c
                    }
                })
                .collect::<String>();
            let dest = PathBuf::from(path).join("new-case-alias");
            let r = finish(run(
                &f,
                Operation::Prepare,
                &json!({"export_version":1,"operation":Uuid::new_v4(),"destination":dest}),
            ));
            assert_eq!(r["status"], "invalid-input");
            assert!(!dest.exists());
        }
    }
    #[test]
    fn consuming_prefix_retains_store_and_mode_until_failure_without_second_report() {
        let f = Fixture::new();
        drop(f.open());
        let input = prepare(&f);
        let report = run(&f, Operation::Apply, &input);
        assert_eq!(report.status, Status::Ok);
        let store = &report._custody.as_ref().unwrap().store;
        let snap = store.sync_mode_snapshot().unwrap();
        assert!(store
            .set_sync_mode(
                &snap.token,
                Uuid::new_v4(),
                crate::storage::sync::mode::SyncModeChoice::LocalOnly
            )
            .is_err());
        struct Prefix {
            paths: crate::storage::StorePaths,
            calls: usize,
            b: Vec<u8>,
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
                self.b.extend_from_slice(&b[..17]);
                Ok(17)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut out = Prefix {
            paths: f.paths(),
            calls: 0,
            b: vec![],
        };
        assert!(report.write_to(&mut out).is_err());
        assert_eq!(out.calls, 2);
        assert_eq!(out.b.len(), 17);
        drop(Store::open_existing(f.paths()).unwrap());
    }
    #[test]
    fn strict_duplicate_nullable_shapes_caps_and_cross_kind_refuse() {
        for raw in [
            br#"[]"#.as_slice(),
            br#"{"intent":[]}"#,
            br#"{"export_version":1,"export_version":1}"#,
            br#"{"restore_version":1}"#,
        ] {
            let mut raw = raw;
            let r = super::run(Operation::Apply, Path::new("-"), None, &mut raw);
            assert_eq!(r.status, Status::InvalidInput);
        }
        let f = Fixture::new();
        drop(f.open());
        let input = prepare(&f);
        let mut missing = input.clone();
        missing["intent"]
            .as_object_mut()
            .unwrap()
            .remove("configuration_snapshot_sha256");
        assert_eq!(
            finish(run(&f, Operation::Status, &missing))["status"],
            "invalid-input"
        );
        let req = json!({"export_version":1,"operation":Uuid::new_v4(),"destination":f.root.join("export")});
        let mut bytes = serde_json::to_vec(&req).unwrap();
        bytes.resize(MAX_IO, b' ');
        let r = super::run(
            Operation::Prepare,
            Path::new("-"),
            Some(f.root.join("config.json")),
            &mut bytes.as_slice(),
        );
        assert_eq!(r.status, Status::Ok);
        drop(r);
        bytes.push(b' ');
        assert_eq!(
            super::run(
                Operation::Prepare,
                Path::new("-"),
                Some(f.root.join("config.json")),
                &mut bytes.as_slice()
            )
            .status,
            Status::InvalidInput
        );
        assert!(!f.root.join("export").exists());
    }
    #[test]
    fn config_change_inside_actual_writer_binding_refuses_before_reservation() {
        let f = Fixture::new();
        drop(f.open());
        let dest = f.root.join("out");
        let doc = Document::load(Some(f.root.join("config.json"))).unwrap();
        let hash = doc.destination(&dest).unwrap();
        let (custody, _) = acquire(&doc, None).unwrap();
        let plan = custody
            .store
            .prepare_ordinary_export(|| doc.binding(&dest, &hash))
            .unwrap();
        let r = custody
            .store
            .apply_ordinary_export(&dest, &plan.conditions, || {
                let mut b = fs::read(&doc.path).unwrap();
                b.push(b' ');
                fs::write(&doc.path, b).unwrap();
                doc.binding(&dest, &hash)
            });
        assert!(matches!(
            r.err().unwrap().downcast_ref::<Refused>(),
            Some(Refused::Conflict)
        ));
        assert!(!dest.exists());
    }
}

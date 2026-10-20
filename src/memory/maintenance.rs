//! One-shot, bounded local maintenance. No provider, sync worker or device initialization.
use super::{Forget, Key, Mutation, Query, Status};
use crate::{
    config::Config,
    storage::{self, files, Store, Uuid, MAX_ITEMS, MAX_METADATA, MAX_RECORD},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

pub const MAX_RESPONSE: usize = MAX_METADATA + 64 * 1024;

#[derive(Clone, Copy, Debug, clap::ValueEnum, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    Inspect,
    Query,
    Put,
    Forget,
    InitializeAggregate,
    InitializeSelection,
    RegisterSelection,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ReportStatus {
    Ok,
    Partial,
    Busy,
    Unavailable,
    Unsupported,
    Conflict,
    InvalidInput,
    Failed,
}
#[derive(Serialize)]
pub struct Report {
    pub report_version: u32,
    pub operation: Operation,
    pub status: ReportStatus,
    pub incomplete: bool,
    pub data: Option<serde_json::Value>,
    #[serde(skip)]
    pending: Option<Pending>,
    #[serde(skip)]
    mode_guard: Option<ModeGuard>,
    #[serde(skip)]
    provider_guard: Option<Arc<dyn super::Provider>>,
}
struct ModeGuard {
    _ordinary: Option<storage::sync::mode::OrdinaryModeLease>,
    _shared: Option<storage::sync::mode::SyncModeLease>,
}
impl ModeGuard {
    fn acquire(store: &Arc<Store>, config: &Config) -> anyhow::Result<Self> {
        anyhow::ensure!(!config.sync.enabled, "stop legacy sync before host memory");
        let current = store.sync_mode_snapshot()?;
        if let Some(identity) = &config.memory.selection {
            let lease = store.sync_mode_lease(&current.token)?;
            lease.require_shared_scope(&identity.scope()?)?;
            Ok(Self {
                _ordinary: None,
                _shared: Some(lease),
            })
        } else {
            Ok(Self {
                _ordinary: Some(store.ordinary_mode_lease(&current.token)?),
                _shared: None,
            })
        }
    }
}
struct Pending {
    provider: Arc<dyn super::Provider>,
    view: PendingView,
    report_sha256: String,
}
enum PendingView {
    Query(super::QueryResult),
    Inspection(super::Inspection),
}
impl Report {
    pub fn new(operation: Operation, status: ReportStatus) -> Self {
        Self {
            report_version: 1,
            operation,
            status,
            incomplete: false,
            data: None,
            pending: None,
            mode_guard: None,
            provider_guard: None,
        }
    }
    /// Formatting only; selected current handoff requires `write_to`.
    /// Bound the entire report including escaping/newline. Never retry a mutation.
    pub fn encoded(&self) -> anyhow::Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        if bytes.len() <= MAX_RESPONSE {
            return Ok(bytes);
        }
        let mut limited = Self::new(self.operation, ReportStatus::Unavailable);
        limited.incomplete = true;
        let mut bytes = serde_json::to_vec(&limited)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
    pub fn write_to(&self, writer: &mut impl std::io::Write) -> anyhow::Result<()> {
        let encoded = self.encoded()?;
        let Some(pending) = &self.pending else {
            return Ok(writer.write_all(&encoded)?);
        };
        let mut attempted = false;
        let guarded = if storage::digest(&serde_json::to_vec(self)?) != pending.report_sha256 {
            Err(anyhow::anyhow!("memory report payload changed"))
        } else {
            let mut consume = || {
                attempted = true;
                Ok(writer.write_all(&encoded)?)
            };
            match &pending.view {
                PendingView::Query(view) => pending.provider.with_current(view, &mut consume),
                PendingView::Inspection(view) => {
                    view.with_current(pending.provider.as_ref(), &mut consume)
                }
            }
        };
        if let Err(error) = guarded {
            // An I/O failure may have written a prefix. Never append a second report.
            if attempted {
                return Err(error);
            }
            let mut unavailable = Self::new(self.operation, ReportStatus::Unavailable);
            unavailable.incomplete = true;
            writer.write_all(&unavailable.encoded()?)?;
        }
        Ok(())
    }
}
enum Input {
    Inspect(Key),
    Query(Query),
    Put(Mutation),
    Forget(Forget),
    InitializeAggregate(super::selected::InitializeAggregate),
    InitializeSelection(super::selected::InitializeSelection),
    RegisterSelection(super::selected::RegisterSelection),
}

// New stdin wire only. Preserve the existing file Key decoder's compatibility.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectKeyWire {
    profile: Uuid,
    scope: InspectScopeWire,
    fact: Uuid,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum InspectScopeWire {
    Global {},
    Document { document: Uuid },
}
impl From<InspectKeyWire> for Key {
    fn from(wire: InspectKeyWire) -> Self {
        Self {
            profile: wire.profile,
            scope: match wire.scope {
                InspectScopeWire::Global {} => super::Scope::Global,
                InspectScopeWire::Document { document } => super::Scope::Document { document },
            },
            fact: wire.fact,
        }
    }
}
impl Input {
    fn read(
        operation: Operation,
        path: &Path,
        profile: Uuid,
        selection: Option<&super::catalog::Identity>,
    ) -> anyhow::Result<Self> {
        let input = match operation {
            Operation::Inspect => Self::Inspect(files::json(path, MAX_RECORD as u64)?),
            Operation::Query => Self::Query(files::json(path, MAX_RECORD as u64)?),
            Operation::Put => Self::Put(files::json(path, MAX_RECORD as u64)?),
            Operation::Forget => Self::Forget(files::json(path, MAX_RECORD as u64)?),
            Operation::InitializeAggregate => {
                Self::InitializeAggregate(files::json(path, MAX_RECORD as u64)?)
            }
            Operation::InitializeSelection => {
                Self::InitializeSelection(files::json(path, MAX_RECORD as u64)?)
            }
            Operation::RegisterSelection => {
                Self::RegisterSelection(files::json(path, MAX_RECORD as u64)?)
            }
        };
        input.validated(profile, selection)
    }
    fn read_stdin(
        operation: Operation,
        input: &mut impl Read,
        profile: Uuid,
        selection: Option<&super::catalog::Identity>,
    ) -> anyhow::Result<Self> {
        let mut bytes = Vec::new();
        input
            .take((MAX_RECORD + 1) as u64)
            .read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() <= MAX_RECORD,
            "memory query input exceeds bound"
        );
        let input = match operation {
            Operation::Query => Self::Query(serde_json::from_slice(&bytes)?),
            Operation::Inspect => {
                Self::Inspect(serde_json::from_slice::<InspectKeyWire>(&bytes)?.into())
            }
            _ => anyhow::bail!("unsupported memory stdin operation"),
        };
        input.validated(profile, selection)
    }
    fn validated(
        self,
        profile: Uuid,
        selection: Option<&super::catalog::Identity>,
    ) -> anyhow::Result<Self> {
        match &self {
            Self::Inspect(key) => {
                key.validate()?;
                anyhow::ensure!(key.profile == profile, "foreign profile");
            }
            Self::Query(query) => query.validate()?,
            Self::Put(mutation) => {
                mutation.fact.validate()?;
                anyhow::ensure!(mutation.fact.key.profile == profile, "foreign profile");
                valid_operation(mutation.operation, &mutation.expected)?;
            }
            Self::Forget(mutation) => {
                mutation.key.validate()?;
                anyhow::ensure!(mutation.key.profile == profile, "foreign profile");
                valid_operation(mutation.operation, &mutation.expected)?;
                anyhow::ensure!(
                    !mutation.expected.is_empty(),
                    "forget requires explicit heads"
                );
            }
            Self::InitializeAggregate(input) => input.validate(
                selection.ok_or_else(|| anyhow::anyhow!("selected configuration absent"))?,
            )?,
            Self::InitializeSelection(input) => input.validate(
                selection.ok_or_else(|| anyhow::anyhow!("selected configuration absent"))?,
            )?,
            Self::RegisterSelection(input) => input.validate(
                selection.ok_or_else(|| anyhow::anyhow!("selected configuration absent"))?,
            )?,
        }
        Ok(self)
    }
}
fn valid_operation(
    operation: Uuid,
    expected: &std::collections::BTreeSet<Uuid>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !operation.is_nil()
            && expected.len() <= MAX_ITEMS
            && expected.iter().all(|id| !id.is_nil()),
        "invalid memory operation"
    );
    let revision = Uuid::new_v5(&operation, b"remarkable-buddies:subject-memory:revision:v1");
    anyhow::ensure!(
        !expected.contains(&revision),
        "invalid self-parent memory operation"
    );
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    format: u32,
    actor: Uuid,
}
fn regular(path: &Path) -> bool {
    files::safe_path(path).is_ok() && fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
}
/// Pure state/path and nonblocking lock admission before normal Store startup.
fn preflight(config: &Config, allow_initialization: bool) -> Result<(), ReportStatus> {
    config
        .paths
        .validate()
        .map_err(|_| ReportStatus::Unsupported)?;
    let root = &config.paths.data;
    let fresh = match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => return Err(ReportStatus::Unavailable),
        Ok(metadata) => {
            if !metadata.is_dir() {
                return Err(ReportStatus::Unsupported);
            }
            fs::read_dir(root)
                .map_err(|_| ReportStatus::Unavailable)?
                .next()
                .is_none()
        }
    };
    if fresh {
        return if allow_initialization {
            Ok(())
        } else {
            Err(ReportStatus::Unavailable)
        };
    }
    let identity = root.join("identity.json");
    let current = root.join("CURRENT");
    let lock = root.join("store.lock");
    if ![&identity, &current, &lock]
        .iter()
        .all(|path| regular(path))
    {
        return Err(ReportStatus::Unavailable);
    }
    let marker: Marker = files::json(&identity, 1024).map_err(|_| ReportStatus::Unsupported)?;
    if marker.format != 1 || marker.actor.is_nil() {
        return Err(ReportStatus::Unsupported);
    }
    let generation: Uuid = files::json(&current, 128).map_err(|_| ReportStatus::Unsupported)?;
    if generation.is_nil() {
        return Err(ReportStatus::Unsupported);
    }
    let generation_root = root.join("generations").join(generation.to_string());
    for directory in [
        &generation_root,
        &generation_root.join("objects"),
        &generation_root.join("commits"),
    ] {
        if files::safe_path(directory).is_err()
            || !fs::symlink_metadata(directory).is_ok_and(|m| m.is_dir())
        {
            return Err(ReportStatus::Unavailable);
        }
    }
    if !regular(&generation_root.join("format.json")) {
        return Err(ReportStatus::Unavailable);
    }
    storage::selection::validate_generation(&generation_root, marker.actor, generation)
        .map_err(|_| ReportStatus::Unsupported)?;
    let handle = File::open(lock).map_err(|_| ReportStatus::Unavailable)?;
    match handle.try_lock() {
        Ok(()) => Ok(()),
        Err(std::fs::TryLockError::WouldBlock) => Err(ReportStatus::Busy),
        Err(std::fs::TryLockError::Error(_)) => Err(ReportStatus::Unsupported),
    }
    // The preflight handle drops before normal Store reacquisition; no unleased read follows.
}

pub fn run(operation: Operation, input_path: &Path, config_path: Option<PathBuf>) -> Report {
    run_with_input(operation, config_path, |profile, selection| {
        Input::read(operation, input_path, profile, selection)
    })
}

/// Read-only stdin transport. The caller supplies EOF and owns its I/O deadline.
pub fn run_stdin(
    operation: Operation,
    input: &mut impl Read,
    config_path: Option<PathBuf>,
) -> Report {
    if !matches!(operation, Operation::Query | Operation::Inspect) {
        return Report::new(operation, ReportStatus::Unsupported);
    }
    run_with_input(operation, config_path, |profile, selection| {
        Input::read_stdin(operation, input, profile, selection)
    })
}

fn run_with_input(
    operation: Operation,
    config_path: Option<PathBuf>,
    read_input: impl FnOnce(Uuid, Option<&super::catalog::Identity>) -> anyhow::Result<Input>,
) -> Report {
    let Some(config_path) = config_path else {
        return Report::new(operation, ReportStatus::Unavailable);
    };
    let config = match Config::load(Some(config_path)) {
        Ok(config) => config,
        Err(_) => return Report::new(operation, ReportStatus::Unavailable),
    };
    let Some(profile) = config.memory.profile else {
        return Report::new(operation, ReportStatus::Unavailable);
    };
    if config.sync.enabled {
        return Report::new(operation, ReportStatus::Unsupported);
    }
    let setup = matches!(
        operation,
        Operation::InitializeAggregate
            | Operation::InitializeSelection
            | Operation::RegisterSelection
    );
    if setup && config.memory.selection.is_none() {
        return Report::new(operation, ReportStatus::Unsupported);
    }
    let input = match read_input(profile, config.memory.selection.as_ref()) {
        Ok(input) => input,
        Err(_) => return Report::new(operation, ReportStatus::InvalidInput),
    };
    let allow_initialization = matches!(
        &input,
        Input::InitializeAggregate(_) | Input::InitializeSelection(_)
    ) || (config.memory.selection.is_none()
        && matches!(&input, Input::Put(mutation) if mutation.expected.is_empty()));
    if let Err(status) = preflight(&config, allow_initialization) {
        return Report::new(operation, status);
    }
    let store = match Store::open(config.paths.clone()) {
        Ok(store) => Arc::new(store),
        Err(error) => return Report::new(operation, store_open_status(&error)),
    };
    execute(operation, input, &config, store)
}

fn execute(operation: Operation, input: Input, config: &Config, store: Arc<Store>) -> Report {
    // Mode authority precedes all domain constructors and operations. The report
    // owns this lifetime through the actual consuming writer, including errors.
    let mode_guard = match ModeGuard::acquire(&store, config) {
        Ok(guard) => guard,
        Err(_) => return Report::new(operation, ReportStatus::Unsupported),
    };
    let selected = config
        .memory
        .selection
        .as_ref()
        .map(|identity| super::selected::SelectedMemory::new(store.clone(), identity.clone()))
        .transpose();
    let selected = match selected {
        Ok(selected) => selected,
        Err(_) => return Report::new(operation, ReportStatus::Unsupported),
    };
    let memory = match super::configured(store, &config.memory) {
        Ok(memory) => memory,
        Err(_) => return Report::new(operation, ReportStatus::Unsupported),
    };
    let mut report = Report::new(operation, ReportStatus::Ok);
    report.mode_guard = Some(mode_guard);
    report.provider_guard = Some(memory.clone());
    let mut pending_view = None;
    let result = match input {
        Input::Inspect(key) => memory.inspect_for_handoff(&key).and_then(|view| {
            let data=serde_json::to_value(&view.envelopes)?;
            if view.selected() { pending_view=Some(PendingView::Inspection(view)); }
            Ok(data)
        }),
        Input::Query(query) => memory.query(&query).map(|result| {
            report.status = match result.status { Status::Partial => ReportStatus::Partial, Status::Unavailable => ReportStatus::Unavailable, _ => ReportStatus::Ok };
            report.incomplete = matches!(result.status, Status::Partial | Status::Unavailable);
            let data = serde_json::json!({"query_status":result.status,"diagnostics":result.diagnostics,"context":result.context});
            if result.selected() { pending_view = Some(PendingView::Query(result)); }
            data
        }),
        Input::Put(mutation) => memory.put(mutation).map(|receipt| serde_json::json!({"transaction":receipt})),
        Input::Forget(mutation) => memory.forget(mutation).map(|receipt| serde_json::json!({"transaction":receipt})),
        Input::InitializeAggregate(input) => selected.as_ref().ok_or_else(|| anyhow::anyhow!("selection absent"))
            .and_then(|memory| memory.initialize_aggregate(input)).and_then(|receipt| Ok(serde_json::to_value(receipt)?)),
        Input::InitializeSelection(input) => selected.as_ref().ok_or_else(|| anyhow::anyhow!("selection absent"))
            .and_then(|memory| memory.initialize_selection(input)).and_then(|receipt| Ok(serde_json::to_value(receipt)?)),
        Input::RegisterSelection(input) => selected.as_ref().ok_or_else(|| anyhow::anyhow!("selection absent"))
            .and_then(|memory| memory.register_selection(input)).and_then(|receipt| Ok(serde_json::to_value(receipt)?)),
    };
    match result {
        Ok(data) => {
            report.data = Some(data);
            if let Some(view) = pending_view {
                match serde_json::to_vec(&report) {
                    Ok(bytes) => {
                        report.pending = Some(Pending {
                            provider: memory,
                            view,
                            report_sha256: storage::digest(&bytes),
                        })
                    }
                    Err(_) => {
                        report.status = ReportStatus::Unavailable;
                        report.incomplete = true;
                        report.data = None;
                    }
                }
            }
            report
        }
        Err(error) => {
            if let Some(conflict) = error.downcast_ref::<storage::Conflict>() {
                if conflict.heads.len() > MAX_ITEMS {
                    report.status = ReportStatus::Unavailable;
                    report.incomplete = true;
                } else {
                    report.status = ReportStatus::Conflict;
                    report.data = Some(serde_json::json!({"current_heads":conflict.heads}));
                }
            } else {
                report.status = ReportStatus::Unavailable;
                report.incomplete = true;
            }
            report
        }
    }
}

// Called only for the trusted Store::open stage, never parser/user errors.
fn store_open_status(error: &anyhow::Error) -> ReportStatus {
    if error.to_string() == "store is owned by another process" {
        ReportStatus::Busy
    } else {
        ReportStatus::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inspect_stdin_is_strict_at_both_levels_then_reuses_key_validation() {
        let profile = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
        let fact = Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap();
        let global = serde_json::json!({"profile":profile,"scope":{"kind":"global"},"fact":fact});
        let read =
            |bytes: &[u8]| Input::read_stdin(Operation::Inspect, &mut &bytes[..], profile, None);
        let Input::Inspect(key) = read(&serde_json::to_vec(&global).unwrap()).unwrap() else {
            panic!("wrong inspection input")
        };
        assert_eq!(
            key.record_id().unwrap().to_string(),
            "aa0b8ad5-2bb5-5515-84d6-668fb768d199"
        );
        let mut document = global.clone();
        document["scope"] = serde_json::json!({"kind":"document","document":Uuid::new_v4()});
        assert!(read(&serde_json::to_vec(&document).unwrap()).is_ok());
        for value in [
            serde_json::json!({"profile":profile,"scope":{"kind":"global","unknown":1},"fact":fact}),
            serde_json::json!({"profile":profile,"scope":{"kind":"document","document":Uuid::new_v4(),"unknown":1},"fact":fact}),
            serde_json::json!({"profile":profile,"scope":{"kind":"document","document":Uuid::nil()},"fact":fact}),
            serde_json::json!({"profile":profile,"scope":{"kind":"unknown"},"fact":fact}),
            serde_json::json!({"profile":Uuid::new_v4(),"scope":{"kind":"global"},"fact":fact}),
            serde_json::json!({"profile":Uuid::nil(),"scope":{"kind":"global"},"fact":fact}),
            serde_json::json!({"profile":profile,"scope":{"kind":"global"},"fact":Uuid::nil()}),
            serde_json::json!({"profile":profile,"scope":{"kind":"global"},"fact":fact,"unknown":1}),
        ] {
            assert!(read(&serde_json::to_vec(&value).unwrap()).is_err());
        }
        for raw in [
            format!(
                r#"{{"profile":"{profile}","profile":"{profile}","scope":{{"kind":"global"}},"fact":"{fact}"}}"#
            ),
            format!(
                r#"{{"profile":"{profile}","scope":{{"kind":"global","kind":"global"}},"fact":"{fact}"}}"#
            ),
            format!(
                r#"{{"profile":"{profile}","scope":{{"kind":"document","document":"{fact}","document":"{fact}"}},"fact":"{fact}"}}"#
            ),
            format!(
                r#"{{"profile":"{profile}","scope":{{"kind":"global"}},"fact":"{fact}","fact":"{fact}"}}"#
            ),
        ] {
            assert!(read(raw.as_bytes()).is_err(), "duplicate accepted");
        }
        // Retain the existing public/file Key decoder behavior, not just valid parity.
        let legacy = serde_json::json!({"profile":profile,"scope":{"kind":"global","unknown":1},"fact":fact});
        assert!(serde_json::from_value::<Key>(legacy).is_ok());
    }
    #[test]
    fn report_holds_ordinary_mode_through_actual_output_then_releases() {
        use storage::sync::mode::SyncModeChoice;
        let root = std::env::temp_dir().join(format!("memory-mode-output-{}", Uuid::new_v4()));
        let config = Config {
            paths: storage::StorePaths {
                data: root.join("data"),
                cache: root.join("cache"),
                credentials: root.join("credentials"),
            },
            memory: super::super::Configuration {
                profile: Some(Uuid::new_v4()),
                ..Default::default()
            },
            ..Default::default()
        };
        let store = Arc::new(Store::open(config.paths.clone()).unwrap());
        let mut report = execute(
            Operation::Query,
            Input::Query(Query::new("gravity".into(), 1)),
            &config,
            store.clone(),
        );
        assert_eq!(report.status, ReportStatus::Ok);
        // Isolate mode lifetime from the independent legacy-data provider lease.
        drop(report.provider_guard.take());
        struct DuringWrite {
            store: Arc<Store>,
            calls: usize,
        }
        impl std::io::Write for DuringWrite {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.calls += 1;
                assert!(self
                    .store
                    .set_sync_mode(
                        &self.store.sync_mode_snapshot().unwrap().token,
                        Uuid::new_v4(),
                        SyncModeChoice::LocalOnly
                    )
                    .is_err());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = DuringWrite {
            store: store.clone(),
            calls: 0,
        };
        report.write_to(&mut writer).unwrap();
        assert_eq!(writer.calls, 1);
        drop(report);
        store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                SyncModeChoice::LocalOnly,
            )
            .unwrap();
        drop(writer);
        drop(store);
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn mode_refusal_precedes_domain_mutation() {
        use storage::sync::mode::SyncModeChoice;
        let root = std::env::temp_dir().join(format!("memory-mode-refusal-{}", Uuid::new_v4()));
        let profile = Uuid::new_v4();
        let mut config = Config {
            paths: storage::StorePaths {
                data: root.join("data"),
                cache: root.join("cache"),
                credentials: root.join("credentials"),
            },
            memory: super::super::Configuration {
                profile: Some(profile),
                ..Default::default()
            },
            ..Default::default()
        };
        let store = Arc::new(Store::open(config.paths.clone()).unwrap());
        let binding = storage::digest(b"isolated");
        store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                SyncModeChoice::IsolatedBackup {
                    binding_sha256: binding.clone(),
                },
            )
            .unwrap();
        assert_eq!(
            execute(
                Operation::Query,
                Input::Query(Query::new("gravity".into(), 1)),
                &config,
                store.clone()
            )
            .status,
            ReportStatus::Unsupported
        );
        config.memory.selection = Some(super::super::catalog::Identity {
            group: Uuid::new_v4(),
            profile,
            binding_sha256: binding.clone(),
        });
        assert_eq!(
            execute(
                Operation::Query,
                Input::Query(Query::new("gravity".into(), 1)),
                &config,
                store.clone()
            )
            .status,
            ReportStatus::Unsupported
        );
        store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                SyncModeChoice::SharedGroup {
                    group: Uuid::new_v4(),
                    binding_sha256: binding,
                    root_id: "foreign-fixture".into(),
                },
            )
            .unwrap();
        assert_eq!(
            execute(
                Operation::Query,
                Input::Query(Query::new("gravity".into(), 1)),
                &config,
                store.clone()
            )
            .status,
            ReportStatus::Unsupported
        );
        let before = store.manifests().unwrap();
        assert_eq!(
            execute(
                Operation::InitializeSelection,
                Input::InitializeSelection(super::super::selected::InitializeSelection {
                    operation: Uuid::new_v4(),
                    accepted_base_sha256: storage::digest(b"valid refused initialization"),
                    registrations: vec![],
                }),
                &config,
                store.clone()
            )
            .status,
            ReportStatus::Unsupported
        );
        assert_eq!(store.manifests().unwrap().len(), before.len());
        drop(store);
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stdin_read_error_and_mutation_refusal_do_not_admit_store() {
        struct MustNotRead;
        impl Read for MustNotRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                panic!("unsupported stdin operation read input")
            }
        }
        for operation in [
            Operation::Put,
            Operation::Forget,
            Operation::InitializeAggregate,
            Operation::InitializeSelection,
            Operation::RegisterSelection,
        ] {
            assert_eq!(
                run_stdin(operation, &mut MustNotRead, None).status,
                ReportStatus::Unsupported
            );
        }
        struct FailedRead;
        impl Read for FailedRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("PRIVATE_STDIN_READ_ERROR"))
            }
        }
        let root = std::env::temp_dir().join(format!("memory-stdin-error-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let paths = storage::StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        };
        let config = Config {
            paths: paths.clone(),
            memory: super::super::Configuration {
                profile: Some(Uuid::new_v4()),
                ..Default::default()
            },
            ..Default::default()
        };
        let config_path = root.join("config.json");
        std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        for operation in [Operation::Query, Operation::Inspect] {
            let report = run_stdin(operation, &mut FailedRead, Some(config_path.clone()));
            assert_eq!(report.status, ReportStatus::InvalidInput);
            assert!(!String::from_utf8(report.encoded().unwrap())
                .unwrap()
                .contains("PRIVATE_STDIN_READ_ERROR"));
        }
        assert!(!paths.data.exists() && !paths.cache.exists() && !paths.credentials.exists());
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn current_query_inspection_and_empty_report_are_guarded_at_the_actual_writer() {
        use crate::memory::{self, Provider};
        struct PrefixFailure(Vec<u8>, usize);
        impl std::io::Write for PrefixFailure {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.1 += 1;
                if self.1 == 1 {
                    self.0.extend_from_slice(&bytes[..7]);
                    Ok(7)
                } else if self.1 == 2 {
                    Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
                } else {
                    self.0.extend_from_slice(bytes);
                    Ok(bytes.len())
                }
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for mode in 0..8 {
            let root = std::env::temp_dir().join(format!("memory-handoff-{}", Uuid::new_v4()));
            let paths = storage::StorePaths {
                data: root.join("data"),
                cache: root.join("cache"),
                credentials: root.join("credentials"),
            };
            let store = Arc::new(Store::open(paths.clone()).unwrap());
            let profile = Uuid::new_v4();
            let identity = memory::catalog::Identity {
                group: Uuid::new_v4(),
                profile,
                binding_sha256: storage::digest(b"local fixture"),
            };
            store
                .set_sync_mode(
                    &store.sync_mode_snapshot().unwrap().token,
                    Uuid::new_v4(),
                    storage::sync::mode::SyncModeChoice::SharedGroup {
                        group: identity.group,
                        binding_sha256: identity.binding_sha256.clone(),
                        root_id: "host-memory-fixture".into(),
                    },
                )
                .unwrap();
            let memory =
                memory::selected::SelectedMemory::new(store.clone(), identity.clone()).unwrap();
            let fact = memory::Fact {
                key: Key {
                    profile,
                    scope: memory::Scope::Global,
                    fact: Uuid::new_v4(),
                },
                claim: "gravity units".into(),
                value: "private-original-gravity-value".into(),
                subject: "gravity".into(),
                tags: vec![],
                provenance: memory::Provenance {
                    basis: memory::Basis::UserConfirmed,
                    description: "Explicit fixture".into(),
                    conversation: None,
                    turn: None,
                    source: None,
                },
                confidence_basis_points: 8000,
                observed_ms: 1,
                expires_ms: None,
            };
            let registration = memory
                .initialize_aggregate(memory::selected::InitializeAggregate {
                    operation: Uuid::new_v4(),
                    accepted_base_sha256: storage::digest(b"aggregate"),
                    fact: fact.clone(),
                })
                .unwrap()
                .registration
                .unwrap();
            memory
                .initialize_selection(memory::selected::InitializeSelection {
                    operation: Uuid::new_v4(),
                    accepted_base_sha256: storage::digest(b"catalog"),
                    registrations: if mode == 2 {
                        vec![]
                    } else {
                        vec![registration.clone()]
                    },
                })
                .unwrap();
            let config = Config {
                paths: paths.clone(),
                memory: memory::Configuration {
                    enabled: false,
                    profile: Some(profile),
                    selection: Some(identity),
                },
                ..Config::default()
            };
            let (operation, input) = if mode == 1 || mode >= 5 {
                let input = if mode >= 5 {
                    Input::read_stdin(
                        Operation::Inspect,
                        &mut serde_json::to_vec(&fact.key).unwrap().as_slice(),
                        profile,
                        config.memory.selection.as_ref(),
                    )
                    .unwrap()
                } else {
                    Input::Inspect(fact.key.clone())
                };
                (Operation::Inspect, input)
            } else {
                (
                    Operation::Query,
                    Input::Query(Query::new("gravity".into(), 10)),
                )
            };
            let mut report = execute(operation, input, &config, store.clone());
            assert!(report.pending.is_some());
            assert!(store
                .set_sync_mode(
                    &store.sync_mode_snapshot().unwrap().token,
                    Uuid::new_v4(),
                    storage::sync::mode::SyncModeChoice::LocalOnly
                )
                .is_err());
            if mode == 2 {
                memory
                    .register_selection(memory::selected::RegisterSelection {
                        operation: Uuid::new_v4(),
                        registration,
                    })
                    .unwrap();
            } else if mode == 3 || mode == 7 {
                report.data = Some(serde_json::json!({"forged":"private-forged-value"}));
            } else if mode != 4 && mode != 5 {
                let expected = memory
                    .inspect(&fact.key)
                    .unwrap()
                    .into_iter()
                    .map(|e| e.revision_id)
                    .collect();
                let mut changed = fact.clone();
                changed.value = "corrected gravity value".into();
                memory
                    .put(memory::Mutation {
                        operation: Uuid::new_v4(),
                        expected,
                        fact: changed,
                    })
                    .unwrap();
            }
            let mut output = Vec::new();
            if mode == 4 || mode == 5 {
                let expected = report.encoded().unwrap();
                let mut writer = PrefixFailure(Vec::new(), 0);
                assert!(report.write_to(&mut writer).is_err());
                assert_eq!(writer.0, &expected[..7]);
                assert_eq!(writer.1, 2);
            } else {
                report.write_to(&mut output).unwrap();
                let printed: serde_json::Value = serde_json::from_slice(&output).unwrap();
                assert_eq!(printed["status"], "unavailable");
                assert!(printed["data"].is_null());
                assert!(!String::from_utf8(output).unwrap().contains("private-"));
            }
            drop(report);
            // Both successful refusal output and a partial I/O error release the
            // mode lifetime only when the report is dropped by its caller.
            store
                .set_sync_mode(
                    &store.sync_mode_snapshot().unwrap().token,
                    Uuid::new_v4(),
                    storage::sync::mode::SyncModeChoice::LocalOnly,
                )
                .unwrap();
            drop(memory);
            drop(store);
            assert!(root.starts_with(std::env::temp_dir()));
            let _ = fs::remove_dir_all(root);
        }
    }
    #[test]
    fn actual_reacquire_refusal_and_exact_error_classifier_never_match_substrings() {
        let root = std::env::temp_dir().join(format!("memory-lock-classifier-{}", Uuid::new_v4()));
        let paths = storage::StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        };
        let held = Store::open(paths.clone()).unwrap();
        let error = match Store::open(paths) {
            Err(error) => error,
            Ok(_) => panic!("exclusive held Store must refuse"),
        };
        assert_eq!(store_open_status(&error), ReportStatus::Busy);
        for text in [
            "private parser value: store is owned by another process",
            "store is owned by another process: private suffix",
            "unsupported locking",
        ] {
            assert_eq!(
                store_open_status(&anyhow::anyhow!(text.to_owned())),
                ReportStatus::Unavailable
            );
        }
        drop(held);
        assert!(root.starts_with(std::env::temp_dir()));
        let _ = fs::remove_dir_all(root);
    }
}

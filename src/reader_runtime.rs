//! Executable selected metadata and explicit unsupported startup. No native capability.
use crate::{
    config::Config,
    conversation::{
        PendingIntentRequest, PendingRequestConflict, SelectedAdmission, SelectedDomainProjection,
    },
    storage::{
        digest, files,
        selection::{SelectedReadBudget, SelectionScope, SelectionToken},
        sync::mode::{OrdinaryModeLease, SyncMode, SyncModeLease, SyncModeSnapshot},
        Store, Uuid, MAX_ITEMS, MAX_METADATA, MAX_RECORD,
    },
    Workflow,
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Configuration {
    pub selection: Option<SelectionScope>,
}
impl Configuration {
    pub fn validate(&self) -> Result<()> {
        if let Some(scope) = &self.selection {
            scope.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Operation {
    Inspect,
    Context,
    RecoverPending,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::Context => "context",
            Self::RecoverPending => "recover-pending",
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Complete,
    Historical,
    NotFound,
    ChoiceNeeded,
    Unsupported,
    Busy,
    Unavailable,
    InvalidInput,
}
#[derive(Serialize)]
pub struct Report {
    report_version: u32,
    operation: &'static str,
    status: Status,
    incomplete: bool,
    native_authority: bool,
    pub data: Option<serde_json::Value>,
    #[serde(skip)]
    pending: Option<Pending>,
    #[serde(skip)]
    ordinary_startup_error: Option<&'static str>,
}
struct Pending {
    admission: SelectedAdmission,
    token: SelectionToken,
    raw_head: String,
    budget: SelectedReadBudget,
    sha256: String,
    consumed: bool,
    mode_lease: Option<SyncModeLease>,
}
impl Report {
    fn new(operation: &'static str, status: Status) -> Self {
        Self {
            report_version: 1,
            operation,
            status,
            incomplete: matches!(status, Status::Unavailable),
            native_authority: false,
            data: None,
            pending: None,
            ordinary_startup_error: None,
        }
    }
    /// Actual ordinary startup is a failing process, not a successful report command.
    pub fn into_ordinary_startup_error(self) -> anyhow::Error {
        anyhow::anyhow!(self.ordinary_startup_error.unwrap_or(match self.status {
            Status::Busy => "ordinary Reader startup busy",
            Status::Unsupported => "ordinary Reader startup unsupported",
            _ => "ordinary Reader startup unavailable",
        }))
    }
    fn encoded(&self) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        if bytes.len() <= MAX_METADATA + 64 * 1024 {
            return Ok(bytes);
        }
        let mut refused = Self::new(self.operation, Status::Unavailable);
        refused.incomplete = true;
        let mut bytes = serde_json::to_vec(&refused)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
    pub fn write_to(&mut self, writer: &mut impl std::io::Write) -> Result<()> {
        let encoded = self.encoded()?;
        let payload = digest(&serde_json::to_vec(self)?);
        let Some(pending) = self.pending.as_mut() else {
            return Ok(writer.write_all(&encoded)?);
        };
        let already_consumed = pending.consumed;
        pending.consumed = true;
        let mut attempted = false;
        let result = if already_consumed || payload != pending.sha256 {
            Err(anyhow::anyhow!("reader report changed"))
        } else {
            SelectedAdmission::with_gates(&[&pending.admission], |store| {
                if let Some(lease) = &pending.mode_lease {
                    lease.require_shared_scope(pending.token.scope())?;
                }
                ensure!(
                    store.unavailable_commits()? == 0,
                    "reader store unavailable"
                );
                let read = store
                    .selected_snapshot_with_budget(
                        pending.token.scope(),
                        MAX_ITEMS,
                        &mut pending.budget,
                    )?
                    .context("reader selection absent")?;
                ensure!(
                    read.snapshot.token == pending.token
                        && read.accepted_history[0].sha256 == pending.raw_head,
                    "reader selection replaced"
                );
                attempted = true;
                Ok(writer.write_all(&encoded)?)
            })
        };
        if let Err(error) = result {
            if attempted {
                return Err(error);
            }
            writer.write_all(&Self::new(self.operation, Status::Unavailable).encoded()?)?;
        }
        Ok(())
    }
}
fn allowance() -> SelectedReadBudget {
    SelectedReadBudget::new(64, 4096, 8 * 1024 * 1024, 32 * 1024 * 1024)
}
fn admission_status(status: crate::memory::maintenance::ReportStatus) -> Status {
    use crate::memory::maintenance::ReportStatus as M;
    match status {
        M::Busy => Status::Busy,
        M::Unsupported => Status::Unsupported,
        _ => Status::Unavailable,
    }
}
fn open(config: &Config) -> std::result::Result<Arc<Store>, Status> {
    crate::memory::maintenance::preflight(config, false).map_err(admission_status)?;
    Store::open(config.paths.clone())
        .map(Arc::new)
        .map_err(|error| admission_status(crate::memory::maintenance::store_open_status(&error)))
}

pub enum StartupDecision {
    Ordinary(Option<Arc<Store>>),
    Report(Box<Report>),
}
fn fresh_auxiliary_roots(config: &Config) -> Result<()> {
    for root in [&config.paths.cache, &config.paths.credentials] {
        files::safe_path(root)?;
        match std::fs::symlink_metadata(root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
            Ok(metadata) => {
                ensure!(metadata.is_dir(), "unsupported auxiliary root shape");
                ensure!(
                    std::fs::read_dir(root)?.next().is_none(),
                    "nonempty auxiliary startup root"
                );
            }
        }
    }
    Ok(())
}
impl StartupDecision {
    fn refused(status: Status) -> Self {
        Self::Report(Box::new(Report::new("startup", status)))
    }
}
enum OrdinaryModeRefusal {
    Unavailable,
    Nonordinary,
    LegacySync,
}
impl OrdinaryModeRefusal {
    fn report(self) -> Report {
        let (status, reason) = match self {
            Self::Unavailable => (Status::Unavailable, "ordinary Reader startup unavailable"),
            Self::Nonordinary => (
                Status::Unsupported,
                "ordinary mode requires absent or LocalOnly choice",
            ),
            Self::LegacySync => (
                Status::Unsupported,
                "explicit local mode requires legacy sync disabled",
            ),
        };
        let mut report = Report::new("startup", status);
        report.ordinary_startup_error = Some(reason);
        report
    }
}
fn ordinary_mode(store: &Store, config: &Config) -> std::result::Result<(), OrdinaryModeRefusal> {
    ordinary_mode_snapshot(store, config).map(|_| ())
}
fn ordinary_mode_snapshot(
    store: &Store,
    config: &Config,
) -> std::result::Result<SyncModeSnapshot, OrdinaryModeRefusal> {
    let mode = store
        .sync_mode_snapshot()
        .map_err(|_| OrdinaryModeRefusal::Unavailable)?;
    match &mode.mode {
        None => Ok(mode),
        Some(SyncMode::LocalOnly) if !config.sync.enabled => Ok(mode),
        Some(SyncMode::LocalOnly) => Err(OrdinaryModeRefusal::LegacySync),
        _ => Err(OrdinaryModeRefusal::Nonordinary),
    }
}
/// Consume the startup-admitted Store once. Fresh ordinary roots are created
/// only after credential validation. Keep the returned mode guard alive through
/// every ordinary consumer; the Arc alone does not pin the admitted mode.
pub fn ordinary_store(
    config: &Config,
    admitted: Option<Arc<Store>>,
) -> std::result::Result<(Arc<Store>, OrdinaryModeLease), Box<Report>> {
    let store = match admitted {
        Some(store) => store,
        None => {
            use crate::memory::maintenance::{preflight_state, StoreReadiness};
            let state = preflight_state(config, true)
                .map_err(|status| Box::new(Report::new("startup", admission_status(status))))?;
            if matches!(state, StoreReadiness::Fresh) {
                fresh_auxiliary_roots(config)
                    .map_err(|_| Box::new(Report::new("startup", Status::Unavailable)))?;
            }
            Arc::new(Store::open(config.paths.clone()).map_err(|error| {
                Box::new(Report::new(
                    "startup",
                    admission_status(crate::memory::maintenance::store_open_status(&error)),
                ))
            })?)
        }
    };
    let mode =
        ordinary_mode_snapshot(&store, config).map_err(|refusal| Box::new(refusal.report()))?;
    // Do not refresh a stale token into a different mode after admission.
    let lease = store
        .ordinary_mode_lease(&mode.token)
        .map_err(|_| Box::new(Report::new("startup", Status::Unavailable)))?;
    Ok((store, lease))
}
fn selected_mode(
    store: &Arc<Store>,
    scope: &SelectionScope,
) -> std::result::Result<Option<SyncModeLease>, Status> {
    let mode = store
        .sync_mode_snapshot()
        .map_err(|_| Status::Unavailable)?;
    match &mode.mode {
        None => Ok(None),
        Some(SyncMode::SharedGroup {
            group,
            binding_sha256,
            ..
        }) if *group == scope.group && binding_sha256 == &scope.binding_sha256 => {
            let lease = store
                .sync_mode_lease(&mode.token)
                .map_err(|_| Status::Unavailable)?;
            lease
                .require_shared_scope(scope)
                .map_err(|_| Status::Unsupported)?;
            Ok(Some(lease))
        }
        _ => Err(Status::Unsupported),
    }
}

/// Called from actual startup after final dotenv/config selection, before secrets
/// or RealDevice/provider construction. No native query can occur in this platform.
pub fn startup(config: &Config) -> StartupDecision {
    if config.reader.selection.is_none() {
        use crate::memory::maintenance::{preflight_state, StoreReadiness};
        match preflight_state(config, true) {
            Ok(StoreReadiness::Fresh) => {
                return if fresh_auxiliary_roots(config).is_ok() {
                    StartupDecision::Ordinary(None)
                } else {
                    StartupDecision::refused(Status::Unavailable)
                };
            }
            Ok(StoreReadiness::Existing) => {}
            Err(status) => return StartupDecision::refused(admission_status(status)),
        }
        let store = match open(config) {
            Ok(store) => store,
            Err(status) => return StartupDecision::refused(status),
        };
        return match ordinary_mode(&store, config) {
            Ok(()) => StartupDecision::Ordinary(Some(store)),
            Err(refusal) => StartupDecision::Report(Box::new(refusal.report())),
        };
    }
    let scope = config.reader.selection.as_ref().expect("selected branch");
    let mut report = Report::new("startup", Status::Unavailable);
    let store = match open(config) {
        Ok(store) => store,
        Err(status) => {
            report.status = status;
            return StartupDecision::Report(Box::new(report));
        }
    };
    let _mode_lease = match selected_mode(&store, scope) {
        Ok(lease) => lease,
        Err(status) => return StartupDecision::refused(status),
    };
    let admission = match SelectedAdmission::new(store.clone(), scope.clone()) {
        Ok(value) => Arc::new(value),
        Err(_) => return StartupDecision::Report(Box::new(report)),
    };
    let mut budget = allowance();
    let validated = SelectedAdmission::with_gates(&[admission.as_ref()], |store| {
        ensure!(
            store.unavailable_commits()? == 0,
            "reader store unavailable"
        );
        let read = store
            .selected_snapshot_with_budget(scope, MAX_ITEMS, &mut budget)?
            .context("reader selection absent")?;
        let projection = SelectedDomainProjection::from_snapshot(&read.snapshot)?;
        for conversation in projection.document_ownership()?.keys() {
            projection.inspect(*conversation)?;
        }
        Ok(())
    });
    if validated.is_err() {
        return StartupDecision::Report(Box::new(report));
    }
    let workflow = Workflow::selected_runtime(admission, config.debug_dump.unwrap_or(false));
    // Startup has no active request. Query availability without pretending to
    // acquire evidence or treating a request-guard failure as unsupported capture.
    if !matches!(
        workflow.reader_acquisition_kind(),
        crate::device::backend::AcquisitionKind::Unsupported
    ) {
        return StartupDecision::Report(Box::new(report));
    }
    use remarkable_open_sdk::{ObservationFailure, Platform, UnsupportedReason};
    let platform = remarkable_open_sdk::UnqualifiedPlatform;
    if let Err(ObservationFailure::Unsupported(UnsupportedReason::NoNativeAdapter)) =
        platform.observe_page()
    {
        report.status = Status::Unsupported;
        report.incomplete = false;
        report.data = Some(serde_json::json!({"reason":"no-native-adapter"}));
    }
    StartupDecision::Report(Box::new(report))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectInput {
    conversation: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextInput {
    conversation: Uuid,
    range: Option<Range>,
    budget: Budget,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Range {
    start: u64,
    end_exclusive: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Budget {
    max_text_bytes: usize,
    max_turns: usize,
    provider_token_limit: Option<usize>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoverInput {
    request: PendingIntentRequest,
}
enum Input {
    Inspect(Uuid),
    Context(Uuid),
    Recover(Box<PendingIntentRequest>),
}
fn input(operation: Operation, path: &Path, scope: &SelectionScope) -> Result<Input> {
    match operation {
        Operation::Inspect => {
            let i: InspectInput = files::json(path, MAX_RECORD as u64)?;
            ensure!(!i.conversation.is_nil(), "nil conversation");
            Ok(Input::Inspect(i.conversation))
        }
        Operation::Context => {
            let i: ContextInput = files::json(path, MAX_RECORD as u64)?;
            ensure!(
                !i.conversation.is_nil()
                    && (1..=MAX_ITEMS).contains(&i.budget.max_turns)
                    && (1..=MAX_METADATA).contains(&i.budget.max_text_bytes)
                    && i.budget.provider_token_limit.is_none_or(|v| v > 0),
                "invalid context budget"
            );
            if let Some(range) = i.range {
                ensure!(range.start < range.end_exclusive, "invalid range");
            }
            Ok(Input::Context(i.conversation))
        }
        Operation::RecoverPending => {
            let i: RecoverInput = files::json(path, MAX_RECORD as u64)?;
            let r = &i.request;
            r.validate_input()?;
            ensure!(
                r.selection.group == scope.group
                    && r.selection.key_sha256 == scope.key_sha256
                    && r.selection.binding_sha256 == scope.binding_sha256,
                "foreign recovery scope"
            );
            Ok(Input::Recover(Box::new(i.request)))
        }
    }
}
pub fn run(operation: Operation, path: &Path, config_path: Option<PathBuf>) -> Report {
    let name = operation.name();
    if path == Path::new("-") {
        return Report::new(name, Status::InvalidInput);
    }
    let Some(config_path) = config_path else {
        return Report::new(name, Status::Unavailable);
    };
    let config = match Config::load(Some(config_path)) {
        Ok(config) => config,
        Err(_) => return Report::new(name, Status::Unavailable),
    };
    let Some(scope) = config.reader.selection.as_ref() else {
        return Report::new(name, Status::Unsupported);
    };
    let input = match input(operation, path, scope) {
        Ok(input) => input,
        Err(_) => return Report::new(name, Status::InvalidInput),
    };
    let store = match open(&config) {
        Ok(store) => store,
        Err(status) => return Report::new(name, status),
    };
    execute(name, input, scope, store)
}
fn execute(name: &'static str, input: Input, scope: &SelectionScope, store: Arc<Store>) -> Report {
    let mut report = Report::new(name, Status::Unavailable);
    let admission = match SelectedAdmission::new(store.clone(), scope.clone()) {
        Ok(value) => value,
        Err(_) => return report,
    };
    if let Input::Recover(request) = input {
        match admission.recover_pending(&request) {
            Ok(Some(history)) => {
                report.status = Status::Historical;
                report.incomplete = false;
                report.data = Some(serde_json::json!({
                    "transaction": history.transaction,
                    "receipt": history.receipt,
                    "receipt_reference": history.receipt_reference
                }));
            }
            Ok(None) => {
                report.status = Status::NotFound;
                report.incomplete = false;
            }
            Err(error) => {
                if error.downcast_ref::<PendingRequestConflict>().is_some() {
                    report.status = Status::InvalidInput;
                    report.incomplete = false;
                }
            }
        }
        return report;
    }
    let mode_lease = match selected_mode(&store, scope) {
        Ok(lease) => lease,
        Err(status) => return Report::new(name, status),
    };
    let conversation = match input {
        Input::Inspect(id) | Input::Context(id) => id,
        Input::Recover(_) => unreachable!(),
    };
    let mut budget = allowance();
    let read = match SelectedAdmission::with_gates(&[&admission], |store| {
        ensure!(
            store.unavailable_commits()? == 0,
            "reader store unavailable"
        );
        store
            .selected_snapshot_with_budget(scope, MAX_ITEMS, &mut budget)?
            .context("reader selection absent")
    }) {
        Ok(read) => read,
        Err(_) => return report,
    };
    let projection = match SelectedDomainProjection::from_snapshot(&read.snapshot) {
        Ok(p) => p,
        Err(_) => return report,
    };
    let records = match projection.inspect(conversation) {
        Ok(records) => records,
        Err(_) => return report,
    };
    if matches!(input, Input::Context(_)) {
        report.status = Status::ChoiceNeeded;
        report.incomplete = false;
    } else {
        report.status = Status::Complete;
        report.incomplete = false;
        report.data = Some(serde_json::json!({
        "records": records,
        "snapshot_sha256": read.accepted_history[0].sha256,
        "store_generation": read.snapshot.token.store_generation(),
        "aggregate_generation": read.snapshot.token.aggregate_generation()
        }));
    }
    let sha256 = match serde_json::to_vec(&report) {
        Ok(bytes) => digest(&bytes),
        Err(_) => return Report::new(name, Status::Unavailable),
    };
    report.pending = Some(Pending {
        admission,
        token: read.snapshot.token,
        raw_head: read.accepted_history[0].sha256.clone(),
        budget,
        sha256,
        consumed: false,
        mode_lease,
    });
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::Ledger;
    use crate::storage::selection::SelectionChange;
    use crate::storage::{Scope, StorePaths};
    use std::collections::BTreeMap;
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn selected() -> (Fixture, Arc<Store>, SelectionScope, Uuid) {
        let fixture =
            Fixture(std::env::temp_dir().join(format!("reader-runtime-writer-{}", Uuid::new_v4())));
        let store = Arc::new(
            Store::open(StorePaths {
                data: fixture.0.join("data"),
                cache: fixture.0.join("cache"),
                credentials: fixture.0.join("credentials"),
            })
            .unwrap(),
        );
        let conversation = Uuid::new_v4();
        Ledger::new(store.clone())
            .create(conversation, Uuid::new_v4(), 1)
            .unwrap();
        let mut manifest = store.manifests().unwrap().remove(0);
        manifest.scope = Scope::SelectedRecords;
        let scope = SelectionScope {
            group: Uuid::new_v4(),
            key_sha256: digest(b"current writer"),
            binding_sha256: digest(b"host binding"),
        };
        store
            .initialize_selected(
                &scope,
                SelectionChange {
                    operation: Uuid::new_v4(),
                    accepted_base_sha256: digest(b"base"),
                    selected: manifest,
                    retained: vec![],
                },
                BTreeMap::new(),
            )
            .unwrap();
        (fixture, store, scope, conversation)
    }
    #[test]
    fn actual_writer_refuses_replacement_exhaustion_and_changed_raw_custody() {
        for case in 0..4 {
            let (_fixture, store, scope, conversation) = selected();
            let mut report = execute(
                "inspect",
                Input::Inspect(conversation),
                &scope,
                store.clone(),
            );
            assert_eq!(report.status, Status::Complete);
            match case {
                1 => {
                    let snapshot = store.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
                    let mut manifest = snapshot.transaction.selected.clone();
                    manifest.transaction_id = Uuid::new_v4();
                    store
                        .activate_selected(
                            &snapshot.token,
                            SelectionChange {
                                operation: Uuid::new_v4(),
                                accepted_base_sha256: digest(b"new accepted history"),
                                selected: manifest,
                                retained: vec![],
                            },
                            BTreeMap::new(),
                        )
                        .unwrap();
                }
                2 => report.pending.as_mut().unwrap().budget = SelectedReadBudget::new(0, 0, 0, 0),
                3 => report.pending.as_mut().unwrap().raw_head = digest(b"wrong original custody"),
                _ => {}
            }
            let mut output = vec![];
            report.write_to(&mut output).unwrap();
            let output: serde_json::Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(
                output["status"],
                if case == 0 { "complete" } else { "unavailable" },
                "case {case}"
            );
            if case != 0 {
                assert!(output["data"].is_null());
            }
        }
    }
    #[test]
    fn local_startup_reuses_admitted_store_and_shared_context_retains_real_mode_lease() {
        use crate::storage::sync::mode::SyncModeChoice;
        let (fixture, store, scope, conversation) = selected();
        store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                SyncModeChoice::SharedGroup {
                    group: scope.group,
                    binding_sha256: scope.binding_sha256.clone(),
                    root_id: "chosen-root".into(),
                },
            )
            .unwrap();
        let mut context = execute(
            "context",
            Input::Context(conversation),
            &scope,
            store.clone(),
        );
        assert_eq!(context.status, Status::ChoiceNeeded);
        assert!(context.pending.is_some() && context.data.is_none());
        assert!(store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                SyncModeChoice::LocalOnly
            )
            .is_err());
        let mut output = vec![];
        context.write_to(&mut output).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output).unwrap()["status"],
            "choice-needed"
        );
        drop(context);
        store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                SyncModeChoice::LocalOnly,
            )
            .unwrap();
        let paths = store.paths.clone();
        drop(store);
        let config = Config {
            paths,
            ..Config::default()
        };
        let StartupDecision::Ordinary(Some(admitted)) = startup(&config) else {
            panic!("local-only should admit ordinary data")
        };
        let saved = admitted.clone();
        let (reused, ordinary_guard) = ordinary_store(&config, Some(admitted)).ok().unwrap();
        assert!(Arc::ptr_eq(&saved, &reused), "no second Store open");
        let mut contradictory = config.clone();
        contradictory.sync.enabled = true;
        assert!(ordinary_store(&contradictory, Some(reused.clone())).is_err());
        drop(ordinary_guard);
        drop(reused);
        drop(saved);
        drop(fixture);
    }
    #[test]
    fn ordinary_startup_retains_mode_fence_through_real_local_consumers() {
        use crate::storage::sync::mode::SyncModeChoice;
        for explicit_local in [false, true] {
            let fixture = Fixture(
                std::env::temp_dir().join(format!("reader-ordinary-mode-{}", Uuid::new_v4())),
            );
            let store = Arc::new(
                Store::open(StorePaths {
                    data: fixture.0.join("data"),
                    cache: fixture.0.join("cache"),
                    credentials: fixture.0.join("credentials"),
                })
                .unwrap(),
            );
            if explicit_local {
                store
                    .set_sync_mode(
                        &store.sync_mode_snapshot().unwrap().token,
                        Uuid::new_v4(),
                        SyncModeChoice::LocalOnly,
                    )
                    .unwrap();
            }
            let config = Config {
                paths: store.paths.clone(),
                ..Config::default()
            };
            drop(store);
            let StartupDecision::Ordinary(admitted) = startup(&config) else {
                panic!("ordinary startup should admit this local mode")
            };
            let (store, guard) = ordinary_store(&config, admitted).ok().unwrap();
            let peer = store.clone();
            let before = peer.sync_mode_snapshot().unwrap();
            let conversation = Uuid::new_v4();
            let ledger = Ledger::new(store);
            ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
            let choice = SyncModeChoice::SharedGroup {
                group: Uuid::new_v4(),
                binding_sha256: digest(b"ordinary lifetime test binding"),
                root_id: "owned-mode-root".into(),
            };
            assert!(peer
                .set_sync_mode(&before.token, Uuid::new_v4(), choice.clone())
                .is_err());
            assert!(peer.sync_mode_snapshot().unwrap().token == before.token);
            drop(ledger);
            assert!(
                peer.set_sync_mode(&before.token, Uuid::new_v4(), choice.clone())
                    .is_err(),
                "the returned guard, not a Ledger clone, pins mode"
            );
            drop(guard);
            peer.set_sync_mode(&before.token, Uuid::new_v4(), choice)
                .unwrap();
        }
    }
    #[test]
    fn deferred_fresh_creation_accepts_empty_auxiliary_roots_and_rechecks_changed_state() {
        for foreign_after_admission in [false, true] {
            let fresh = Fixture(
                std::env::temp_dir().join(format!("reader-deferred-aux-{}", Uuid::new_v4())),
            );
            let config = Config {
                paths: StorePaths {
                    data: fresh.0.join("data"),
                    cache: fresh.0.join("cache"),
                    credentials: fresh.0.join("credentials"),
                },
                ..Config::default()
            };
            std::fs::create_dir_all(&config.paths.cache).unwrap();
            std::fs::create_dir_all(&config.paths.credentials).unwrap();
            let StartupDecision::Ordinary(admitted) = startup(&config) else {
                panic!("safe empty auxiliaries")
            };
            assert!(admitted.is_none() && !config.paths.data.exists());
            if foreign_after_admission {
                std::fs::write(
                    config.paths.credentials.join("owner.json"),
                    b"foreign-owner-after-admission",
                )
                .unwrap();
                assert!(ordinary_store(&config, admitted).is_err());
                assert!(!config.paths.data.exists());
            } else {
                let (store, _ordinary_guard) = ordinary_store(&config, admitted).ok().unwrap();
                assert_eq!(store.paths.data, config.paths.data);
                assert!(store.sync_mode_snapshot().unwrap().mode.is_none());
            }
        }
    }
    #[test]
    fn typed_startup_availability_keeps_request_guards_separate() {
        let (_fixture, store, scope, _) = selected();
        let admission = Arc::new(SelectedAdmission::new(store, scope).unwrap());
        let mut workflow = Workflow::selected_runtime(admission, false);
        assert!(matches!(
            workflow.reader_acquisition_kind(),
            crate::device::backend::AcquisitionKind::Unsupported
        ));
        // No active request exists. Acquisition must continue to refuse at its
        // request guard; startup availability is not evidence or permission.
        let error = workflow.acquire_reader_evidence().err().unwrap();
        assert_eq!(
            error.to_string(),
            "Selected operation requires context dispatch"
        );
    }
    #[test]
    fn default_config_skips_selected_startup_and_whole_report_bound_is_fixed() {
        let fresh =
            Fixture(std::env::temp_dir().join(format!("reader-fresh-default-{}", Uuid::new_v4())));
        let config = Config {
            paths: StorePaths {
                data: fresh.0.join("data"),
                cache: fresh.0.join("cache"),
                credentials: fresh.0.join("credentials"),
            },
            ..Config::default()
        };
        assert!(matches!(startup(&config), StartupDecision::Ordinary(None)));
        assert!(!config.paths.data.exists());
        let mut report = Report::new("recover-pending", Status::Historical);
        report.data = Some(serde_json::Value::String(format!(
            "private-sentinel{}",
            "x".repeat(MAX_METADATA + 64 * 1024)
        )));
        let mut output = vec![];
        report.write_to(&mut output).unwrap();
        assert!(output.len() < 512);
        let output: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(output["status"], "unavailable");
        assert!(output["data"].is_null());
        assert_eq!(output["native_authority"], false);
    }
}

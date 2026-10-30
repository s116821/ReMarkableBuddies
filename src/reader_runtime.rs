//! Executable selected metadata and explicit unsupported startup. No native capability.
use crate::{
    config::Config,
    conversation::{
        PendingIntentRequest, PendingRequestConflict, SelectedAdmission, SelectedDomainProjection,
    },
    storage::{
        digest, files,
        selection::{SelectedReadBudget, SelectionScope, SelectionToken},
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
}
struct Pending {
    admission: SelectedAdmission,
    token: SelectionToken,
    raw_head: String,
    budget: SelectedReadBudget,
    sha256: String,
    consumed: bool,
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
        }
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

/// Called from actual startup after final dotenv/config selection, before secrets
/// or RealDevice/provider construction. No native query can occur in this platform.
pub fn startup(config: &Config) -> Option<Report> {
    let scope = config.reader.selection.as_ref()?;
    let mut report = Report::new("startup", Status::Unavailable);
    let store = match open(config) {
        Ok(store) => store,
        Err(status) => {
            report.status = status;
            return Some(report);
        }
    };
    let admission = match SelectedAdmission::new(store.clone(), scope.clone()) {
        Ok(value) => Arc::new(value),
        Err(_) => return Some(report),
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
        return Some(report);
    }
    let workflow = Workflow::selected_runtime(admission, config.debug_dump.unwrap_or(false));
    // Startup has no active request. Query availability without pretending to
    // acquire evidence or treating a request-guard failure as unsupported capture.
    if !matches!(
        workflow.reader_acquisition_kind(),
        crate::device::backend::AcquisitionKind::Unsupported
    ) {
        return Some(report);
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
    Some(report)
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
        return report;
    }
    report.status = Status::Complete;
    report.incomplete = false;
    report.data = Some(serde_json::json!({
        "records": records,
        "snapshot_sha256": read.accepted_history[0].sha256,
        "store_generation": read.snapshot.token.store_generation(),
        "aggregate_generation": read.snapshot.token.aggregate_generation()
    }));
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
        assert!(startup(&Config::default()).is_none());
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

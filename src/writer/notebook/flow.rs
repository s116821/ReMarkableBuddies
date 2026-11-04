//! Joined notebook generation/export through the existing owners and backend journal.
use super::*;
use crate::{
    conversation::{Ledger, Outcome, WriterExportInput, WriterFlowAdmission, WriterSelection},
    note_export::{
        self, ExportControl, ExportObservation, ExportOwner, ExportPhase, ExportReport,
        ExportStatus, NoteDestination,
    },
    writer_flow::{Phase, Status},
};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::sync::watch;

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub outer_operation: Uuid,
    pub correlation: Uuid,
    pub effect_operation: Uuid,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Failure {
    InvalidInput,
    MissingCredentials,
    Storage,
    Unsupported,
    NoFreshSelection,
    Deadline,
}
#[derive(Serialize)]
pub struct Report {
    pub status: Status,
    pub conversation: Uuid,
    pub outer_operation: Uuid,
    pub correlation: Uuid,
    pub effect_operation: Uuid,
    pub plan: Uuid,
    pub archive: Uuid,
    pub draft: Option<Uuid>,
    pub generation: Option<runner::Report>,
    pub export: Option<ExportReport>,
    pub observation: ExportObservation,
    pub trace: Vec<Phase>,
    pub failure: Option<Failure>,
}
impl Report {
    pub fn refused(
        conversation: Uuid,
        identity: Identity,
        plan: Uuid,
        archive: Uuid,
        failure: Failure,
    ) -> Self {
        Self {
            status: Status::Refused,
            conversation,
            outer_operation: identity.outer_operation,
            correlation: identity.correlation,
            effect_operation: identity.effect_operation,
            plan,
            archive,
            draft: None,
            generation: None,
            export: None,
            observation: ExportObservation::default(),
            trace: vec![],
            failure: Some(failure),
        }
    }
}
pub fn run<E: crate::LLMEngine>(
    ledger: &Ledger,
    engine: &mut E,
    destination: &impl NoteDestination,
    input: runner::Input,
    identity: Identity,
    canceled: watch::Receiver<bool>,
) -> Report {
    run_with_events(
        ledger,
        engine,
        destination,
        input,
        identity,
        canceled,
        &mut |_, _| {},
    )
}
pub(crate) fn preflight(input: &runner::Input, identity: Identity) -> Result<()> {
    let plan = &input.admission.plan;
    ensure!(
        input.admission.plan == input.payloads.plan,
        "notebook flow plan mismatch"
    );
    let draft = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("urn:rmb:writer:v1:{}:draft", plan.operation).as_bytes(),
    );
    let ids = [
        identity.outer_operation,
        identity.correlation,
        identity.effect_operation,
        plan.operation,
        plan.conversation,
        draft,
    ];
    ensure!(
        ids.iter().all(|id| !id.is_nil())
            && ids
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == ids.len(),
        "notebook flow identities overlap"
    );
    let calls = plan.chunks.len() + 1;
    let history = input.admission.guards.len();
    ensure!(
        (2..=9).contains(&calls) && history <= 64,
        "invalid notebook flow reservation"
    );
    let model = 11 + 2 * history + 12 * calls + 2 * history * calls + calls * calls;
    ensure!(
        model + 4 + history + calls + 14 <= 256 && 7 + history + 3 * calls + 3 <= 64,
        "joined notebook publication reservation exceeded"
    );
    Ok(())
}
fn is_canceled(receiver: &watch::Receiver<bool>) -> bool {
    let value = *receiver.borrow();
    value || receiver.has_changed().is_err()
}
pub(crate) fn run_with_events<E: crate::LLMEngine>(
    ledger: &Ledger,
    engine: &mut E,
    destination: &impl NoteDestination,
    mut input: runner::Input,
    identity: Identity,
    canceled: watch::Receiver<bool>,
    events: &mut crate::writer_flow::Events<'_>,
) -> Report {
    let plan = input.admission.plan.clone();
    let mut report = Report::refused(
        plan.conversation,
        identity,
        plan.id,
        plan.archive_id,
        Failure::Storage,
    );
    if preflight(&input, identity).is_err() {
        report.failure = Some(Failure::InvalidInput);
        return report;
    }
    let result = (|| -> Result<()> {
        let Some(writer_owner) = super::super::Owner::acquire(ledger.store()) else {
            report.status = Status::Busy;
            report.failure = None;
            return Ok(());
        };
        let Some(export_owner) = ExportOwner::acquire(ledger) else {
            report.status = Status::Busy;
            report.failure = None;
            return Ok(());
        };
        let started = Instant::now();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        if is_canceled(&canceled) {
            report.status = Status::Canceled;
            report.failure = None;
            return Ok(());
        }
        let source = &input.admission.archive;
        let scope = format!(
            "normalized-document:{}:{}",
            source.document,
            match source.selection {
                TextScope::Page { page } => format!("page:{page}"),
                TextScope::SuppliedNotebook => "supplied-notebook".into(),
            }
        );
        if let Some(old) = ledger.export_association(plan.conversation, identity.correlation)? {
            if old.backend != destination.backend() || old.source_scope != scope {
                report.failure = Some(Failure::InvalidInput);
                return Ok(());
            }
            if old.outcome != Outcome::Completed
                && !old.attempt.as_ref().is_some_and(|attempt| {
                    attempt.no_send && matches!(old.outcome, Outcome::Failed | Outcome::Canceled)
                })
            {
                report.status = Status::ReconcileRequired;
                report.failure = None;
                return Ok(());
            }
        }
        let fingerprint = digest(&serde_json::to_vec(&(
            "writer-notebook-flow-intent-v1",
            identity,
            plan.id,
            &plan.core_digest,
            &plan.map_input_digests,
            &plan.reducer_prefix_digest,
            plan.reducer_reserved_bytes,
            destination.backend(),
        ))?);
        report.trace.push(Phase::BeforeAdmission);
        events(Phase::BeforeAdmission, ledger);
        let grant = match ledger.admit_writer_flow(WriterFlowAdmission {
            conversation: plan.conversation,
            operation: identity.outer_operation,
            fingerprint,
            correlation: identity.correlation,
            effect: identity.effect_operation,
            draft: Uuid::new_v5(
                &Uuid::NAMESPACE_URL,
                format!("urn:rmb:writer:v1:{}:draft", plan.operation).as_bytes(),
            ),
            expected: input.admission.expected.clone(),
        })? {
            Some(grant) => grant,
            None => {
                report.status = Status::Historical;
                report.failure = None;
                return Ok(());
            }
        };
        // Carry this invocation's exact post-intent root; never refresh unrelated history.
        input.admission.expected = grant.root.clone();
        report.trace.push(Phase::AfterAdmission);
        events(Phase::AfterAdmission, ledger);
        report.trace.push(Phase::BeforeModel);
        events(Phase::BeforeModel, ledger);
        let generated = runner::run_owned(
            ledger,
            engine,
            input,
            &mut || is_canceled(&canceled),
            &writer_owner,
            started,
        );
        report.draft = generated.report.draft;
        let status = generated.report.status;
        report.generation = Some(generated.report);
        report.trace.push(Phase::AfterModel);
        events(Phase::AfterModel, ledger);
        let Some(finalized) = generated.finalized else {
            report.status = match status {
                runner::Status::Canceled => Status::Canceled,
                runner::Status::Historical => Status::Interrupted,
                _ => Status::GenerationFailed,
            };
            report.failure = None;
            return Ok(());
        };
        if is_canceled(&canceled) {
            report.status = Status::Canceled;
            report.failure = None;
            return Ok(());
        }
        if started.elapsed() >= Duration::from_secs(600) {
            report.status = Status::Canceled;
            report.failure = Some(Failure::Deadline);
            return Ok(());
        }
        let selection = WriterSelection::published_notebook(ledger, finalized)?;
        let export_input = WriterExportInput::new(grant, selection);
        let note = export_input.note()?;
        let mut event = |phase, ledger: &Ledger| {
            let phase = match phase {
                ExportPhase::BeforeAdmission => Phase::BeforeExportAdmission,
                ExportPhase::AfterAdmission => Phase::AfterExportAdmission,
                ExportPhase::BeforeDestinationPoll => Phase::BeforeDestinationPoll,
                ExportPhase::BeforeWritePoll => Phase::BeforeWritePoll,
            };
            report.trace.push(phase);
            events(phase, ledger);
        };
        let mut control = ExportControl {
            input: Some(export_input),
            guard: None,
            observation: &mut report.observation,
            event: &mut event,
            deadline: Some(started + Duration::from_secs(600)),
        };
        let export = runtime.block_on(note_export::export_with_owner(
            ledger,
            destination,
            &note,
            canceled.clone(),
            &export_owner,
            Some(&mut control),
        ));
        report.status = match export.status {
            ExportStatus::VerifiedCurrent => Status::Exported,
            ExportStatus::HistoricalCompleted => Status::HistoricalExport,
            ExportStatus::CanceledNoEffect => Status::Canceled,
            ExportStatus::Busy => Status::Busy,
            ExportStatus::ReconcileRequired => Status::ReconcileRequired,
            _ => Status::Refused,
        };
        report.export = Some(export);
        report.failure = None;
        report.trace.push(Phase::Complete);
        events(Phase::Complete, ledger);
        Ok(())
    })();
    if result.is_err() {
        report.failure = Some(Failure::Storage);
    }
    report
}

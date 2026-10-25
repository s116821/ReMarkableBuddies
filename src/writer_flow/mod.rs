//! Deliberate host Writer/export orchestration. No device or native admission.
use crate::{
    config::PersonalizationSnapshot,
    conversation::{Ledger, Outcome, WriterExportInput, WriterFlowAdmission},
    note_export::{
        self, ExportControl, ExportObservation, ExportOwner, ExportPhase, ExportReport,
        ExportStatus, NoteDestination,
    },
    storage::{digest, Namespace, Uuid},
    writer, LLMEngine,
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    GenerateAndExport {
        outer_operation: Uuid,
        generation: writer::Request,
        correlation: Uuid,
        effect_operation: Uuid,
    },
    RefineAndExport {
        outer_operation: Uuid,
        refinement: writer::Request,
        correlation: Uuid,
        effect_operation: Uuid,
    },
    ExportDraft {
        outer_operation: Uuid,
        conversation: Uuid,
        draft: Uuid,
        correlation: Uuid,
        effect_operation: Uuid,
    },
    ReconcileExport {
        conversation: Uuid,
        correlation: Uuid,
    },
}
impl Request {
    pub fn model(&self) -> Option<&writer::Request> {
        match self {
            Self::GenerateAndExport { generation, .. } => Some(generation),
            Self::RefineAndExport { refinement, .. } => Some(refinement),
            _ => None,
        }
    }
    pub fn conversation(&self) -> Uuid {
        match self {
            Self::GenerateAndExport { generation, .. } => generation.conversation(),
            Self::RefineAndExport { refinement, .. } => refinement.conversation(),
            Self::ExportDraft { conversation, .. } | Self::ReconcileExport { conversation, .. } => {
                *conversation
            }
        }
    }
    pub fn identities(&self) -> (Uuid, Uuid, Uuid) {
        match self {
            Self::GenerateAndExport {
                outer_operation,
                correlation,
                effect_operation,
                ..
            }
            | Self::RefineAndExport {
                outer_operation,
                correlation,
                effect_operation,
                ..
            }
            | Self::ExportDraft {
                outer_operation,
                correlation,
                effect_operation,
                ..
            } => (*outer_operation, *correlation, *effect_operation),
            Self::ReconcileExport { correlation, .. } => (Uuid::nil(), *correlation, Uuid::nil()),
        }
    }
    fn draft(&self) -> Uuid {
        match self {
            Self::ExportDraft { draft, .. } => *draft,
            _ => self.model().map_or(Uuid::nil(), |request| {
                writer::stage(request.operation(), "draft")
            }),
        }
    }
    pub fn preflight(&self) -> Result<()> {
        match self {
            Self::GenerateAndExport { generation, .. } => ensure!(
                matches!(generation, writer::Request::Generate { .. }),
                "generation variant required"
            ),
            Self::RefineAndExport { refinement, .. } => ensure!(
                matches!(refinement, writer::Request::Refine { .. }),
                "refinement variant required"
            ),
            _ => {}
        }
        if let Some(model) = self.model() {
            ensure!(model.preflight().is_ok(), "invalid Writer request");
        }
        let (outer, correlation, effect) = self.identities();
        if matches!(self, Self::ReconcileExport { .. }) {
            ensure!(
                !self.conversation().is_nil()
                    && !correlation.is_nil()
                    && self.conversation() != correlation,
                "invalid reconciliation identity"
            );
            return Ok(());
        }
        let mut ids = vec![
            self.conversation(),
            outer,
            correlation,
            effect,
            self.draft(),
        ];
        if let Some(model) = self.model() {
            ids.push(model.operation());
        }
        ensure!(ids.iter().all(|id| !id.is_nil()), "nil flow identity");
        let mut distinct = ids.clone();
        distinct.sort();
        distinct.dedup();
        ensure!(distinct.len() == ids.len(), "flow identities collide");
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Exported,
    HistoricalExport,
    Historical,
    Interrupted,
    GenerationFailed,
    ReconcileRequired,
    Canceled,
    Busy,
    Refused,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Failure {
    InvalidInput,
    MissingCredentials,
    Storage,
    Unsupported,
    NoFreshSelection,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    BeforeAdmission,
    AfterAdmission,
    BeforeModel,
    AfterModel,
    BeforeSelection,
    BeforeExportAdmission,
    AfterExportAdmission,
    BeforeDestinationPoll,
    BeforeWritePoll,
    Complete,
}
#[derive(Serialize)]
pub struct Report {
    pub status: Status,
    pub conversation: Uuid,
    pub outer_operation: Uuid,
    pub correlation: Uuid,
    pub effect_operation: Uuid,
    pub draft: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<writer::Report>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export: Option<ExportReport>,
    pub observation: ExportObservation,
    pub trace: Vec<Phase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
}
impl Report {
    pub fn refused(request: &Request, failure: Failure) -> Self {
        let (outer, correlation, effect) = request.identities();
        Self {
            status: Status::Refused,
            conversation: request.conversation(),
            outer_operation: outer,
            correlation,
            effect_operation: effect,
            draft: None,
            generation: None,
            export: None,
            observation: ExportObservation::default(),
            trace: vec![],
            failure: Some(failure),
        }
    }
}
pub(crate) type Events<'a> = dyn FnMut(Phase, &Ledger) + Send + 'a;

/// Blocking host entrypoint; the command owns/joins it on its existing worker.
/// Caller cancellation must be monotonic. No model/destination is polled on replay.
pub fn run<E: LLMEngine>(
    ledger: &Ledger,
    engine: &mut E,
    destination: &impl NoteDestination,
    request: &Request,
    preference: &PersonalizationSnapshot,
    canceled: watch::Receiver<bool>,
) -> Report {
    run_with_events(
        ledger,
        engine,
        destination,
        request,
        preference,
        canceled,
        &mut |_, _| {},
    )
}
pub(crate) fn run_with_events<E: LLMEngine>(
    ledger: &Ledger,
    engine: &mut E,
    destination: &impl NoteDestination,
    request: &Request,
    preference: &PersonalizationSnapshot,
    canceled: watch::Receiver<bool>,
    events: &mut Events<'_>,
) -> Report {
    let mut result = Report::refused(request, Failure::Storage);
    if request.preflight().is_err() {
        result.failure = Some(Failure::InvalidInput);
        return result;
    }
    let Some(writer_owner) = writer::Owner::acquire(ledger.store()) else {
        result.status = Status::Busy;
        result.failure = None;
        return result;
    };
    let Some(export_owner) = ExportOwner::acquire(ledger) else {
        result.status = Status::Busy;
        result.failure = None;
        return result;
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            result.failure = Some(Failure::Unsupported);
            return result;
        }
    };
    result.failure = None;
    if *canceled.borrow() {
        result.status = Status::Canceled;
        return result;
    }
    if matches!(request, Request::ReconcileExport { .. }) {
        let export = runtime.block_on(note_export::reconcile_with_owner(
            ledger,
            destination,
            request.conversation(),
            result.correlation,
            canceled,
            &export_owner,
            Some(&mut result.observation),
        ));
        result.status = export_status(export.status);
        result.export = Some(export);
        return result;
    }
    let expected = match ledger.expected(Namespace::Conversation, request.conversation()) {
        Ok(expected) => expected,
        Err(_) => {
            result.failure = Some(Failure::Storage);
            return result;
        }
    };
    match ledger.export_association(request.conversation(), result.correlation) {
        Ok(Some(old))
            if old.outcome != Outcome::Completed
                && !old.attempt.as_ref().is_some_and(|a| {
                    a.no_send && matches!(old.outcome, Outcome::Failed | Outcome::Canceled)
                }) =>
        {
            result.status = Status::ReconcileRequired;
            return result;
        }
        Err(_) => {
            result.failure = Some(Failure::Storage);
            return result;
        }
        _ => {}
    }
    let fingerprint =
        match serde_json::to_vec(&("writer-flow-intent-v1", request, destination.backend())) {
            Ok(value) => digest(&value),
            Err(_) => {
                result.failure = Some(Failure::InvalidInput);
                return result;
            }
        };
    result.trace.push(Phase::BeforeAdmission);
    events(Phase::BeforeAdmission, ledger);
    if *canceled.borrow() {
        result.status = Status::Canceled;
        return result;
    }
    let grant = match ledger.admit_writer_flow(WriterFlowAdmission {
        conversation: request.conversation(),
        operation: result.outer_operation,
        fingerprint,
        correlation: result.correlation,
        effect: result.effect_operation,
        draft: request.draft(),
        expected,
    }) {
        Ok(Some(grant)) => grant,
        Ok(None) => {
            result.status = Status::Historical;
            if ledger
                .writer_turn(request.draft(), request.conversation())
                .is_ok_and(|t| t.outcome == Outcome::Generated)
            {
                result.draft = Some(request.draft());
            }
            return result;
        }
        Err(_) => {
            result.failure = Some(Failure::Storage);
            return result;
        }
    };
    result.trace.push(Phase::AfterAdmission);
    events(Phase::AfterAdmission, ledger);
    let selection = if let Some(model) = request.model() {
        result.trace.push(Phase::BeforeModel);
        events(Phase::BeforeModel, ledger);
        let mut produced = None;
        let generated = writer::run_owned(
            ledger,
            engine,
            model,
            preference,
            &mut || *canceled.borrow(),
            writer::OwnedRun {
                owner: &writer_owner,
                expected_root: Some(grant.root.clone()),
                produced: &mut produced,
            },
        );
        let status = generated.status;
        result.draft = generated.draft;
        result.generation = Some(generated);
        result.trace.push(Phase::AfterModel);
        events(Phase::AfterModel, ledger);
        if *canceled.borrow() {
            result.status = Status::Canceled;
            return result;
        }
        if status != writer::Status::Generated {
            result.status = match status {
                writer::Status::Canceled => Status::Canceled,
                writer::Status::Busy => Status::Busy,
                writer::Status::HistoricalGenerated | writer::Status::HistoricalPending => {
                    Status::Interrupted
                }
                _ => Status::GenerationFailed,
            };
            return result;
        }
        let Some(selection) = produced else {
            result.status = Status::Interrupted;
            result.failure = Some(Failure::NoFreshSelection);
            return result;
        };
        selection
    } else {
        result.trace.push(Phase::BeforeSelection);
        events(Phase::BeforeSelection, ledger);
        match ledger.select_writer_for_flow(
            request.conversation(),
            request.draft(),
            grant.root.clone(),
        ) {
            Ok(selection) => {
                result.draft = Some(request.draft());
                selection
            }
            Err(_) => {
                result.failure = Some(Failure::Storage);
                return result;
            }
        }
    };
    if *canceled.borrow() {
        result.status = Status::Canceled;
        return result;
    }
    let input = WriterExportInput::new(grant, selection);
    let note = match input.note() {
        Ok(note) => note,
        Err(_) => {
            result.failure = Some(Failure::Storage);
            return result;
        }
    };
    let mut export_event = |phase, ledger: &Ledger| {
        let phase = match phase {
            ExportPhase::BeforeAdmission => Phase::BeforeExportAdmission,
            ExportPhase::AfterAdmission => Phase::AfterExportAdmission,
            ExportPhase::BeforeDestinationPoll => Phase::BeforeDestinationPoll,
            ExportPhase::BeforeWritePoll => Phase::BeforeWritePoll,
        };
        result.trace.push(phase);
        events(phase, ledger);
    };
    let mut control = ExportControl {
        input: Some(input),
        guard: None,
        observation: &mut result.observation,
        event: &mut export_event,
    };
    let export = runtime.block_on(note_export::export_with_owner(
        ledger,
        destination,
        &note,
        canceled,
        &export_owner,
        Some(&mut control),
    ));
    result.status = export_status(export.status);
    result.export = Some(export);
    result.trace.push(Phase::Complete);
    events(Phase::Complete, ledger);
    result
}
fn export_status(status: ExportStatus) -> Status {
    match status {
        ExportStatus::VerifiedCurrent => Status::Exported,
        ExportStatus::HistoricalCompleted => Status::HistoricalExport,
        ExportStatus::CanceledNoEffect => Status::Canceled,
        ExportStatus::Busy => Status::Busy,
        ExportStatus::ReconcileRequired => Status::ReconcileRequired,
        _ => Status::Refused,
    }
}

#[cfg(test)]
mod tests;

// Typed native image/intent ingress remains an explicit integration gap.
#[allow(dead_code)]
pub(crate) mod handwriting;
use crate::{
    config::Config,
    conversation::{
        ContextBudget, ExpectedHeads, Ledger, Mode, Outcome, Role, SourceObservation, Turn,
        TurnRange,
    },
    device::interaction::Interaction,
    storage::{Namespace, Uuid},
    writer, LLMEngine,
};
use anyhow::{ensure, Result};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeRoute {
    OrdinaryLocal,
    ExistingReader,
}

impl RuntimeRoute {
    pub fn for_config(config: &Config) -> Self {
        if config.sync.enabled || config.reader.selection.is_some() {
            Self::ExistingReader
        } else {
            Self::OrdinaryLocal
        }
    }

    pub fn admits_startup(self, config: &Config) -> bool {
        config.reader_enabled() || (self == Self::OrdinaryLocal && config.admits_writer(false))
    }
}

pub struct Dispatch<'a> {
    config: &'a Config,
    store: Arc<crate::storage::Store>,
    _local: Arc<crate::mode_maintenance::LocalDispatchLifetime>,
    memory: Option<Arc<crate::memory::LocalMemory>>,
    input_events: std::collections::BTreeSet<Uuid>,
}

impl<'a> Dispatch<'a> {
    pub fn local(config: &'a Config, ledger: &Ledger) -> Result<Self> {
        ensure!(
            RuntimeRoute::for_config(config) == RuntimeRoute::OrdinaryLocal
                && config.memory.selection.is_none(),
            "unified dispatch requires ordinary local configuration"
        );
        config.memory.validate()?;
        let local = Arc::new(crate::mode_maintenance::admit_local_dispatch(
            ledger.store(),
        )?);
        let memory = if config.memory.enabled {
            Some(Arc::new(crate::memory::LocalMemory::admitted_local(
                ledger.store().clone(),
                config
                    .memory
                    .profile
                    .ok_or_else(|| anyhow::anyhow!("memory profile absent"))?,
                local.clone(),
            )?))
        } else {
            None
        };
        Ok(Self {
            config,
            store: ledger.store().clone(),
            _local: local,
            memory,
            input_events: Default::default(),
        })
    }

    pub(crate) fn handwriting_runtime(
        &self,
        ledger: &Ledger,
    ) -> Result<Option<crate::handwriting::runtime::Runtime>> {
        self.require_ledger(ledger)?;
        crate::handwriting::runtime::Runtime::new(
            self.config,
            self.store.clone(),
            self._local.clone(),
        )
    }

    pub fn local_memory(&self) -> Option<Arc<crate::memory::LocalMemory>> {
        self.memory.clone()
    }

    pub(super) fn require_ledger(&self, ledger: &Ledger) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&self.store, ledger.store()),
            "unified dispatch ledger changed"
        );
        Ok(())
    }

    /// This admits the existing capture trigger only. It does not supply page
    /// facts to typed follow-up or Writer operations.
    pub(crate) fn reader_trigger(&self, events: &[Interaction]) -> bool {
        self.admits(Mode::Reader, false)
            && matches!(
                events,
                [Interaction::Reader] | [Interaction::Invalidated, Interaction::Reader]
            )
    }

    pub(crate) fn admits(&self, receiver: Mode, external_export: bool) -> bool {
        match receiver {
            Mode::Reader => self.config.reader_enabled() && !external_export,
            Mode::Writer => self.config.admits_writer(external_export),
        }
    }

    pub(crate) fn blank_start(
        &mut self,
        ledger: &Ledger,
        admitted: &RequestScope,
        observe: &mut dyn FnMut() -> Result<SourceObservation>,
        open: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        self.require_ledger(ledger)?;
        ensure!(
            RuntimeRoute::OrdinaryLocal.admits_startup(self.config),
            "no enabled buddy"
        );
        admitted.check_source(observe)?;
        ensure!(
            admitted.root.0.len() <= 1
                && ledger.expected(Namespace::Conversation, admitted.conversation)?
                    == admitted.root,
            "conversation revision changed"
        );
        ensure!(
            self.input_events.len() < 4096 && self.input_events.insert(admitted.operation),
            "blank start event unavailable or already consumed"
        );
        open()
    }

    pub(crate) fn export_draft<E: LLMEngine>(
        &mut self,
        ledger: &Ledger,
        engine: &mut E,
        destination: &impl crate::note_export::NoteDestination,
        request: Option<&crate::writer_flow::Request>,
        admitted: &RequestScope,
        observe: &mut (dyn FnMut() -> Result<SourceObservation> + Send),
    ) -> Result<ExportDecision> {
        self.require_ledger(ledger)?;
        use crate::writer_flow::{self, Phase, Request};
        ensure!(self.admits(Mode::Writer, true), "Writer export disabled");
        admitted.check(ledger, observe)?;
        let Some(request @ Request::ExportDraft { .. }) = request else {
            return Ok(ExportDecision::Clarify("Which draft should I export?"));
        };
        ensure!(
            request.conversation() == admitted.conversation
                && request.identities().0 == admitted.operation,
            "export request identity changed"
        );
        let (cancel, canceled) = tokio::sync::watch::channel(false);
        let preference = self.config.personalization_snapshot()?;
        let report = writer_flow::run_with_events(
            ledger,
            engine,
            destination,
            request,
            &preference,
            canceled,
            &mut |phase, ledger| {
                let valid = if phase == Phase::BeforeAdmission {
                    admitted.check(ledger, observe).is_ok()
                } else {
                    observe().is_ok_and(|current| current == admitted.source)
                };
                if !valid {
                    let _ = cancel.send(true);
                }
            },
        );
        Ok(ExportDecision::Attempt(Box::new(report)))
    }

    pub(crate) fn writer<E: LLMEngine>(
        &mut self,
        ledger: &Ledger,
        engine: &mut E,
        request: &writer::Request,
        admitted: &RequestScope,
        observe: &mut dyn FnMut() -> Result<SourceObservation>,
    ) -> Result<(writer::Report, Option<PendingOutput>)> {
        self.require_ledger(ledger)?;
        ensure!(self.admits(Mode::Writer, false), "Writer disabled");
        ensure!(
            !matches!(request, writer::Request::PrepareExport { .. }),
            "export requires explicit effect admission"
        );
        ensure!(
            request.conversation() == admitted.conversation
                && request.operation() == admitted.operation,
            "request identity changed"
        );
        if let writer::Request::Generate { document, .. } = request {
            ensure!(
                document.document == admitted.source.document,
                "Writer source document changed"
            );
        }
        admitted.check(ledger, observe)?;
        let preference = self.config.personalization_snapshot()?;
        let mut canceled = || !observe().is_ok_and(|current| current == admitted.source);
        let (report, selection) = if let Some(provider) = self.memory.as_deref() {
            writer::run_receiving_with_memory(
                ledger,
                engine,
                request,
                &preference,
                Some(provider),
                &mut canceled,
                Some(admitted.root.clone()),
            )
        } else {
            writer::run_receiving(
                ledger,
                engine,
                request,
                &preference,
                &mut canceled,
                Some(admitted.root.clone()),
            )
        };
        let output = selection
            .zip(report.draft)
            .map(|(selection, id)| PendingOutput {
                id,
                revision: ExpectedHeads(std::collections::BTreeSet::from([
                    selection.draft_revision()
                ])),
                mode: Mode::Writer,
                conversation: admitted.conversation,
                root: selection.root,
                source: admitted.source.clone(),
            });
        Ok((report, output))
    }

    pub(crate) fn reader(
        &mut self,
        ledger: &Ledger,
        engine: &mut dyn LLMEngine,
        request: &str,
        history_range: Option<TurnRange>,
        admitted: &RequestScope,
        observe: &mut dyn FnMut() -> Result<SourceObservation>,
    ) -> Result<PendingOutput> {
        self.require_ledger(ledger)?;
        ensure!(self.admits(Mode::Reader, false), "Reader disabled");
        ensure!(
            !request.trim().is_empty() && request.len() <= 4096 && !request.contains('\0'),
            "invalid Reader request"
        );
        admitted.check(ledger, observe)?;
        let context = ledger.context_range(
            admitted.conversation,
            history_range,
            &ContextBudget {
                max_text_bytes: 16 * 1024,
                max_turns: 64,
                provider_token_limit: Some(16 * 1024),
            },
            |turns| {
                Ok(turns
                    .iter()
                    .map(|t| t.text.as_ref().map_or(0, String::len))
                    .sum())
            },
        )?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis()
            .try_into()?;
        let user = Turn {
            id: writer::stage(admitted.operation, "reader-user"),
            conversation: admitted.conversation,
            exchange: admitted.operation,
            sequence: 0,
            role: Role::User,
            mode: Mode::Reader,
            outcome: Outcome::Interpreted,
            text: Some(request.to_owned()),
            sources: vec![],
            correction_of: None,
            created_ms: timestamp,
            updated_ms: timestamp,
            completion: None,
        };
        ensure!(
            ledger
                .expected(Namespace::Conversation, user.id)?
                .0
                .is_empty(),
            "duplicate Reader request"
        );
        let saved = ledger.append(
            admitted.operation,
            admitted.root.clone(),
            user.clone(),
            vec![],
        )?;
        let root = ExpectedHeads(
            saved
                .current_root
                .iter()
                .map(|value| value.revision_id)
                .collect(),
        );
        let user_envelope = ledger
            .store()
            .value(Namespace::Conversation, user.id)?
            .ok_or_else(|| anyhow::anyhow!("Reader request absent"))?;
        let user_heads =
            ExpectedHeads(std::collections::BTreeSet::from(
                [user_envelope.revision_id],
            ));
        let crate::conversation::Record::Turn(persisted_user) = Ledger::decode(&user_envelope)?
        else {
            anyhow::bail!("Reader request changed");
        };
        let mut progress = || {
            ensure!(observe()? == admitted.source, "request visit changed");
            ensure!(
                ledger.expected(Namespace::Conversation, admitted.conversation)? == root,
                "conversation revision changed"
            );
            Ok(())
        };
        let answer = crate::conversation::receiving::answer::follow_up(
            ledger,
            engine,
            &context,
            request,
            &self.config.personalization_snapshot()?,
            self.memory
                .as_deref()
                .map(|provider| (provider, Some(admitted.source.document), timestamp)),
            &mut progress,
        );
        let answer = match answer {
            Ok(answer) => answer,
            Err(error) if self.memory.is_none() => return Err(error),
            Err(error) => {
                let canceled = progress().is_err();
                let mut failed = persisted_user.clone();
                failed.outcome = if canceled {
                    Outcome::Canceled
                } else {
                    Outcome::Failed
                };
                let reason = if canceled {
                    crate::conversation::AttemptReason::Canceled
                } else {
                    crate::conversation::AttemptReason::ProviderFailure
                };
                ledger.record_attempt_outcome(
                    writer::stage(admitted.operation, "reader-failure"),
                    root.clone(),
                    user_heads,
                    failed,
                    reason,
                )?;
                return Err(error);
            }
        };
        let draft = Turn {
            id: writer::stage(admitted.operation, "reader-draft"),
            role: Role::Assistant,
            outcome: Outcome::Generated,
            text: Some(answer),
            ..user
        };
        let saved = ledger.append(
            writer::stage(admitted.operation, "reader-answer"),
            root,
            draft.clone(),
            vec![],
        )?;
        Ok(PendingOutput {
            id: draft.id,
            revision: ledger.expected(Namespace::Conversation, draft.id)?,
            mode: Mode::Reader,
            conversation: draft.conversation,
            root: ExpectedHeads(
                saved
                    .current_root
                    .iter()
                    .map(|value| value.revision_id)
                    .collect(),
            ),
            source: admitted.source.clone(),
        })
    }

    pub(crate) fn notebook<E: LLMEngine>(
        &mut self,
        ledger: &Ledger,
        engine: &mut E,
        request: &writer::notebook::command::Request,
        frozen: Option<writer::notebook::ValidatedNotebook>,
        admitted: &RequestScope,
        observe: &mut dyn FnMut() -> Result<SourceObservation>,
    ) -> Result<(writer::notebook::runner::Report, Option<PendingOutput>)> {
        use writer::notebook::{command, runner};
        self.require_ledger(ledger)?;
        ensure!(self.admits(Mode::Writer, false), "Writer disabled");
        ensure!(
            request.conversation() == admitted.conversation
                && request.operation() == admitted.operation,
            "notebook request identity changed"
        );
        admitted.check(ledger, observe)?;
        let input = command::prepare_receiving(
            ledger,
            request,
            &self.config.personalization_snapshot()?,
            frozen,
            &admitted.root,
            admitted.source.document,
            self.memory.as_deref(),
        )?;
        admitted.check(ledger, observe)?;
        let result = runner::run(ledger, engine, input, &mut || {
            !observe().is_ok_and(|current| current == admitted.source)
        });
        let output = result.finalized.map(|finalized| PendingOutput {
            id: finalized.draft().record_id,
            revision: ExpectedHeads(std::collections::BTreeSet::from([finalized
                .draft()
                .revision_id])),
            mode: Mode::Writer,
            conversation: admitted.conversation,
            root: ExpectedHeads(std::collections::BTreeSet::from([finalized
                .root()
                .revision_id])),
            source: admitted.source.clone(),
        });
        Ok((result.report, output))
    }

    /// Record the page owner's verified result. This path never emits native
    /// input or treats the draft itself as render authority.
    pub(crate) fn record_output(
        &mut self,
        ledger: &Ledger,
        admitted: &RequestScope,
        draft: &PendingOutput,
        evidence: crate::conversation::CompletionEvidence,
        observe: &mut dyn FnMut() -> Result<SourceObservation>,
    ) -> Result<()> {
        self.require_ledger(ledger)?;
        ensure!(
            self.admits(draft.mode.clone(), false),
            "receiving buddy disabled"
        );
        ensure!(
            draft.conversation == admitted.conversation
                && draft.root == admitted.root
                && draft.source == admitted.source,
            "draft ownership changed"
        );
        admitted.check(ledger, observe)?;
        let mut turn = ledger
            .inspect(admitted.conversation, false)?
            .into_iter()
            .find_map(|record| match record {
                crate::conversation::Record::Turn(turn) if turn.id == draft.id => Some(turn),
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("draft unavailable"))?;
        ensure!(
            turn.mode == draft.mode
                && turn.role == Role::Assistant
                && turn.outcome == Outcome::Generated
                && turn.text.as_deref() == Some(evidence.exact_text.as_str())
                && evidence.native_operation == admitted.operation
                && evidence.observation == admitted.source,
            "output evidence does not match the admitted draft"
        );
        let expected_turn = ledger.expected(Namespace::Conversation, draft.id)?;
        ensure!(
            expected_turn.0.len() == 1 && expected_turn == draft.revision,
            "draft revision changed"
        );
        turn.outcome = Outcome::Completed;
        turn.completion = Some(evidence);
        admitted.check(ledger, observe)?;
        ensure!(
            ledger.expected(Namespace::Conversation, draft.id)? == expected_turn,
            "draft revision changed"
        );
        if draft.mode == Mode::Writer {
            // Draft selection for refine/export continues to refer to Generated.
            // Record the verified visible response separately in its exchange.
            turn.id = writer::stage(draft.id, "visible-output");
            turn.sequence = 0;
            ledger.append(admitted.operation, admitted.root.clone(), turn, vec![])?;
        } else {
            ledger.advance(
                admitted.operation,
                admitted.root.clone(),
                expected_turn,
                turn,
            )?;
        }
        Ok(())
    }
}

pub(crate) struct PendingOutput {
    id: Uuid,
    revision: ExpectedHeads,
    mode: Mode,
    conversation: Uuid,
    root: ExpectedHeads,
    source: SourceObservation,
}

pub(crate) enum ExportDecision {
    Clarify(&'static str),
    Attempt(Box<crate::writer_flow::Report>),
}

/// The input owner supplies an observation and stable event identity. A raw
/// Interaction cannot construct these facts.
pub(crate) struct RequestScope {
    pub conversation: Uuid,
    pub operation: Uuid,
    pub root: ExpectedHeads,
    pub source: SourceObservation,
}

impl RequestScope {
    fn check(
        &self,
        ledger: &Ledger,
        observe: &mut dyn FnMut() -> Result<SourceObservation>,
    ) -> Result<()> {
        self.check_source(observe)?;
        ensure!(
            self.root.0.len() == 1
                && ledger.expected(Namespace::Conversation, self.conversation)? == self.root,
            "conversation revision changed"
        );
        Ok(())
    }

    fn check_source(&self, observe: &mut dyn FnMut() -> Result<SourceObservation>) -> Result<()> {
        ensure!(
            !self.conversation.is_nil()
                && !self.operation.is_nil()
                && !self.source.document.is_nil()
                && !self.source.page.is_nil()
                && !self.source.session.is_empty()
                && !self.source.visit.is_empty()
                && !self.source.capability_revision.is_empty()
                && !self.source.evidence_procedure.is_empty()
                && self
                    .source
                    .content_revision
                    .as_ref()
                    .is_some_and(|value| !value.is_empty()),
            "request source authority unavailable"
        );
        ensure!(observe()? == self.source, "request visit changed");
        Ok(())
    }
}

// Typed native image/intent ingress remains an explicit integration gap.
pub(super) mod followup_research;
#[allow(dead_code)]
pub(crate) mod handwriting;
use crate::{
    config::Config,
    conversation::{
        evidence::FollowupEvidence, receiving::ReceivingBudget, resolution::SourceDirective,
        ContextBudget, ExpectedHeads, Ledger, Mode, Outcome, Role, SourceObservation, Turn,
        TurnRange,
    },
    device::interaction::Interaction,
    retrieval::{
        notebook::{
            NotebookSourceReceipt, NotebookText, OrderedNotebookImages, PageImage, SuppliedNotebook,
        },
        IndexKey, Provider,
    },
    storage::{Namespace, Uuid},
    writer, LLMEngine,
};
use anyhow::{ensure, Result};
pub(crate) use followup_research::FollowupResearch;
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

    pub(super) fn reader_trigger(self, config: &Config, events: &[Interaction]) -> bool {
        config.reader_enabled()
            && match self {
                Self::OrdinaryLocal => matches!(
                    events,
                    [Interaction::Reader] | [Interaction::Invalidated, Interaction::Reader]
                ),
                Self::ExistingReader => events.contains(&Interaction::Reader),
            }
    }
}

pub struct Dispatch<'a> {
    pub(super) config: &'a Config,
    store: Arc<crate::storage::Store>,
    _local: Arc<crate::mode_maintenance::LocalDispatchLifetime>,
    memory: Option<Arc<crate::memory::LocalMemory>>,
    #[cfg(target_os = "linux")]
    regional_writer_model: Option<String>,
    input_events: std::collections::BTreeSet<Uuid>,
    #[cfg(all(test, target_os = "linux", feature = "installed-resident-output"))]
    pub(super) test_note_destination: Option<Arc<crate::writer_flow::tests::Destination>>,
}

impl<'a> Dispatch<'a> {
    pub fn local(config: &'a Config, ledger: &Ledger) -> Result<Self> {
        ensure!(
            RuntimeRoute::for_config(config) == RuntimeRoute::OrdinaryLocal
                && config.memory.selection.is_none(),
            "unified dispatch requires ordinary local configuration"
        );
        config.memory.validate()?;
        let store = ledger.store();
        let local = Arc::new(match store.sync_mode_snapshot()?.mode {
            Some(crate::storage::sync::mode::SyncMode::IsolatedBackup { .. }) => {
                crate::mode_maintenance::admit_isolated_local_dispatch(store, config)?
            }
            _ => crate::mode_maintenance::admit_local_dispatch(store)?,
        });
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
            #[cfg(target_os = "linux")]
            regional_writer_model: None,
            input_events: Default::default(),
            #[cfg(all(test, target_os = "linux", feature = "installed-resident-output"))]
            test_note_destination: None,
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

    #[cfg(target_os = "linux")]
    pub fn with_regional_writer_model(mut self, resolved_model: String) -> Self {
        self.regional_writer_model = Some(resolved_model);
        self
    }

    #[cfg(all(target_os = "linux", feature = "installed-resident-output"))]
    pub(super) fn shared_source_writer_enabled(&self) -> bool {
        self.admits(Mode::Writer, false) && self.regional_writer_model.is_some()
    }

    #[cfg(target_os = "linux")]
    pub(super) fn regional_writer_settings(&self) -> Result<(&Config, &str)> {
        ensure!(self.admits(Mode::Writer, false), "Writer disabled");
        let model = self
            .regional_writer_model
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("regional Writer model profile unavailable"))?;
        Ok((self.config, model))
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
        RuntimeRoute::OrdinaryLocal.reader_trigger(self.config, events)
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
                followup: None,
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
            followup: None,
        })
    }

    /// The trusted owner supplies the independently observed PDF or EPUB
    /// separately from the input visit and a deadline-aware, single-request
    /// answering engine. The observer must refuse revoked input or configuration.
    #[allow(dead_code, clippy::too_many_arguments)]
    pub(crate) fn reader_with_source(
        &mut self,
        ledger: &Ledger,
        engine: &mut dyn LLMEngine,
        request: &str,
        history_range: Option<TurnRange>,
        admitted: &RequestScope,
        source: SourceDirective,
        original: Option<FollowupSource<'_>>,
        research: FollowupResearch<'_>,
        observe: &mut dyn FnMut() -> Result<SourceObservation>,
    ) -> Result<PendingOutput> {
        self.reader_with_original(
            ledger,
            engine,
            request,
            history_range,
            admitted,
            source,
            original,
            None,
            research,
            observe,
        )
    }

    /// Admit a bounded, independently supplied historical OCR page set.
    #[allow(dead_code, clippy::too_many_arguments)]
    pub(crate) fn reader_with_notebook(
        &mut self,
        ledger: &Ledger,
        engine: &mut dyn LLMEngine,
        request: &str,
        history_range: Option<TurnRange>,
        admitted: &RequestScope,
        source: SourceDirective,
        notebook: NotebookFollowupSource<'_>,
        research: FollowupResearch<'_>,
        observe: &mut dyn FnMut() -> Result<SourceObservation>,
    ) -> Result<PendingOutput> {
        self.require_ledger(ledger)?;
        ensure!(self.admits(Mode::Reader, false), "Reader disabled");
        admitted.check(ledger, observe)?;
        let mut check = |_| {
            research.check(admitted, request)?;
            (notebook.check)()
        };
        check(crate::retrieval::notebook::Progress::BeforeReturn)?;
        let original = notebook
            .ocr
            .admit_supplied(notebook.input, notebook.pages, &mut check)?;
        let retrieval = FollowupSource {
            key: original.index().key().clone(),
            provider: original.index(),
            check: notebook.check,
        };
        self.reader_with_original(
            ledger,
            engine,
            request,
            history_range,
            admitted,
            source,
            Some(retrieval),
            Some(&original),
            research,
            observe,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn reader_with_original(
        &mut self,
        ledger: &Ledger,
        engine: &mut dyn LLMEngine,
        request: &str,
        history_range: Option<TurnRange>,
        admitted: &RequestScope,
        source: SourceDirective,
        original: Option<FollowupSource<'_>>,
        notebook: Option<&SuppliedNotebook<'_>>,
        research: FollowupResearch<'_>,
        observe: &mut dyn FnMut() -> Result<SourceObservation>,
    ) -> Result<PendingOutput> {
        self.require_ledger(ledger)?;
        ensure!(self.admits(Mode::Reader, false), "Reader disabled");
        ensure!(
            !request.trim().is_empty() && request.len() <= 4096 && !request.contains('\0'),
            "invalid Reader request"
        );
        research.check(admitted, request)?;
        let policy = self.config.research_policy_snapshot()?.policy();
        let expected = match &source {
            SourceDirective::Explicit(
                crate::conversation::receiving::RequestSource::Original { expected, .. },
            ) => Some(expected),
            SourceDirective::Explicit(
                crate::conversation::receiving::RequestSource::NoDocument
                | crate::conversation::receiving::RequestSource::CapturedPage { .. },
            ) => None,
            SourceDirective::Inherit { expected, .. } => expected.as_ref(),
        };
        ensure!(
            original.as_ref().map(|original| &original.key) == expected,
            "Reader source admission changed"
        );
        ensure!(
            expected.is_some_and(|key| key.format == crate::retrieval::SourceFormat::Notebook)
                == notebook.is_some(),
            "Reader notebook requires independent supplied OCR admission"
        );
        let source_check = original.as_ref().map(|original| original.check.clone());
        let check_source = || verify_followup_source(&source_check);
        let visit = std::cell::RefCell::new(observe);
        let check_visit = || admitted.check_source(&mut **visit.borrow_mut());
        check_visit()?;
        check_source()?;
        research.check(admitted, request)?;
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
        let head =
            ledger.append_followup_user(admitted.operation, admitted.root.clone(), user.clone())?;
        let check_retrieval = || {
            check_visit()?;
            check_source()?;
            research.check(admitted, request)
        };
        let retrieval = original.as_ref().map(|original| CheckedFollowupProvider {
            provider: original.provider,
            check: &check_retrieval,
        });
        let prepared = ledger.prepare_declared_evidence(
            admitted.conversation,
            user.id,
            history_range,
            source,
            ReceivingBudget {
                context: ContextBudget {
                    max_text_bytes: 16 * 1024,
                    max_turns: 64,
                    provider_token_limit: Some(96 * 1024),
                },
                max_encoded_bytes: 96 * 1024,
                max_passages: 8,
                max_passage_bytes: 16 * 1024,
                neighbors: true,
            },
            retrieval.as_ref().map(|provider| provider as &dyn Provider),
            |text| Ok(text.len()),
            Some(head),
            notebook,
        )?;
        check_visit()?;
        check_source()?;
        research.check(admitted, request)?;
        let (context, mut evidence) = ledger.persist_followup_evidence(
            writer::stage(admitted.operation, "reader-source-evidence"),
            timestamp,
            prepared,
        )?;
        let check = || {
            check_visit()?;
            check_source()?;
            research.check(admitted, request)?;
            evidence.with_current(ledger, || Ok(()))
        };
        let submit = |submit: &mut dyn FnMut() -> Result<()>| {
            check_visit()?;
            check_source()?;
            research.check(admitted, request)?;
            evidence.with_current(ledger, || {
                research.check(admitted, request)?;
                submit()
            })
        };
        let answer = followup_research::answer(
            ledger,
            engine,
            context,
            self.config,
            self.memory.as_deref(),
            timestamp,
            policy,
            &research.answer,
            &check,
            submit,
        )?;
        check()?;
        let draft = Turn {
            id: writer::stage(admitted.operation, "reader-draft"),
            role: Role::Assistant,
            outcome: Outcome::Generated,
            text: Some(answer),
            ..user
        };
        let revision = evidence.generated(
            ledger,
            writer::stage(admitted.operation, "reader-answer"),
            draft.clone(),
        )?;
        Ok(PendingOutput {
            id: draft.id,
            revision,
            mode: Mode::Reader,
            conversation: draft.conversation,
            root: evidence.root(),
            source: admitted.source.clone(),
            followup: Some(FollowupOutput {
                evidence,
                source_check,
                operation: admitted.operation,
                attempted: std::cell::Cell::new(false),
                submitted: std::cell::Cell::new(false),
            }),
        })
    }

    /// Consume a sealed acquisition. Both the admitted content revision and
    /// every subsequent observation must cover the entire saved source.
    #[allow(
        dead_code,
        reason = "the installed notebook acquisition runs only in the Linux output profile"
    )]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn whole_notebook<E: LLMEngine>(
        &self,
        ledger: &Ledger,
        engine: &mut E,
        text: &str,
        images: &OrderedNotebookImages<'_>,
        receipt: &NotebookSourceReceipt,
        model_profile: &str,
        admitted: &RequestScope,
        visit: &mut dyn NotebookVisit,
    ) -> Result<(writer::notebook::runner::Report, Option<PendingOutput>)> {
        use writer::notebook::{command::Request, ValidatedNotebook};

        self.require_ledger(ledger)?;
        ensure!(self.admits(Mode::Writer, false), "Writer disabled");
        let order_revision = images.order_revision()?;
        ensure!(
            images.document == admitted.source.document
                && admitted.source.order_revision.as_deref() == Some(order_revision.as_str()),
            "notebook document or page order changed"
        );
        ensure!(
            crate::storage::valid_digest(&receipt.saved_source_revision)
                && admitted.source.content_revision.as_deref()
                    == Some(receipt.saved_source_revision.as_str()),
            "notebook saved source differs from acquisition receipt"
        );
        let request = Request::GenerateAcquiredNotebook {
            conversation: admitted.conversation,
            operation: admitted.operation,
            request: text.into(),
        };
        request.preflight()?;
        admitted.check(ledger, &mut || visit.observe())?;
        ensure!(
            ledger
                .store()
                .value(
                    Namespace::Conversation,
                    crate::conversation::notebook::NotebookPlan::identity(admitted.operation),
                )?
                .is_none(),
            "routed notebook operation consumed"
        );
        let originals =
            images.admit_originals(admitted.source.page, &receipt.ordered_image_sha256)?;
        let handwriting = self.handwriting_runtime(ledger)?.map(|runtime| {
            runtime.capture(ledger.store(), &originals.target_hashes, &mut || {
                visit
                    .progress(&admitted.source)
                    .and_then(|()| admitted.check_root(ledger))
                    .map_err(|_| crate::handwriting::ContextFailure::Canceled)
            })
        });
        ensure!(
            handwriting
                .as_ref()
                .is_none_or(|capture| capture.failure.is_none()),
            "Writer handwriting context unavailable"
        );
        let normalized = originals.normalize(
            model_profile,
            handwriting
                .as_ref()
                .and_then(|capture| capture.context.as_deref()),
            engine,
            &mut |progress| {
                use crate::retrieval::notebook::Progress;
                match progress {
                    Progress::BeforeDispatch | Progress::AfterResponse => {
                        admitted.check(ledger, &mut || visit.observe())
                    }
                    _ => {
                        visit.progress(&admitted.source)?;
                        admitted.check_root(ledger)
                    }
                }
            },
        )?;
        drop(handwriting);
        let bytes = serde_json::to_vec(&normalized)?;
        let frozen = ValidatedNotebook::from_bytes(&bytes, &crate::storage::digest(&bytes))?;
        self.notebook(ledger, engine, &request, Some(frozen), admitted, visit)
    }

    pub(crate) fn notebook<E: LLMEngine>(
        &self,
        ledger: &Ledger,
        engine: &mut E,
        request: &writer::notebook::command::Request,
        frozen: Option<writer::notebook::ValidatedNotebook>,
        admitted: &RequestScope,
        visit: &mut dyn NotebookVisit,
    ) -> Result<(writer::notebook::runner::Report, Option<PendingOutput>)> {
        use writer::notebook::{command, runner};
        self.require_ledger(ledger)?;
        ensure!(self.admits(Mode::Writer, false), "Writer disabled");
        ensure!(
            request.conversation() == admitted.conversation
                && request.operation() == admitted.operation,
            "notebook request identity changed"
        );
        admitted.check(ledger, &mut || visit.observe())?;
        let input = command::prepare_receiving(
            ledger,
            request,
            &self.config.personalization_snapshot()?,
            frozen,
            &admitted.root,
            admitted.source.document,
            self.memory.as_deref(),
        )?;
        admitted.check(ledger, &mut || visit.observe())?;
        let result = runner::run_checked(ledger, engine, input, &mut |check| match check {
            runner::Check::Boundary => !visit
                .observe()
                .is_ok_and(|current| current == admitted.source),
            runner::Check::Progress => visit.progress(&admitted.source).is_err(),
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
            followup: None,
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
        if let Some(followup) = &draft.followup {
            ensure!(
                followup.submitted.get() && followup.operation == admitted.operation,
                "Reader output submission is unverified"
            );
            verify_followup_source(&followup.source_check)?;
            followup.evidence.with_current(ledger, || Ok(()))?;
        }
        let mut turn = if let Some(followup) = &draft.followup {
            followup.evidence.draft(draft.id)?
        } else {
            ledger
                .inspect(admitted.conversation, false)?
                .into_iter()
                .find_map(|record| match record {
                    crate::conversation::Record::Turn(turn) if turn.id == draft.id => Some(turn),
                    _ => None,
                })
                .ok_or_else(|| anyhow::anyhow!("draft unavailable"))?
        };
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
        if let Some(followup) = &draft.followup {
            verify_followup_source(&followup.source_check)?;
            followup.evidence.complete(
                ledger,
                writer::stage(admitted.operation, "reader-output"),
                expected_turn,
                turn,
            )?;
        } else if draft.mode == Mode::Writer {
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
    followup: Option<FollowupOutput>,
}

#[cfg(all(target_os = "linux", feature = "installed-resident-output"))]
impl PendingOutput {
    pub(crate) fn notebook_selection(
        &self,
        ledger: &Ledger,
    ) -> Result<crate::conversation::WriterSelection> {
        self.notebook_draft(ledger)?;
        ledger.select_writer_for_flow(self.conversation, self.id, self.root.clone())
    }

    pub(crate) fn notebook_draft(&self, ledger: &Ledger) -> Result<Turn> {
        ensure!(
            self.mode == Mode::Writer
                && ledger.expected(Namespace::Conversation, self.conversation)? == self.root
                && ledger.expected(Namespace::Conversation, self.id)? == self.revision,
            "notebook Writer draft changed before placement"
        );
        let draft = ledger.writer_turn(self.id, self.conversation)?;
        ensure!(
            draft.role == Role::Assistant && draft.outcome == Outcome::Generated,
            "notebook Writer draft is no longer generated"
        );
        Ok(draft)
    }
}

type SourceCurrent = Arc<dyn Fn() -> Result<()> + Send + Sync>;

/// Host-only material from independent source observation. `check` must
/// revalidate that original observation, not just assert a cache key.
pub(crate) struct FollowupSource<'a> {
    pub(crate) key: IndexKey,
    pub(crate) provider: &'a dyn Provider,
    pub(crate) check: SourceCurrent,
}

/// Fresh observation of the supplied notebook for this Reader operation.
/// `check` samples that observation only and must not reopen the Store.
pub(crate) struct NotebookFollowupSource<'a> {
    pub(crate) input: &'a writer::notebook::ValidatedNotebook,
    pub(crate) ocr: &'a NotebookText,
    pub(crate) pages: &'a [PageImage<'a>],
    pub(crate) check: SourceCurrent,
}

struct FollowupOutput {
    evidence: FollowupEvidence,
    source_check: Option<SourceCurrent>,
    operation: Uuid,
    attempted: std::cell::Cell<bool>,
    submitted: std::cell::Cell<bool>,
}

impl PendingOutput {
    #[allow(dead_code)]
    pub(crate) fn admit_followup_output(
        &self,
        ledger: &Ledger,
        admitted: &RequestScope,
        visit: &mut dyn FnMut() -> Result<SourceObservation>,
        submit: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let followup = self
            .followup
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("source-aware Reader draft required"))?;
        ensure!(
            admitted.conversation == self.conversation
                && admitted.operation == followup.operation
                && admitted.root == self.root
                && admitted.source == self.source,
            "Reader output ownership changed"
        );
        admitted.check_source(visit)?;
        verify_followup_source(&followup.source_check)?;
        // This callback only submits the original effect. It cannot reenter
        // this Store or wait for native completion while the guard is held.
        followup.evidence.with_current(ledger, || {
            ensure!(
                !followup.attempted.replace(true),
                "Reader output already attempted"
            );
            submit()?;
            followup.submitted.set(true);
            Ok(())
        })
    }

    #[allow(dead_code)]
    pub(crate) fn root(&self) -> &ExpectedHeads {
        &self.root
    }
}

fn verify_followup_source(check: &Option<SourceCurrent>) -> Result<()> {
    if let Some(check) = check {
        check()?;
    }
    Ok(())
}

struct CheckedFollowupProvider<'a> {
    provider: &'a dyn Provider,
    check: &'a dyn Fn() -> Result<()>,
}

impl Provider for CheckedFollowupProvider<'_> {
    fn query(
        &self,
        key: &IndexKey,
        question: &str,
        max_passages: usize,
        max_bytes: usize,
        neighbors: bool,
    ) -> Result<crate::retrieval::Retrieved> {
        (self.check)()?;
        let result = self
            .provider
            .query(key, question, max_passages, max_bytes, neighbors)?;
        (self.check)()?;
        Ok(result)
    }
}

pub(crate) enum ExportDecision {
    Clarify(&'static str),
    Attempt(Box<crate::writer_flow::Report>),
}

/// A notebook owner observes the visit and saved source at operation boundaries.
/// The cheap progress pulse services the same input and cancellation guard.
pub(crate) trait NotebookVisit {
    fn observe(&mut self) -> Result<SourceObservation>;
    fn progress(&mut self, expected: &SourceObservation) -> Result<()>;
}

impl<F: FnMut() -> Result<SourceObservation>> NotebookVisit for F {
    fn observe(&mut self) -> Result<SourceObservation> {
        self()
    }

    fn progress(&mut self, expected: &SourceObservation) -> Result<()> {
        ensure!(&self()? == expected, "request visit changed");
        Ok(())
    }
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
        self.check_root(ledger)
    }

    fn check_root(&self, ledger: &Ledger) -> Result<()> {
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

//! Typed image input for the existing selected Writer.
use super::*;
use crate::{
    conversation::{IntentSelectionEvidence, SelectedWriterImage, WriterNormalizationPrepared},
    handwriting::ConfirmedContext,
    retrieval::notebook::{self, NormalizeFailure, Normalized, TranscriptionPolicy},
    storage::{
        selection::SelectedReadBudget,
        sync::selected::{NotebookPhase, RequestPreferences},
        ObjectRef,
    },
};
use std::{
    cell::{Cell, RefCell},
    time::{Duration, Instant},
};
pub(crate) mod regional;
pub(crate) use regional::run_target;
pub use regional::WriterTargetRequest;
use regional::{Input, SourceProof};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriterImageRequest {
    pub operation: Uuid,
    pub request: String,
    pub image: SelectedWriterImage,
    pub guidance: Option<String>,
    pub model_profile: String,
}

impl WriterImageRequest {
    pub(crate) fn validate(&self, root: &IntentRecordRef) -> Result<()> {
        self.image.validate()?;
        root.validate()?;
        validate_request(&self.request)?;
        ensure!(
            !self.operation.is_nil()
                && self.operation != root.record_id
                && root.namespace == Namespace::Conversation
                && self
                    .guidance
                    .as_ref()
                    .is_none_or(|text| !text.trim().is_empty()
                        && text.len() <= 4096
                        && !text.contains('\0'))
                && !self.model_profile.trim().is_empty()
                && self.model_profile.len() <= 128
                && !self.model_profile.chars().any(char::is_control),
            "invalid Writer image request"
        );
        Ok(())
    }

    pub(crate) fn preparation(&self) -> Uuid {
        normalization_preparations(self.operation)[0]
    }

    pub(crate) fn preparation_fingerprint(
        &self,
        root: &IntentRecordRef,
        selection: &IntentSelectionEvidence,
    ) -> Result<String> {
        // Exact source-envelope bytes bind the target image descriptor and ordinal.
        Ok(digest(&serde_json::to_vec(&(
            "selected-writer-normalize-v1",
            self,
            root,
            selection,
        ))?))
    }
}

type Memory<'a> = (
    &'a dyn crate::memory::Provider,
    &'a crate::memory::QueryResult,
);

pub(super) struct Continuation<'a> {
    preferences: &'a RequestPreferences,
    original: &'a SelectionToken,
    operation: Uuid,
    source: SourceProof<'a>,
    canceled: Option<&'a dyn Fn() -> bool>,
    completed: Cell<u8>,
    document: Uuid,
    handwriting: Option<ConfirmedContext>,
    memory_view: Option<crate::memory::QueryResult>,
    stage: Cell<u8>,
    submitted: Cell<u8>,
}

impl Continuation<'_> {
    fn check_time(&self) -> Result<()> {
        self.check_phase_time(None)
    }

    fn check_phase_time(&self, phase: Option<NotebookPhase>) -> Result<()> {
        let offset = match (phase, self.completed.get()) {
            (Some(NotebookPhase::Admission), _) => 60,
            (_, 0) => 150,
            (_, 1) => 240,
            _ => 300,
        };
        self.check_cutoff(offset)
    }

    fn check_cutoff(&self, offset: u64) -> Result<()> {
        if self.source.regional() {
            ensure!(
                !self.canceled.is_some_and(|c| c()),
                "regional Writer canceled"
            );
            ensure!(
                regional::now() < self.preferences.regional_cutoff(offset)?,
                "regional Writer slot expired"
            );
        }
        Ok(())
    }

    fn check_storage_time(&self, phase: NotebookPhase) -> Result<()> {
        if self.source.regional() {
            self.check_phase_time(Some(phase))
        } else {
            ensure!(!self.canceled.is_some_and(|c| c()), "Writer image canceled");
            self.preferences.writer_image_phase_time(phase)
        }
    }

    pub(super) fn memory_view(&mut self) -> Option<crate::memory::QueryResult> {
        self.memory_view.take()
    }
    pub(super) fn progress(
        &self,
        writer: &SelectedAdmission,
        token: &SelectionToken,
    ) -> Result<()> {
        self.check_time()?;
        self.preferences.notebook_progress(writer, token)
    }

    pub(super) fn draft_input(
        &self,
        writer: &SelectedAdmission,
        prepared: &crate::conversation::SelectedWriterPrepared,
    ) -> Result<crate::conversation::SelectedWriterInput> {
        self.phase(
            writer,
            (&prepared.accepted, &prepared.root),
            NotebookPhase::Handoff,
            None,
            |store, reads| writer.writer_input_with_reads_locked(store, prepared, Some(reads)),
        )
    }

    pub(super) fn draft_context(
        &self,
        writer: &SelectedAdmission,
        prepared: &crate::conversation::SelectedWriterPrepared,
        budget: &crate::conversation::ContextBudget,
        count: impl Fn(&[crate::conversation::Turn]) -> Result<usize>,
    ) -> Result<crate::conversation::ContextView> {
        self.phase(
            writer,
            (&prepared.accepted, &prepared.root),
            NotebookPhase::Handoff,
            None,
            |store, reads| {
                writer.writer_context_with_reads_locked(store, prepared, budget, count, reads)
            },
        )
    }

    fn phase<T>(
        &self,
        writer: &SelectedAdmission,
        selection: (&SelectionToken, &IntentRecordRef),
        phase: NotebookPhase,
        memory: Option<Memory<'_>>,
        run: impl FnOnce(&Store, &mut SelectedReadBudget) -> Result<T>,
    ) -> Result<T> {
        let (token, root) = selection;
        self.preferences.with_document_phase(
            writer,
            Some(token),
            phase,
            memory.map(|(provider, view)| (provider, view, self.original)),
            self.handwriting.as_ref(),
            |store, reads| {
                self.check_phase_time(Some(phase))?;
                self.source
                    .check(writer, store, token, root, reads, &mut || {
                        #[cfg(test)]
                        if matches!(phase, NotebookPhase::Admission) {
                            regional::admission_recheck_for_test();
                        }
                        self.check_phase_time(Some(phase))
                    })?;
                self.check_phase_time(Some(phase))?;
                run(store, reads)
            },
        )
    }

    fn execute(
        &self,
        engine: &mut dyn LLMEngine,
        writer: &SelectedAdmission,
        selection: (&SelectionToken, &IntentRecordRef),
        stage: u8,
        memory: Option<Memory<'_>>,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<String> {
        ensure!(
            self.submitted.get() == stage,
            "selected Writer model stage out of order"
        );
        self.check_time()?;
        let deadline = if self.source.regional() {
            self.preferences
                .regional_cutoff(if stage == 0 { 150 } else { 240 })?
                .min(regional::now() + Duration::from_secs(90))
        } else {
            self.preferences.writer_image_deadline()
        };
        let response = engine.execute_with_deadline(
            progress,
            &mut |submit| {
                ensure!(
                    self.stage.get() == stage,
                    "selected Writer handoff already attempted"
                );
                self.stage.set(stage + 1);
                self.phase(
                    writer,
                    selection,
                    NotebookPhase::Handoff,
                    memory,
                    |store, reads| {
                        let check = || {
                            self.check_storage_time(NotebookPhase::Handoff)?;
                            ensure!(regional::now() < deadline, "Writer image model deadline");
                            Ok(())
                        };
                        if stage == 0 {
                            store.with_selected_operation_absent(
                                selection.0,
                                self.operation,
                                reads,
                                check,
                                submit,
                            )?;
                        } else {
                            check()?;
                            submit()?;
                        }
                        self.submitted.set(stage + 1);
                        Ok(())
                    },
                )
            },
            deadline,
        )?;
        ensure!(
            self.submitted.get() == stage + 1,
            "selected Writer result has no guarded handoff"
        );
        if self.source.regional() {
            ensure!(
                regional::now() < deadline,
                "regional Writer model slot expired"
            );
        }
        self.completed.set(stage + 1);
        Ok(response)
    }

    pub(super) fn execute_draft(
        &self,
        engine: &mut dyn LLMEngine,
        writer: &SelectedAdmission,
        prepared: &crate::conversation::SelectedWriterPrepared,
        memory: Option<Memory<'_>>,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<String> {
        self.execute(
            engine,
            writer,
            (&prepared.accepted, &prepared.root),
            1,
            memory,
            progress,
        )
    }

    pub(super) fn finish_draft(
        &self,
        writer: &SelectedAdmission,
        finish: SelectedWriterFinish,
        memory: Option<Memory<'_>>,
    ) -> Result<SelectedWriterPublication> {
        ensure!(
            finish.outcome != Outcome::Generated || self.submitted.get() == 2,
            "selected Writer draft has no guarded submission"
        );
        ensure!(
            finish.outcome != Outcome::Generated
                || !self.preferences.config().memory.enabled
                || memory.is_some(),
            "selected Writer draft memory evidence absent"
        );
        let token = finish.prepared.accepted.clone();
        let root = finish.prepared.root.clone();
        self.phase(
            writer,
            (&token, &root),
            NotebookPhase::Publication,
            memory,
            |store, reads| {
                writer.finish_writer_guarded_locked(
                    store,
                    finish,
                    Some(reads),
                    |_| Ok(()),
                    || self.check_storage_time(NotebookPhase::Publication),
                )
            },
        )
    }
}

pub(crate) fn run_image<E: LLMEngine>(
    writer: &SelectedAdmission,
    selection: &Selection,
    input: &WriterImageRequest,
    engine: &mut E,
    preferences: RequestPreferences,
    memory: Option<&dyn crate::memory::Provider>,
    canceled: &mut dyn FnMut() -> bool,
) -> Result<SelectedReport> {
    run_input(
        writer,
        selection,
        Input::Capture(input),
        engine,
        preferences,
        memory,
        canceled,
    )
}

fn run_input<E: LLMEngine>(
    writer: &SelectedAdmission,
    selection: &Selection,
    input: Input<'_>,
    engine: &mut E,
    preferences: RequestPreferences,
    memory: Option<&dyn crate::memory::Provider>,
    canceled: &mut dyn FnMut() -> bool,
) -> Result<SelectedReport> {
    let cancellation = RefCell::new(canceled);
    let is_canceled = || (cancellation.borrow_mut())();
    let mut canceled = || is_canceled();
    let fields = input.fields();
    input.validate(&selection.root)?;
    if input.regional() {
        ensure!(
            regional::now() < preferences.regional_cutoff(60)?,
            "regional admission expired"
        );
    }
    preferences.require_writer(writer)?;
    ensure!(
        selection.refine.is_none()
            && preferences.config().admits_writer(false)
            && preferences.config().memory.enabled == memory.is_some()
            && engine.guarded_timeout().is_some_and(
                |timeout| !timeout.is_zero() && timeout <= crate::llm::openai::DEFAULT_TIMEOUT
            ),
        "unsupported selected Writer image request"
    );
    if let Some(memory) = memory {
        memory.require_selected_writer(writer)?;
        ensure!(fields.request.len() <= 2048, "Writer memory query bound");
    }
    let _owner = Owner::acquire(writer.writer_store()).context("selected Writer busy")?;
    let check = || preferences.notebook_progress(writer, &selection.accepted);
    let frozen = preferences.with_document_phase(
        writer,
        Some(&selection.accepted),
        NotebookPhase::Admission,
        None,
        None,
        |store, reads| {
            for operation in normalization_prerequisites(fields.operation) {
                ensure!(
                    store
                        .selected_receipt_with_budget(writer.scope(), operation, reads)?
                        .is_none(),
                    "selected Writer image operation already has original evidence"
                );
            }
            input.resolve(
                writer,
                store,
                &selection.accepted,
                &selection.root,
                reads,
                &mut || {
                    ensure!(!canceled(), "regional Writer canceled");
                    if input.regional() {
                        ensure!(
                            regional::now() < preferences.regional_cutoff(60)?,
                            "regional admission expired"
                        );
                    }
                    Ok(())
                },
            )
        },
    )?;
    let document_id = frozen.document();
    let page_number = frozen.page();
    let target = notebook::admit_target(page_number, frozen.png().to_vec(), &mut |_| {
        ensure!(!canceled(), "selected Writer image canceled");
        check()
    })?;
    ensure!(
        target.sha256() == frozen.image_sha(),
        "Writer normalization target differs"
    );
    let handwriting = if input.regional() {
        preferences.capture_writer_targets(writer, &selection.accepted, &frozen.exclusions())?
    } else {
        preferences.capture_writer_handwriting(writer, &selection.accepted, target.sha256())?
    };
    let memory_view = memory
        .map(|provider| {
            let mut query = crate::memory::Query::new(fields.request.clone(), now());
            query.document = Some(document_id);
            let view = provider.query_selected_notebook(&query, 2)?;
            ensure!(
                view.selected(),
                "selected Writer image requires sealed memory"
            );
            Ok::<_, anyhow::Error>(view)
        })
        .transpose()?;
    let receiving = Continuation {
        preferences: &preferences,
        original: &selection.accepted,
        operation: fields.operation,
        source: frozen,
        canceled: Some(&is_canceled as &dyn Fn() -> bool),
        completed: Cell::new(0),
        document: document_id,
        handwriting,
        memory_view,
        stage: Cell::new(0),
        submitted: Cell::new(0),
    };
    let context = receiving.handwriting.as_ref();
    let memory_pin = memory.zip(receiving.memory_view.as_ref());
    let fingerprint = input.preparation_fingerprint(
        &selection.root,
        &IntentSelectionEvidence::from_token(&selection.accepted),
    )?;
    ensure!(!canceled(), "selected Writer image canceled");
    let WriterNormalizationPrepared { token, root } = receiving.phase(
        writer,
        (&selection.accepted, &selection.root),
        NotebookPhase::Admission,
        memory_pin,
        |store, reads| {
            writer.prepare_writer_normalization_locked(
                store,
                (&selection.accepted, &selection.root),
                (input.preparation(), &fingerprint, now()),
                reads,
                &normalization_prerequisites(fields.operation),
                &mut || receiving.check_storage_time(NotebookPhase::Admission),
            )
        },
    )?;
    let (normalized, _) = notebook::normalize_admitted_with(
        receiving.document,
        &target,
        &fields.model_profile,
        context.filter(|context| !context.identity().examples.is_empty()),
        TranscriptionPolicy::WholePage,
        engine,
        &mut |_| {
            ensure!(!canceled(), "selected Writer image canceled");
            receiving.progress(writer, &token)
        },
        &mut |engine, progress| {
            receiving.execute(engine, writer, (&token, &root), 0, memory_pin, progress)
        },
    );
    let Normalized::Page(page) = normalized.map_err(|failure| match failure {
        NormalizeFailure::Canceled => {
            anyhow::anyhow!("selected Writer normalization canceled or stale")
        }
        _ => anyhow::anyhow!("selected Writer image normalization failed"),
    })?
    else {
        anyhow::bail!("selected Writer image did not normalize to a complete page");
    };
    let mut page = page;
    let extractor = page.document.extractor_revision.clone();
    if let SourceProof::Regional(proof) = &receiving.source {
        page.document.extractor_revision = format!(
            "region-v1:{}",
            digest(&serde_json::to_vec(&(&proof.reference, &extractor))?)
        );
    }
    let text_scope = TextScope::Page { page: page_number };
    let normalized = NormalizedTextSource::new(
        root.record_id,
        stage(fields.operation, "user"),
        page.document.clone(),
        text_scope.clone(),
        fields.guidance.clone(),
    )?;
    let request = Request::Generate {
        conversation: root.record_id,
        operation: fields.operation,
        request: fields.request.clone(),
        document: page.document.clone(),
        selection: text_scope,
        guidance: fields.guidance.clone(),
    };
    request
        .preflight()
        .map_err(|_| anyhow::anyhow!("normalized Writer source refused"))?;
    let prepare = SelectedWriterPrepare {
        operation: fields.operation,
        root: root.clone(),
        request: fields.request.clone(),
        source: receiving.source.writer_source(
            normalized,
            &page,
            &extractor,
            &fields.model_profile,
        )?,
    };
    ensure!(!canceled(), "selected Writer image canceled");
    let prepared = receiving.phase(
        writer,
        (&token, &root),
        NotebookPhase::Publication,
        memory_pin,
        |store, reads| {
            if let SourceProof::Regional(proof) = &receiving.source {
                ensure!(
                    regional::now() < preferences.regional_cutoff(150)?,
                    "regional normalization result expired"
                );
                writer.prepare_regional_writer_locked(
                    store,
                    &token,
                    &prepare,
                    (&proof.png, now()),
                    reads,
                    &mut || receiving.check_cutoff(150),
                )
            } else {
                writer.prepare_normalized_writer_locked(
                    store,
                    &token,
                    &prepare,
                    now(),
                    reads,
                    &mut || receiving.check_storage_time(NotebookPhase::Publication),
                )
            }
        },
    );
    let personalization = preferences.config().personalization_snapshot()?;
    Ok(finish_run(
        writer,
        engine,
        &request,
        Preferences {
            personalization: &personalization,
            shared: Some(&preferences),
        },
        memory,
        &mut canceled,
        &fields.request,
        prepared,
        Some(receiving),
    ))
}

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
use std::{cell::Cell, time::Instant};

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
        stage(self.operation, "source-normalization-admission")
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
    target: &'a SelectedWriterImage,
    reference: ObjectRef,
    document: Uuid,
    handwriting: Option<ConfirmedContext>,
    memory_view: Option<crate::memory::QueryResult>,
    stage: Cell<u8>,
    submitted: Cell<u8>,
}

impl Continuation<'_> {
    pub(super) fn memory_view(&mut self) -> Option<crate::memory::QueryResult> {
        self.memory_view.take()
    }
    pub(super) fn progress(
        &self,
        writer: &SelectedAdmission,
        token: &SelectionToken,
    ) -> Result<()> {
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
                let image =
                    writer.selected_writer_image_locked(store, token, root, self.target, reads)?;
                ensure!(
                    image.document == self.document && image.reference == self.reference,
                    "selected Writer normalization source changed"
                );
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
        let response = engine.execute_with_deadline(
            progress,
            &mut |submit| {
                ensure!(
                    self.stage.get() == stage,
                    "selected Writer handoff already attempted"
                );
                self.stage.set(stage + 1);
                self.phase(writer, selection, NotebookPhase::Handoff, memory, |_, _| {
                    ensure!(
                        Instant::now() < self.preferences.writer_image_deadline(),
                        "Writer image model deadline"
                    );
                    submit()?;
                    self.submitted.set(stage + 1);
                    Ok(())
                })
            },
            self.preferences.writer_image_deadline(),
        )?;
        ensure!(
            self.submitted.get() == stage + 1,
            "selected Writer result has no guarded handoff"
        );
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
            |store, reads| writer.finish_writer_locked(store, finish, Some(reads), |_| Ok(())),
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
    input.validate(&selection.root)?;
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
        ensure!(input.request.len() <= 2048, "Writer memory query bound");
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
            ensure!(
                store
                    .selected_receipt_with_budget(writer.scope(), input.preparation(), reads)?
                    .is_none()
                    && store
                        .selected_receipt_with_budget(writer.scope(), input.operation, reads)?
                        .is_none(),
                "selected Writer image operation already has original evidence"
            );
            writer.selected_writer_image_locked(
                store,
                &selection.accepted,
                &selection.root,
                &input.image,
                reads,
            )
        },
    )?;
    let target = notebook::admit_target(input.image.page, frozen.png, &mut |_| {
        ensure!(!canceled(), "selected Writer image canceled");
        check()
    })?;
    ensure!(
        target.sha256() == frozen.reference.sha256,
        "Writer normalization target differs"
    );
    let handwriting =
        preferences.capture_writer_handwriting(writer, &selection.accepted, target.sha256())?;
    let memory_view = memory
        .map(|provider| {
            let mut query = crate::memory::Query::new(input.request.clone(), now());
            query.document = Some(frozen.document);
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
        target: &input.image,
        reference: frozen.reference,
        document: frozen.document,
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
                input.preparation(),
                &fingerprint,
                now(),
                reads,
            )
        },
    )?;
    let (normalized, _) = notebook::normalize_admitted_with(
        receiving.document,
        &target,
        &input.model_profile,
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
    let text_scope = TextScope::Page {
        page: input.image.page,
    };
    let normalized = NormalizedTextSource::new(
        root.record_id,
        stage(input.operation, "user"),
        page.document.clone(),
        text_scope.clone(),
        input.guidance.clone(),
    )?;
    let request = Request::Generate {
        conversation: root.record_id,
        operation: input.operation,
        request: input.request.clone(),
        document: page.document,
        selection: text_scope,
        guidance: input.guidance.clone(),
    };
    request
        .preflight()
        .map_err(|_| anyhow::anyhow!("normalized Writer source refused"))?;
    let prepare = SelectedWriterPrepare {
        operation: input.operation,
        root: root.clone(),
        request: input.request.clone(),
        source: SelectedWriterSource::Generate { normalized },
    };
    ensure!(!canceled(), "selected Writer image canceled");
    let prepared = receiving.phase(
        writer,
        (&token, &root),
        NotebookPhase::Publication,
        memory_pin,
        |store, reads| {
            writer.prepare_normalized_writer_locked(store, &token, &prepare, now(), reads)
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
        canceled,
        &input.request,
        prepared,
        Some(receiving),
    ))
}

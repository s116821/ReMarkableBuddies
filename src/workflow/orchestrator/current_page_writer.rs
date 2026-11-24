//! The installed Writer action for the observed source page.
use super::{Dispatch, Orchestrator};
use crate::{
    conversation::{
        ExpectedHeads, Mode, Outcome, PreparedResidentCapture, ResidentSourceCapture, Role,
        TextScope, Turn,
    },
    llm::LLMEngine,
    retrieval::notebook::{self, Normalized, TranscriptionPolicy},
    storage::Namespace,
    writer,
};
use anyhow::{ensure, Context, Result};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

impl<M: LLMEngine> Orchestrator<M> {
    pub(super) fn run_resident_current_page_writer(
        &mut self,
        request: String,
        capture: &ResidentSourceCapture,
        dispatch: &Dispatch<'_>,
    ) -> Result<()> {
        dispatch.require_ledger(&self.ledger)?;
        writer::validate_request(&request)?;
        let (_, model_profile) = dispatch.regional_writer_settings()?;
        let (capture_operation, page) = capture.writer_page_input()?;
        let operation = writer::stage(capture_operation, "current-page");
        let store = self.ledger.store().clone();
        let owner = writer::Owner::acquire(&store).context("Writer is already active")?;

        let association = self.ledger.resident_answer_association(capture)?;
        let conversation = if let Some(binding) = &association {
            ensure!(
                binding.receipt().source.page == capture.page()
                    && binding.receipt().observed_page != capture.page(),
                "current-page Writer requires the source page"
            );
            binding.receipt().conversation
        } else {
            ensure!(
                self.blocked_unbound_insertion().is_none(),
                "an earlier Buddy page insertion needs reconciliation"
            );
            writer::stage(operation, "conversation")
        };
        let preparation = writer::stage(operation, "source-capture");
        ensure!(
            self.ledger
                .expected(Namespace::Conversation, preparation)?
                .0
                .is_empty(),
            "current-page Writer already started"
        );

        self.workflow.deliver_reader_acquisition()?;
        let deadline = Instant::now() + Duration::from_secs(300);
        self.require_same_resident_source(deadline)?;

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_millis()
            .try_into()?;
        if association.is_none() {
            self.ledger.create(
                conversation,
                writer::stage(operation, "conversation-create"),
                timestamp,
            )?;
        }
        let expected = self
            .ledger
            .expected(Namespace::Conversation, conversation)?;
        // Reserve the capture before recognition. A replay of this action cannot
        // submit the same image to the provider twice.
        let (original_png, source_sha256, expected) = {
            let (
                PreparedResidentCapture {
                    original_png,
                    evidence,
                    ..
                },
                prepared,
            ) = self.ledger.prepare_resident_historical_with_receipt(
                writer::stage(operation, "source-capture-admission"),
                expected,
                Turn {
                    id: preparation,
                    conversation,
                    exchange: operation,
                    sequence: 0,
                    role: Role::User,
                    mode: Mode::Writer,
                    outcome: Outcome::Prepared,
                    text: None,
                    sources: vec![],
                    correction_of: None,
                    created_ms: timestamp,
                    updated_ms: timestamp,
                    completion: None,
                },
                writer::stage(operation, "source-evidence"),
                capture,
            )?;
            let expected = ExpectedHeads(
                prepared
                    .current_root
                    .iter()
                    .map(|value| value.revision_id)
                    .collect(),
            );
            (original_png, evidence.original_png.sha256, expected)
        };

        let recognition_deadline = deadline - Duration::from_secs(150);
        let mut progress = || {
            ensure!(
                Instant::now() < recognition_deadline,
                "Writer recognition expired"
            );
            self.workflow.check_request_progress()?;
            ensure!(
                self.ledger
                    .expected(Namespace::Conversation, conversation)?
                    == expected,
                "Writer conversation changed during recognition"
            );
            Ok(())
        };
        let page_image = notebook::admit_target(page, original_png, &mut |_| progress())?;
        let handwriting = dispatch.handwriting_runtime(&self.ledger)?.map(|runtime| {
            runtime.capture(
                &store,
                &std::collections::BTreeSet::from([source_sha256]),
                &mut || progress().map_err(|_| crate::handwriting::ContextFailure::Canceled),
            )
        });
        ensure!(
            handwriting
                .as_ref()
                .is_none_or(|capture| capture.failure.is_none()),
            "Writer handwriting context unavailable"
        );
        let mut recognition = writer::ModelStage::new(&mut self.llm, recognition_deadline);
        let (normalized, _) = notebook::normalize_admitted(
            capture.document(),
            &page_image,
            model_profile,
            handwriting
                .as_ref()
                .and_then(|capture| capture.context.as_deref()),
            TranscriptionPolicy::WholePage,
            &mut recognition,
            &mut |_| progress(),
        );
        progress()?;
        let Normalized::Page(normalized) =
            normalized.map_err(|_| anyhow::anyhow!("current-page Writer normalization refused"))?
        else {
            anyhow::bail!("current-page Writer requires readable source notes");
        };
        ensure!(
            !normalized.document.units.is_empty(),
            "current-page Writer source is blank"
        );
        drop(page_image);

        self.require_same_resident_source(deadline)?;
        let request = writer::Request::Generate {
            conversation,
            operation,
            request,
            document: normalized.document,
            selection: TextScope::Page { page },
            guidance: None,
        };
        let memory = dispatch.local_memory();
        let mut produced = None;
        let mut generation =
            writer::ModelStage::new(&mut self.llm, deadline - Duration::from_secs(60));
        let report = writer::run_owned_with_memory(
            &self.ledger,
            &mut generation,
            &request,
            &self.personalization,
            memory.as_deref().map(|p| p as &dyn crate::memory::Provider),
            &mut || self.workflow.check_request_progress().is_err(),
            writer::OwnedRun {
                owner: &owner,
                expected_root: Some(expected),
                historical_images: false,
                produced: &mut produced,
            },
        );
        ensure!(
            report.status == writer::Status::Generated,
            "current-page Writer did not generate a fresh draft"
        );
        self.present_resident_writer_draft(
            conversation,
            capture,
            association.as_ref(),
            produced,
            true,
        )
    }

    pub(super) fn require_same_resident_source(&mut self, deadline: Instant) -> Result<()> {
        let resident = self
            .workflow
            .resident_source
            .as_mut()
            .context("installed Writer page owner unavailable")?;
        let health = self
            .workflow
            .service_health
            .as_mut()
            .context("installed Writer service health unavailable")?;
        ensure!(
            resident.captured_page_matches(health, deadline)?,
            "Source page or page order changed"
        );
        Ok(())
    }
}

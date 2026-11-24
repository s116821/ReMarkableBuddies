//! Installed whole-notebook Writer uses the saved source owner and local rasterizer.
use super::{
    current_page_writer::{require_same_resident_source, PreparedWriterCapture, WriterCapture},
    Dispatch, Orchestrator, Workflow,
};
use crate::{
    conversation::{Ledger, ResidentSourceCapture, SourceObservation},
    llm::LLMEngine,
    retrieval::notebook::{
        saved_render::{NotebookRaster, SavedPage},
        NotebookSourceReceipt,
    },
    storage::Uuid,
    workflow::{
        conversation_dispatch::{NotebookVisit, RequestScope},
        resident_output::{format_exchange, ResidentOutputText},
        saved_notebook::{
            acquire_saved_notebook, acquire_saved_notebook_with_pages, NotebookSourceAcquisition,
            SavedNotebookSource,
        },
    },
    writer,
};
use anyhow::{bail, ensure, Context, Result};
use remarkable_open_sdk::native_adapter::ReaderFacts;
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

const COVERAGE_UNAVAILABLE: &str =
    "Whole-notebook Writer needs complete ordered coverage of the notebook's source pages";

impl<M: LLMEngine> Orchestrator<M> {
    pub(super) fn run_resident_whole_notebook_writer(
        &mut self,
        request: String,
        capture: &ResidentSourceCapture,
        dispatch: &Dispatch<'_>,
    ) -> Result<()> {
        dispatch.require_ledger(&self.ledger)?;
        writer::validate_request(&request)?;
        let (_, model_profile) = dispatch.regional_writer_settings()?;
        let (facts, mut source) = capture.notebook_source();
        let order: Vec<_> = facts
            .order()
            .iter()
            .map(|page| Uuid::from_bytes(*page.as_bytes()))
            .collect();
        let acquisition_deadline = self
            .workflow
            .resident_source
            .as_mut()
            .context("installed Writer page owner unavailable")?
            .check_source_delivery(
                self.workflow
                    .service_health
                    .as_mut()
                    .context("installed Writer service health unavailable")?,
            )?;
        let mut raster = NotebookRaster::default();
        let saved = admitted_source(acquire_saved_notebook_with_pages(
            facts,
            &self.ledger,
            acquisition_deadline,
            &mut || self.workflow.check_request_progress(),
            &mut |page| {
                raster.push(
                    SavedPage {
                        id: Uuid::from_bytes(*page.page.as_bytes()),
                        scene: page.scene,
                        template_svg: page.template_svg,
                        template_png: page.template_png,
                    },
                    &mut || {
                        ensure!(
                            Instant::now() < acquisition_deadline,
                            "notebook render expired"
                        );
                        Ok(())
                    },
                )
            },
        ))?;
        let buddy_pages: BTreeSet<_> = facts
            .order()
            .iter()
            .filter(|id| !saved.original_pages().contains(id))
            .map(|id| Uuid::from_bytes(*id.as_bytes()))
            .collect();
        let reopened = read_saved_source(
            &mut self.workflow,
            &self.ledger,
            facts,
            acquisition_deadline,
        )?;
        ensure!(reopened == saved, "saved notebook changed during rendering");
        saved.bind_content_revision(&mut source)?;

        let (capture_operation, _) = capture.writer_page_input()?;
        let operation = writer::stage(capture_operation, "whole-notebook");
        let PreparedWriterCapture {
            association,
            conversation,
            expected,
            deadline,
            ..
        } = self.prepare_installed_writer_capture(
            capture,
            operation,
            writer::stage(operation, "source-capture"),
            WriterCapture::Draft,
        )?;
        let generation_deadline = deadline - Duration::from_secs(60);
        let (report, pending) = raster.images(source.document, &order, &buddy_pages, |images| {
            source.order_revision = Some(images.order_revision()?);
            let admitted = RequestScope {
                conversation,
                operation,
                root: expected,
                source: source.clone(),
            };
            let receipt = NotebookSourceReceipt {
                saved_source_revision: saved.revision().to_owned(),
                ordered_image_sha256: images.image_digest()?,
            };
            let mut engine = writer::ModelStage::new(&mut self.llm, generation_deadline);
            dispatch.whole_notebook(
                &self.ledger,
                &mut engine,
                &request,
                images,
                &receipt,
                model_profile,
                &admitted,
                &mut ResidentNotebookVisit {
                    workflow: &mut self.workflow,
                    ledger: &self.ledger,
                    facts,
                    source: &source,
                    saved: &saved,
                    deadline: generation_deadline,
                },
            )
        })?;
        drop(raster);
        let pending = pending.context("whole-notebook Writer did not generate a fresh draft")?;
        ensure!(
            report.status == writer::notebook::runner::Status::Generated,
            "whole-notebook Writer draft is unresolved"
        );
        let draft = pending.notebook_draft(&self.ledger)?;
        let (title, body) = writer::parse_draft(
            draft
                .text
                .as_deref()
                .context("notebook Writer draft text absent")?,
        )?;
        let text = format_exchange(&request, format_args!("{title}\n\n{body}"));
        self.require_same_resident_source(deadline)?;
        ensure!(
            read_saved_source(&mut self.workflow, &self.ledger, facts, deadline)? == saved,
            "saved notebook changed before placement"
        );
        pending.notebook_draft(&self.ledger)?;
        self.workflow.place_resident_writer_draft(
            &self.ledger,
            conversation,
            ResidentOutputText::WriterSourceDraft(&text),
            association.as_ref(),
        )
    }
}

struct ResidentNotebookVisit<'a> {
    workflow: &'a mut Workflow,
    ledger: &'a Ledger,
    facts: &'a ReaderFacts,
    source: &'a SourceObservation,
    saved: &'a SavedNotebookSource,
    deadline: Instant,
}

impl NotebookVisit for ResidentNotebookVisit<'_> {
    fn observe(&mut self) -> Result<SourceObservation> {
        let current = read_saved_source(self.workflow, self.ledger, self.facts, self.deadline)?;
        ensure!(
            current.original_pages() == self.saved.original_pages(),
            "notebook source page ownership changed"
        );
        require_same_resident_source(self.workflow, self.deadline)?;
        let mut observed = self.source.clone();
        current.bind_content_revision(&mut observed)?;
        Ok(observed)
    }

    fn progress(&mut self) -> Result<()> {
        self.workflow.check_request_progress()
    }
}

fn read_saved_source(
    workflow: &mut Workflow,
    ledger: &Ledger,
    facts: &ReaderFacts,
    deadline: Instant,
) -> Result<SavedNotebookSource> {
    admitted_source(acquire_saved_notebook(facts, ledger, deadline, &mut || {
        workflow.check_request_progress()
    }))
}

fn admitted_source(value: NotebookSourceAcquisition) -> Result<SavedNotebookSource> {
    match value {
        NotebookSourceAcquisition::SavedInputs(saved) => Ok(saved),
        NotebookSourceAcquisition::Unavailable(error) => {
            log::debug!("Whole-notebook saved source unavailable: {error}");
            bail!(COVERAGE_UNAVAILABLE)
        }
    }
}

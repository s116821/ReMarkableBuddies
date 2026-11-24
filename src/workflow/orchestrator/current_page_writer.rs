//! The installed Writer action for the observed source page.
use super::{Dispatch, Orchestrator};
use crate::{
    conversation::{
        ExpectedHeads, HistoricalPageAssociation, Mode, Outcome, PreparedResidentCapture,
        ResidentSourceCapture, Role, TextScope, Turn,
    },
    llm::LLMEngine,
    retrieval::notebook::{self, Normalized, TranscriptionPolicy},
    storage::{Namespace, Uuid},
    writer,
};
use anyhow::{ensure, Context, Result};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(super) enum NoteExport {
    CurrentPage,
    Separate,
}

pub(super) enum WriterCapture<'a> {
    Draft,
    Save,
    Clarification(&'a str),
}

const EXPORT_CLARIFICATION: &str =
    "Should I save a note from this page to your configured destination?";

fn source_conversation(capture: &ResidentSourceCapture) -> Uuid {
    writer::stage(
        crate::conversation::Ledger::binding_id(capture.document(), capture.page()),
        "source-page-conversation",
    )
}

fn source_note(capture: &ResidentSourceCapture) -> Uuid {
    writer::stage(source_conversation(capture), "current-page-note")
}

pub(super) struct PreparedWriterCapture {
    pub(super) association: Option<HistoricalPageAssociation>,
    pub(super) conversation: Uuid,
    pub(super) expected: ExpectedHeads,
    pub(super) capture: PreparedResidentCapture,
    pub(super) deadline: Instant,
}

impl<M: LLMEngine> Orchestrator<M> {
    pub(super) fn run_resident_current_page_writer(
        &mut self,
        request: String,
        capture: &ResidentSourceCapture,
        dispatch: &Dispatch<'_>,
        export: Option<NoteExport>,
    ) -> Result<()> {
        dispatch.require_ledger(&self.ledger)?;
        writer::validate_request(&request)?;
        let (_, model_profile) = dispatch.regional_writer_settings()?;
        let (capture_operation, page) = capture.writer_page_input()?;
        let operation = writer::stage(capture_operation, "current-page");
        let store = self.ledger.store().clone();
        let owner = writer::Owner::acquire(&store).context("Writer is already active")?;

        let PreparedWriterCapture {
            association,
            conversation,
            expected,
            capture:
                PreparedResidentCapture {
                    original_png,
                    evidence,
                    ..
                },
            deadline,
        } = self.prepare_installed_writer_capture(
            capture,
            operation,
            writer::stage(operation, "source-capture"),
            if export.is_some() {
                WriterCapture::Save
            } else {
                WriterCapture::Draft
            },
        )?;
        let source_sha256 = evidence.original_png.sha256;

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
        if let Some(export) = export {
            drop(owner);
            let selected = produced.context("fresh Writer selection unavailable")?;
            let (selected, result) = self.export_resident_page_draft(
                dispatch, capture, operation, selected, deadline, export,
            )?;
            self.present_resident_writer_result(
                conversation,
                capture,
                association.as_ref(),
                Some(selected),
                true,
                Some(result),
            )
        } else {
            self.present_resident_writer_draft(
                conversation,
                capture,
                association.as_ref(),
                produced,
                true,
            )
        }
    }

    pub(super) fn clarify_resident_export(
        &mut self,
        request: &str,
        capture: &ResidentSourceCapture,
    ) -> Result<()> {
        use super::super::resident_output::{format_exchange, ResidentOutputText};

        let (capture_operation, _) = capture.writer_page_input()?;
        let operation = writer::stage(capture_operation, "clarify-export");
        let prepared = self.prepare_installed_writer_capture(
            capture,
            operation,
            writer::stage(operation, "source-capture"),
            WriterCapture::Clarification(request),
        )?;
        self.require_same_resident_source(prepared.deadline)?;
        ensure!(
            self.ledger
                .expected(Namespace::Conversation, prepared.conversation)?
                == prepared.expected,
            "Clarification history changed before presentation"
        );
        let text = format_exchange(request, EXPORT_CLARIFICATION);
        self.workflow.place_resident_writer_draft(
            &self.ledger,
            prepared.conversation,
            ResidentOutputText::WriterSourceDraft(&text),
            prepared.association.as_ref(),
        )
    }

    fn export_resident_page_draft(
        &mut self,
        dispatch: &Dispatch<'_>,
        capture: &ResidentSourceCapture,
        operation: Uuid,
        selected: crate::conversation::WriterSelection,
        deadline: Instant,
        export: NoteExport,
    ) -> Result<(crate::conversation::WriterSelection, &'static str)> {
        use crate::note_export::dispatch::ConfiguredDestination;
        let config = dispatch.config;
        if !dispatch.admits(Mode::Writer, true) {
            return Ok((selected, "Draft saved. Note export is disabled."));
        }
        let saved = config.saved_revision();
        #[cfg(test)]
        if let Some(destination) = &dispatch.test_note_destination {
            return self.export_page_to_destination(
                capture,
                operation,
                selected,
                deadline,
                export,
                config,
                saved.as_ref(),
                &crate::note_export::dispatch::LegacyDestination(destination.as_ref()),
            );
        }
        let destination = match ConfiguredDestination::selected(
            config,
            saved.as_ref(),
            Some(self.ledger.store()),
        ) {
            Ok(destination) => destination,
            Err(_) => {
                return Ok((
                    selected,
                    "Draft saved. The configured note destination is unavailable.",
                ))
            }
        };
        self.export_page_to_destination(
            capture,
            operation,
            selected,
            deadline,
            export,
            config,
            saved.as_ref(),
            &destination,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn export_page_to_destination(
        &mut self,
        capture: &ResidentSourceCapture,
        operation: Uuid,
        selected: crate::conversation::WriterSelection,
        deadline: Instant,
        export: NoteExport,
        config: &crate::config::Config,
        saved: Option<&crate::config::SavedRevision>,
        destination: &impl crate::note_export::dispatch::OwnedDestination,
    ) -> Result<(crate::conversation::WriterSelection, &'static str)> {
        use crate::writer_flow::{Phase, Request, Status};

        let ledger = crate::conversation::Ledger::new(self.ledger.store().clone());
        let (draft, source) = selected.values()?;
        let conversation = draft.conversation;
        let correlation = match export {
            NoteExport::CurrentPage => {
                let current = source_note(capture);
                if ledger
                    .correlated_export_owner(current)?
                    .is_some_and(|owner| owner != conversation)
                {
                    return Ok((selected, "Draft saved. An earlier save is linked to another conversation. Check that export before retrying."));
                }
                let original = writer::stage(conversation, "current-page-note");
                let legacy = if original == current {
                    None
                } else {
                    ledger.export_association(
                        conversation,
                        destination.association_id(conversation, original),
                    )?
                };
                if let Some(prior) = legacy {
                    ensure!(
                        prior.backend == destination.backend()
                            && prior.source_scope == source.scope()
                            && ledger
                                .export_association(
                                    conversation,
                                    destination.association_id(conversation, current)
                                )?
                                .is_none(),
                        "Conflicting prior source-page exports"
                    );
                    original
                } else {
                    current
                }
            }
            NoteExport::Separate => writer::stage(operation, "separate-current-page-note"),
        };
        let request = Request::ExportDraft {
            conversation,
            draft: draft.id,
            correlation,
            outer_operation: writer::stage(operation, "save-note"),
            effect_operation: writer::stage(operation, "save-note-effect"),
        };
        let preference = config.personalization_snapshot()?;
        let check = |phase| -> Result<()> {
            self.require_same_resident_source(deadline)?;
            if phase == Phase::BeforeAdmission {
                ensure!(
                    ledger.expected(Namespace::Conversation, conversation)? == selected.root,
                    "Writer draft changed before export"
                );
            }
            ensure!(
                saved.is_none_or(|revision| revision.current()),
                "Note destination configuration changed"
            );
            Ok(())
        };
        let report = crate::writer_flow::run_guarded_draft_export(
            &ledger,
            destination,
            &request,
            &preference,
            check,
        );
        self.require_same_resident_source(deadline)?;
        let visible = ledger.select_writer_for_flow(
            conversation,
            draft.id,
            ledger.expected(Namespace::Conversation, conversation)?,
        )?;
        ensure!(
            selected.same_draft_and_source(&visible),
            "Writer draft changed during export"
        );
        let result = match report.status {
            Status::Exported | Status::HistoricalExport => {
                "Saved to your configured note destination."
            }
            Status::ReconcileRequired => {
                "The destination may have received the note. Check the destination before retrying."
            }
            _ => "Draft saved. Export could not be verified.",
        };
        Ok((visible, result))
    }

    pub(super) fn prepare_installed_writer_capture(
        &mut self,
        capture: &ResidentSourceCapture,
        operation: Uuid,
        preparation: Uuid,
        purpose: WriterCapture<'_>,
    ) -> Result<PreparedWriterCapture> {
        let association = self.ledger.resident_answer_association(capture)?;
        let conversation = if let Some(binding) = &association {
            ensure!(
                binding.receipt().source.page == capture.page()
                    && binding.receipt().observed_page != capture.page(),
                "Writer requires the source page"
            );
            binding.receipt().conversation
        } else {
            ensure!(
                self.blocked_unbound_insertion().is_none(),
                "an earlier Buddy page insertion needs reconciliation"
            );
            match purpose {
                WriterCapture::Draft => writer::stage(operation, "conversation"),
                WriterCapture::Save | WriterCapture::Clarification(_) => self
                    .ledger
                    .correlated_export_owner(source_note(capture))?
                    .unwrap_or_else(|| source_conversation(capture)),
            }
        };
        ensure!(
            self.ledger
                .expected(Namespace::Conversation, preparation)?
                .0
                .is_empty(),
            "Writer already started this submission"
        );
        self.workflow.deliver_reader_acquisition()?;
        let deadline = Instant::now() + Duration::from_secs(300);
        self.require_same_resident_source(deadline)?;

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_millis()
            .try_into()?;
        if association.is_none()
            && self
                .ledger
                .expected(Namespace::Conversation, conversation)?
                .0
                .is_empty()
        {
            self.ledger.create(
                conversation,
                writer::stage(operation, "conversation-create"),
                timestamp,
            )?;
        }
        let expected = self
            .ledger
            .expected(Namespace::Conversation, conversation)?;
        let admission = writer::stage(operation, "source-capture-admission");
        let turn = Turn {
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
        };
        let evidence = writer::stage(operation, "source-evidence");
        let (capture, prepared) = match purpose {
            WriterCapture::Clarification(request) => {
                self.ledger.prepare_resident_writer_clarification(
                    admission,
                    expected,
                    turn,
                    evidence,
                    capture,
                    (request, EXPORT_CLARIFICATION),
                )?
            }
            WriterCapture::Draft | WriterCapture::Save => {
                self.ledger.prepare_resident_historical_with_receipt(
                    admission, expected, turn, evidence, capture,
                )?
            }
        };
        let expected = ExpectedHeads(
            prepared
                .current_root
                .iter()
                .map(|value| value.revision_id)
                .collect(),
        );
        Ok(PreparedWriterCapture {
            association,
            conversation,
            expected,
            capture,
            deadline,
        })
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

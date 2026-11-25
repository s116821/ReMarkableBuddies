//! Ordinary Writer generation for the circled target on a newly captured source page.
use super::{
    current_page_writer::{
        NoteExport, NoteScope, PreparedWriterCapture, ResidentSave, WriterCapture,
    },
    Dispatch, Orchestrator,
};
use crate::{
    conversation::{target, Outcome, ResidentSourceCapture},
    llm::LLMEngine,
    storage::Namespace,
    writer,
};
use anyhow::{ensure, Context, Result};
use std::time::{SystemTime, UNIX_EPOCH};

impl<M: LLMEngine> Orchestrator<M> {
    pub(super) fn run_resident_circled_writer(
        &mut self,
        request: String,
        candidate: target::Candidate,
        capture: &ResidentSourceCapture,
        dispatch: &Dispatch<'_>,
        export: Option<NoteExport>,
    ) -> Result<()> {
        dispatch.require_ledger(&self.ledger)?;
        writer::validate_request(&request)?;
        ensure!(
            matches!(candidate.understanding, target::Understanding::Interpreted),
            "Writer requires an unambiguous circled target"
        );
        let (config, model_profile) = dispatch.regional_writer_settings()?;
        let (capture_operation, _) = capture.writer_page_input()?;
        let operation = writer::stage(capture_operation, "circled");
        let user_id = writer::stage(operation, "user");
        let region = export
            .as_ref()
            .map(|_| {
                let geometry = candidate
                    .geometry
                    .clone()
                    .context("circled geometry absent")?;
                capture.installed_region(geometry, &mut || self.workflow.check_request_progress())
            })
            .transpose()?;
        let store = self.ledger.store().clone();
        let owner = writer::Owner::acquire(&store).context("Writer is already active")?;

        let PreparedWriterCapture {
            association,
            conversation,
            expected,
            capture: retained,
            deadline,
        } = self.prepare_installed_writer_capture(
            capture,
            operation,
            user_id,
            region.as_ref().map_or(WriterCapture::Draft, |region| {
                WriterCapture::Save(NoteScope::Region(region.region.id))
            }),
        )?;
        let capture_id = retained.evidence.id;
        drop(retained);
        let mut frame = self.ledger.target_source_frame(conversation, capture_id)?;
        ensure!(
            frame.expected_root() == expected,
            "Writer source root changed"
        );
        let mut user = self.ledger.writer_turn(user_id, conversation)?;
        user.outcome = Outcome::Interpreted;
        user.text = Some(request.clone());
        user.updated_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_millis()
            .try_into()?;
        self.require_same_resident_source(deadline)?;
        self.ledger.advance_with_source_target(
            writer::stage(operation, "target-interpretation"),
            self.ledger.expected(Namespace::Conversation, user_id)?,
            user.clone(),
            &mut frame,
            &candidate,
            None,
        )?;
        let expected_root = frame.expected_root();
        drop(frame);
        let target = self
            .ledger
            .current_resident_writer_target(conversation, capture.document(), capture.page())?
            .context("Fresh Writer target absent")?;
        let request = writer::target::Request::GenerateTarget {
            conversation,
            operation,
            expected_root,
            target,
            request,
            guidance: None,
        };
        self.require_same_resident_source(deadline)?;
        let mut produced = None;
        let report = writer::target::run_owned(
            &self.ledger,
            &mut self.llm,
            &request,
            &mut || self.workflow.check_request_progress().is_err(),
            writer::target::TargetRun {
                config,
                model_profile,
                owner: &owner,
                deadline,
                accepted: Some(&user),
                installed_region: region.as_ref(),
            },
            &mut produced,
        );
        ensure!(
            report.status == writer::Status::Generated,
            "circled Writer did not generate a fresh draft"
        );
        self.require_same_resident_source(deadline)?;
        if let Some((export, prepared)) = export.zip(region) {
            let region = prepared.region;
            drop(owner);
            let selected = produced.context("fresh regional Writer selection unavailable")?;
            let updating = matches!(export, NoteExport::Update);
            let (selected, result) = if updating
                && self
                    .ledger
                    .installed_region_conflict(&region)
                    .unwrap_or(true)
            {
                (
                    selected,
                    "Draft saved. I couldn't match this selection to an earlier save. Check the earlier note or ask for a separate one.",
                )
            } else {
                self.export_resident_draft(
                    dispatch,
                    ResidentSave {
                        capture,
                        operation,
                        scope: NoteScope::Region(region.id),
                        export,
                    },
                    selected,
                    deadline,
                    |_, ledger| {
                        ensure!(
                            !updating || !ledger.installed_region_conflict(&region)?,
                            "regional export overlap changed before writing"
                        );
                        Ok(())
                    },
                )?
            };
            return self.present_resident_writer_result(
                conversation,
                capture,
                association.as_ref(),
                Some(selected),
                true,
                Some(result),
            );
        }
        self.present_resident_writer_draft(
            conversation,
            capture,
            association.as_ref(),
            produced,
            true,
        )
    }
}

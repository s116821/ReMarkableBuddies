use anyhow::Result;
use log::{debug, error, info};

use super::conversation_dispatch::Dispatch;
use super::reader_attempt::Attempt;
use super::{indicator::Failure, AnswerPageType, ReturnOutcome, Workflow};
use crate::analysis::{
    BoundingBox, ReaderSelection, ReaderSourceSubject, ReaderSubject, SelectionCenter,
};
use crate::config::Config;
use crate::conversation::{AttemptReason, Ledger, Outcome};
use crate::device::screenshot::{SCREENSHOT_VIRTUAL_HEIGHT, SCREENSHOT_VIRTUAL_WIDTH};
use crate::llm::{openai::OpenAI, LLMEngine};
use base64::{engine::general_purpose::STANDARD, Engine};
#[cfg(all(target_os = "linux", feature = "installed-resident-output"))]
mod circled_writer;
#[cfg(all(target_os = "linux", feature = "installed-resident-output"))]
mod current_page_writer;
#[cfg(all(target_os = "linux", feature = "installed-resident-output"))]
mod ordinary_pdf;
#[cfg(target_os = "linux")]
mod researched;
#[cfg(all(target_os = "linux", feature = "installed-resident-output"))]
mod source_submission;
#[cfg(all(test, target_os = "linux", feature = "development-resident-bootstrap"))]
pub(crate) use researched::fixture as researched_fixture;

/// Shared by the live workflow and bounded vision-comparison helper.
pub const ANALYSIS_PROMPT: &str = "Read the handwritten question on this tablet page and answer it \
             using the content selected by a hand-drawn closed outline or a deliberate highlight. \
             The source may be handwritten or printed and may include text, equations, or diagrams. \
             A highlight can select content without any surrounding outline and may appear gray \
             on a grayscale tablet. Handwriting may be neat uppercase block letters, connected \
             cursive, or abbreviated shorthand. The question may be near the selection, in a \
             margin, or inside the same outline.\n\
             The first image is the full-page overview; \
             the following three images are overlapping full-width detail strips in \
             top-to-bottom order. They show the SAME page.\n\
             Transcribe only the question as written, retaining abbreviations. Distinguish the \
             question from selected content and surrounding notes even when all are handwritten. \
             A nearby question may refer to the selection without repeating its contents. Use \
             the visible selection and placement to identify a clear referent, but do not add \
             that content to the transcription. If there is no readable handwritten question, \
             no clear user outline or highlight, or the question or its referent is ambiguous, \
             return NONE. If several distinct topics are marked and the question's association \
             is unclear, return NONE rather than choosing a topic arbitrarily. \
             Do not guess missing question words or confidently answer an uncertain reading. \
             Cross-check the question in the overview and full-width detail before answering. \
             Resolve shorthand only if its meaning is clear from the visible words and source. \
             If any essential word or abbreviation is unreadable or has competing meanings, \
             return NONE. Never substitute a general summary for an answer to an unclear question.\n\
             The selection identifies the subject. You may use the surrounding page and general \
             knowledge to answer the actual question. Distinguish general explanation from \
             claims about the source. Preserve source-specific numbers, uncertainties and units \
             rather than substituting remembered values or inventing measurements. \
             If a source detail is unreadable, say so rather than guessing.\n\
             An X mark, printed gray figure or document shading is not a user selection. \
             Do not infer a selection from these.\n\
             Reply in this exact format, with coordinates in the overview's 768x1024 space:\n\
             QUESTION: [question]\n\
             QUESTION_BOX: x,y,width,height\n\
             SELECTION_CENTER: x,y\n\
             ---\n\
             ANSWER: [concise answer about the selected content]\n\
             SELECTION_CENTER is the approximate center of the selected outlined OR highlighted content; \
             use the full-page overview pixel frame, not the question location or detail-strip coordinates.\n\
             Use plain ASCII notation: +/- for uncertainty, * for multiplication, ^ for powers, \
             spelled-out Greek letters. No LaTeX or Markdown.";

const MARKED_QUESTION_PROMPT: &str = "For this handwritten source request, add exactly one header field HANDWRITTEN_QUESTION_MARK: present, absent, or unclear. Use present only when a visible handwritten question mark belongs to the same fragment in QUESTION, anywhere within that fragment. Keep its written text, punctuation, and shorthand, and give one tight QUESTION_BOX for the fragment in the overview. Ignore printed question marks and marks in other writing. If you can read a marked fragment and localize it but cannot answer, retain QUESTION and QUESTION_BOX with the mark field and write ANSWER: NONE after the separator. For all other cases follow the original answer instructions.";

const SOURCE_SUBJECT_FIELD: &str = "SELECTION_SUBJECT:";

const SOURCE_SUBJECT_PROMPT: &str = "For this request, include one additional header line \
    before the --- separator: SELECTION_SUBJECT: [short description of the marked source]. \
    Name the selected content using only what is visibly supported by the images. \
    Describe the selected subject, not the handwritten question or the answer. \
    Use a single short phrase in plain ASCII, at most 120 characters. \
    If you cannot name the selected subject reliably, write SELECTION_SUBJECT: NONE. Keep the original \
    whole-response NONE rules for an absent or ambiguous question or selection.";

/// Reader proposal before independent question verification.
struct AnalysisResult {
    question: String,
    answer: String,
    _question_box: Option<BoundingBox>,
    selection: ReaderSelection,
    target: Option<crate::conversation::target::Candidate>,
    about: Option<String>,
}

struct MarkedFragment {
    text: String,
    region: [i32; 4],
    target: Option<crate::conversation::target::Candidate>,
}

fn encode_model_images(images: Vec<Vec<u8>>) -> Vec<String> {
    images
        .into_iter()
        .map(|image| STANDARD.encode(image))
        .collect()
}

/// High-level orchestrator for the complete workflow
pub struct Orchestrator<M: LLMEngine = OpenAI> {
    workflow: Workflow,
    llm: M,
    trigger_enabled: bool,
    ledger: Ledger,
    memory: Option<std::sync::Arc<dyn crate::memory::Provider>>,
    personalization: crate::config::PersonalizationSnapshot,
    handwriting: Option<crate::handwriting::runtime::Runtime>,
    handwriting_operation: Option<crate::handwriting::runtime::Capture>,
    #[cfg(all(target_os = "linux", feature = "installed-resident-output"))]
    ordinary_book_cache: Option<super::resident_source::original_pdf::OrdinaryBookCache>,
}
struct ModelContent<'a, M: LLMEngine>(&'a mut M);
impl<M: LLMEngine> Drop for ModelContent<'_, M> {
    fn drop(&mut self) {
        self.0.clear_content();
    }
}

enum RenderOutcome {
    NoOutput(AttemptReason),
    SubmittedUnverified,
}

impl<M: LLMEngine> Orchestrator<M> {
    #[cfg(any(target_os = "linux", test))]
    pub(crate) fn receive_resident_prepared(
        &mut self,
        token: crate::storage::selection::SelectionToken,
        acknowledgment: crate::conversation::selected_reader_prepare::ResidentPreparedAcknowledgment,
        policy: crate::storage::sync::selected::ReaderPreferences,
    ) -> Result<()> {
        let mut attempt =
            Attempt::prepare_resident(&self.ledger, &self.workflow, token, acknowledgment, policy)?;
        self.run_prepared(&mut attempt)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn into_workflow(self) -> Workflow {
        self.workflow
    }

    /// Continue an explicitly acknowledged selected input. This consumes current
    /// stored data only; installed acquisition and native output remain separate.
    pub fn run_selected_prepared(
        &mut self,
        token: crate::storage::selection::SelectionToken,
        root: crate::conversation::IntentRecordRef,
        user: crate::conversation::IntentRecordRef,
        capture: crate::storage::Uuid,
    ) -> Result<()> {
        anyhow::ensure!(
            self.handwriting.is_none(),
            "confirmed handwriting requires ordinary local Reader"
        );
        if let Some(memory) = &self.memory {
            let admission = self.workflow.selected_admission()?;
            memory.require_selected_reader(admission.as_ref())?;
        }
        let mut attempt =
            Attempt::prepare_selected(&self.ledger, &self.workflow, token, root, user, capture)?;
        self.workflow.begin_iteration()?;
        attempt.check_selected()?;
        self.run_prepared(&mut attempt)
    }
    pub fn new(workflow: Workflow, llm: M, ledger: Ledger) -> Self {
        Self {
            workflow,
            llm,
            trigger_enabled: true,
            ledger,
            memory: None,
            handwriting: None,
            handwriting_operation: None,
            #[cfg(all(target_os = "linux", feature = "installed-resident-output"))]
            ordinary_book_cache: None,
            personalization: crate::config::Personalization::default()
                .snapshot()
                .expect("valid default preference"),
        }
    }

    pub fn set_trigger_enabled(&mut self, enabled: bool) {
        self.trigger_enabled = enabled;
    }

    /// Startup preference only; each request retains an immutable snapshot.
    pub fn set_personalization(&mut self, snapshot: crate::config::PersonalizationSnapshot) {
        self.personalization = snapshot;
    }

    /// Ordinary local context only.
    pub fn set_subject_memory(
        &mut self,
        memory: Option<std::sync::Arc<crate::memory::LocalMemory>>,
    ) {
        self.memory =
            memory.map(|provider| provider as std::sync::Arc<dyn crate::memory::Provider>);
    }

    /// Set a provider for guarded selected publication. Admission still checks
    /// its exact receiving Store and selection before any model work.
    pub fn set_selected_subject_memory(
        &mut self,
        memory: std::sync::Arc<dyn crate::memory::Provider>,
    ) {
        self.memory = Some(memory);
    }

    pub fn invalidate_history(&mut self) {
        self.workflow.invalidate_history();
    }
    pub fn history_state(&self) -> super::history::State {
        self.workflow.history_state()
    }
    pub fn history_action(&mut self, action: super::history::Action) -> Result<bool> {
        self.workflow.history_action(action)
    }

    /// Run one complete iteration of the reader buddy workflow
    /// Processes one outlined/highlighted concept and question per trigger.
    pub fn run_iteration(&mut self) -> Result<()> {
        let _run = crate::measurement::Run::new();
        self.run_iteration_with_trigger(self.trigger_enabled, None, None)
    }

    pub(super) fn run_iteration_with_trigger(
        &mut self,
        wait_for_trigger: bool,
        local: Option<&Dispatch<'_>>,
        config: Option<&Config>,
    ) -> Result<()> {
        anyhow::ensure!(
            self.handwriting_operation.is_none(),
            "prior handwriting operation unsettled"
        );
        let result = (|| -> Result<()> {
            let _timing = crate::measurement::Span::new("reader.iteration");
            self.workflow.begin_iteration()?;
            let result = self.run_iteration_inner(wait_for_trigger, local, config);
            // A failed input guard or service health cannot be cleared by cleanup.
            if self.workflow.runtime_failed() {
                return result.and(Err(anyhow::anyhow!(
                    "Runtime health or input lost; further activity stopped"
                )));
            }
            if let Err(error) = self.workflow.finish_request() {
                log::warn!("Request finalization failed: {error:#}");
                return result.and(Err(error));
            }
            if result.is_err() {
                self.workflow.report_failure(Failure::Device)?;
            }
            result
        })();
        // Keep successful/refused capture ownership through finalization and failure output.
        self.handwriting_operation = None;
        result
    }

    fn run_iteration_inner(
        &mut self,
        wait_for_trigger: bool,
        local: Option<&Dispatch<'_>>,
        config: Option<&Config>,
    ) -> Result<()> {
        #[cfg(not(target_os = "linux"))]
        let _ = local;
        info!("=== Starting Reader Buddy Iteration ===");

        // Step 1: Wait for trigger
        if wait_for_trigger {
            let _timing = crate::measurement::Span::new("reader.trigger_wait");
            self.workflow.wait_for_trigger()?;
        }
        let _active = crate::measurement::Span::new("reader.active");
        log::debug!(
            "timing_origin run={} origin={}",
            crate::measurement::context(),
            if self.trigger_enabled {
                "trigger_observed"
            } else {
                "immediate_capture"
            }
        );

        // Step 2: Capture screenshot (of current/question page)
        #[cfg(all(target_os = "linux", feature = "installed-resident-output"))]
        let (acquired, observed_book) = ordinary_pdf::acquire(&mut self.workflow, local)?;
        #[cfg(not(all(target_os = "linux", feature = "installed-resident-output")))]
        let acquired = self.workflow.acquire_reader_evidence()?;
        #[cfg(target_os = "linux")]
        if let Some(request) = self.workflow.selected_resident_conversation(
            &acquired,
            config.is_none_or(|settings| settings.admits_writer(false)),
        )? {
            use super::resident_output::SelectedConversationRequest;
            let super::AcquiredEvidence::ResidentHistorical(capture) = acquired else {
                anyhow::bail!("selected conversation requires the retained native source");
            };
            let writer_memory =
                request.installed_writer_memory(self.ledger.store(), self.memory.as_ref())?;
            return match request {
                SelectedConversationRequest::UnsupportedRegionalWriter => {
                    anyhow::bail!("fresh regional Writer requires the selected receiver")
                }
                SelectedConversationRequest::WriterCurrentTarget(request) => {
                    let dispatch = local.ok_or_else(|| {
                        anyhow::anyhow!("current-target Writer requires ordinary local dispatch")
                    })?;
                    self.run_resident_current_target_writer(request, &capture, dispatch)
                }
                SelectedConversationRequest::BlankStart(conversation) => {
                    self.run_blank_start(conversation, &capture)
                }
                SelectedConversationRequest::WriterRefinement {
                    request,
                    require_current_draft,
                } => self.run_resident_writer_refinement(
                    &request,
                    &capture,
                    require_current_draft,
                    writer_memory.as_deref(),
                ),
            };
        }
        #[cfg(all(target_os = "linux", feature = "installed-resident-output"))]
        if let (Some(dispatch), super::AcquiredEvidence::ResidentHistorical(capture)) =
            (local, &acquired)
        {
            if self.workflow.uses_installed_reader_history()
                && dispatch.shared_source_writer_enabled()
            {
                use source_submission::SourceIntent;
                match self.classify_source_submission(capture, dispatch)? {
                    SourceIntent::Reader => {}
                    SourceIntent::WriterCurrentPage { request } => {
                        return self
                            .run_resident_current_page_writer(request, capture, dispatch, None);
                    }
                    SourceIntent::WriterExportCurrentPage {
                        request,
                        separate_note,
                    } => {
                        use current_page_writer::NoteExport;
                        let export = if separate_note {
                            NoteExport::Separate
                        } else {
                            NoteExport::CurrentPage
                        };
                        return self.run_resident_current_page_writer(
                            request,
                            capture,
                            dispatch,
                            Some(export),
                        );
                    }
                    SourceIntent::ClarifyExport { request } => {
                        return self.clarify_resident_export(&request, capture);
                    }
                    SourceIntent::WriterCircled { request, target } => {
                        return self
                            .run_resident_circled_writer(request, target, capture, dispatch);
                    }
                    SourceIntent::Unresolved => {
                        self.workflow.report_failure(Failure::Selection)?;
                        return Ok(());
                    }
                }
            }
        }
        if config.is_some_and(|settings| !settings.reader_enabled()) {
            return Ok(());
        }
        #[cfg(all(target_os = "linux", feature = "installed-resident-output"))]
        let observed_book = ordinary_pdf::classify(&mut self.workflow, observed_book?)?;
        let mut attempt = Attempt::prepare(&self.ledger, acquired)?;
        #[cfg(all(target_os = "linux", feature = "installed-resident-output"))]
        let result = match observed_book {
            Some(book) => self.run_ordinary_book(
                &mut attempt,
                book,
                local.ok_or_else(|| anyhow::anyhow!("Ordinary Reader dispatch absent"))?,
            ),
            None => self.run_prepared(&mut attempt),
        };
        #[cfg(not(all(target_os = "linux", feature = "installed-resident-output")))]
        let result = self.run_prepared(&mut attempt);
        match &result {
            Err(error) if attempt.user.outcome == Outcome::Prepared => {
                if let Err(recording) = attempt.outcome(
                    &self.ledger,
                    false,
                    if self.workflow.input_failed() {
                        Outcome::Canceled
                    } else {
                        Outcome::Failed
                    },
                    if self.workflow.input_failed() {
                        AttemptReason::Canceled
                    } else {
                        AttemptReason::RequestFailed
                    },
                ) {
                    return Err(anyhow::anyhow!(
                        "{}; terminal recording failed: {recording:#}",
                        error
                    ));
                }
            }
            _ => {}
        }
        result
    }
    #[cfg(target_os = "linux")]
    fn run_blank_start(
        &mut self,
        selected_conversation: Option<crate::storage::Uuid>,
        capture: &crate::conversation::ResidentSourceCapture,
    ) -> Result<()> {
        use super::resident_output::ResidentOutputOutcome;
        use crate::storage::{Namespace, Uuid};

        let association = self.ledger.resident_answer_association(capture)?;
        let conversation = selected_conversation
            .or_else(|| {
                association
                    .as_ref()
                    .map(|binding| binding.receipt().conversation)
            })
            .unwrap_or_else(Uuid::new_v4);
        if let Some(association) = &association {
            anyhow::ensure!(
                association.receipt().conversation == conversation,
                "blank start target belongs to a different conversation"
            );
        } else {
            // An existing unbound root can represent a previously interrupted
            // insertion. Only a fresh root may start a new blank page.
            anyhow::ensure!(
                self.ledger
                    .expected(Namespace::Conversation, conversation)?
                    .0
                    .is_empty(),
                "blank start already exists without a resolved page binding"
            );
            #[cfg(feature = "installed-resident-output")]
            if let Some(original) = self.blocked_unbound_insertion() {
                self.workflow.check_request_progress()?;
                anyhow::bail!(
                    "Blank start cannot insert a page until original operation {} is reconciled",
                    original.0
                );
            }
            let created_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis()
                .try_into()?;
            self.ledger
                .create(conversation, Uuid::new_v4(), created_ms)?;
        }
        match self.workflow.render_resident_answer(
            &self.ledger,
            conversation,
            super::resident_output::ResidentOutputText::Plain(""),
            association.as_ref(),
            &mut || Ok(()),
        )? {
            ResidentOutputOutcome::SubmittedUnverified => {
                info!("Blank conversation page submitted")
            }
            ResidentOutputOutcome::ReusedWithoutWrite => {
                info!("Blank conversation reopened on existing bound page")
            }
            ResidentOutputOutcome::InsertionRefused => {
                anyhow::bail!("blank conversation page insertion was refused");
            }
            ResidentOutputOutcome::Unavailable | ResidentOutputOutcome::Unresolved => {
                anyhow::bail!("blank conversation page placement unresolved");
            }
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn run_resident_writer_refinement(
        &mut self,
        request: &crate::writer::Request,
        capture: &crate::conversation::ResidentSourceCapture,
        require_current_draft: bool,
        memory: Option<&dyn crate::memory::Provider>,
    ) -> Result<()> {
        use crate::{storage::Namespace, writer};

        let writer::Request::Refine {
            conversation,
            draft,
            ..
        } = request
        else {
            anyhow::bail!("native Writer placement requires a refinement request");
        };
        let store = self.ledger.store().clone();
        let owner = writer::Owner::acquire(&store)
            .ok_or_else(|| anyhow::anyhow!("Writer is already active"))?;
        let expected = self
            .ledger
            .expected(Namespace::Conversation, *conversation)?;
        if require_current_draft {
            use crate::conversation::{Mode, Outcome, Record, Role};
            let records = self.ledger.inspect(*conversation, false)?;
            let latest = records
                .iter()
                .filter_map(|record| match record {
                    Record::Turn(turn)
                        if turn.mode == Mode::Writer
                            && turn.role == Role::Assistant
                            && turn.outcome == Outcome::Generated =>
                    {
                        Some(turn)
                    }
                    _ => None,
                })
                .max_by_key(|turn| turn.sequence);
            anyhow::ensure!(
                latest.is_some_and(|turn| turn.id == *draft),
                "Writer tablet request no longer names the current draft"
            );
        }
        let selection =
            self.ledger
                .select_writer_for_flow(*conversation, *draft, expected.clone())?;
        let (_, source) = selection.values()?;
        anyhow::ensure!(
            source.document.document == capture.document(),
            "Writer draft belongs to a different source document"
        );
        let association_before = self.ledger.resident_answer_association(capture)?;
        anyhow::ensure!(
            association_before
                .as_ref()
                .is_none_or(|association| association.receipt().conversation == *conversation),
            "Writer target belongs to a different conversation"
        );
        if require_current_draft {
            self.workflow.check_request_progress()?;
            #[cfg(all(
                feature = "development-resident-bootstrap",
                feature = "development-input-diagnostics"
            ))]
            let resident = {
                let resident =
                    self.workflow.resident_source.as_mut().ok_or_else(|| {
                        anyhow::anyhow!("installed Writer page owner unavailable")
                    })?;
                let health = self.workflow.service_health.as_mut().ok_or_else(|| {
                    anyhow::anyhow!("installed Writer service health unavailable")
                })?;
                resident.require_writer_page_current(health)?;
                resident
            };
            anyhow::ensure!(
                self.ledger
                    .expected(Namespace::Conversation, *conversation)?
                    == expected,
                "Writer conversation changed before claiming the current draft"
            );
            #[cfg(all(
                feature = "development-resident-bootstrap",
                feature = "development-input-diagnostics"
            ))]
            anyhow::ensure!(
                resident
                    .installed_writer_for_page(
                        capture,
                        super::resident_supervisor::writer_request::SlotAction::Claim
                    )?
                    .is_some(),
                "Writer request changed or expired before claiming the current draft"
            );
            info!(
                "Installed Writer refinement accepted: {}",
                request.operation()
            );
        }
        let mut produced = None;
        let report = writer::run_owned_with_memory(
            &self.ledger,
            &mut self.llm,
            request,
            &self.personalization,
            memory,
            &mut || self.workflow.check_request_progress().is_err(),
            writer::OwnedRun {
                owner: &owner,
                expected_root: Some(expected),
                // Preserve resident refinement's delivered text-only contract;
                // its current capture authorizes placement, not image opt-in.
                historical_images: false,
                produced: &mut produced,
            },
        );
        anyhow::ensure!(
            report.status == writer::Status::Generated,
            "Writer refinement did not generate a fresh draft"
        );
        self.present_resident_writer_draft(
            *conversation,
            capture,
            association_before.as_ref(),
            produced,
            false,
        )
    }

    #[cfg(target_os = "linux")]
    fn run_resident_current_target_writer(
        &mut self,
        request: crate::writer::target::CurrentRequest,
        capture: &crate::conversation::ResidentSourceCapture,
        dispatch: &super::conversation_dispatch::Dispatch<'_>,
    ) -> Result<()> {
        use crate::writer;

        dispatch.require_ledger(&self.ledger)?;
        let (config, model_profile) = dispatch.regional_writer_settings()?;
        let association = self
            .ledger
            .resident_answer_association(capture)?
            .ok_or_else(|| {
                anyhow::anyhow!("current-target Writer requires a bound conversation")
            })?;
        let receipt = association.receipt();
        let conversation = receipt.conversation;
        let request = request.resolve(
            &self.ledger,
            conversation,
            receipt.source.document,
            receipt.source.page,
        )?;
        let store = self.ledger.store().clone();
        let owner = writer::Owner::acquire(&store)
            .ok_or_else(|| anyhow::anyhow!("Writer is already active"))?;
        self.workflow.deliver_reader_acquisition()?;
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
                deadline: std::time::Instant::now() + std::time::Duration::from_secs(300),
                accepted: None,
            },
            &mut produced,
        );
        anyhow::ensure!(
            report.status == writer::Status::Generated,
            "current-target Writer did not generate a fresh draft"
        );
        self.present_resident_writer_draft(
            conversation,
            capture,
            Some(&association),
            produced,
            false,
        )
    }

    #[cfg(target_os = "linux")]
    fn present_resident_writer_draft(
        &mut self,
        conversation: crate::storage::Uuid,
        capture: &crate::conversation::ResidentSourceCapture,
        association_before: Option<&crate::conversation::HistoricalPageAssociation>,
        produced: Option<crate::conversation::WriterSelection>,
        protect_source: bool,
    ) -> Result<()> {
        self.present_resident_writer_result(
            conversation,
            capture,
            association_before,
            produced,
            protect_source,
            None,
        )
    }

    #[cfg(target_os = "linux")]
    #[allow(clippy::too_many_arguments)]
    fn present_resident_writer_result(
        &mut self,
        conversation: crate::storage::Uuid,
        capture: &crate::conversation::ResidentSourceCapture,
        association_before: Option<&crate::conversation::HistoricalPageAssociation>,
        produced: Option<crate::conversation::WriterSelection>,
        protect_source: bool,
        result: Option<&str>,
    ) -> Result<()> {
        use super::resident_output::{format_exchange, ResidentOutputText};
        use crate::{conversation::Role, storage::Namespace, writer};
        use anyhow::Context;

        let selected = produced.context("fresh Writer selection unavailable")?;
        let (draft, source) = selected.values()?;
        // A refinement reuses the note's original source. Its request belongs to
        // the new draft's exchange, even when several drafts share that source.
        let user = self
            .ledger
            .writer_turn(writer::stage(draft.exchange, "user"), conversation)?;
        anyhow::ensure!(
            user.exchange == draft.exchange
                && user.role == Role::User
                && user.outcome == Outcome::Interpreted
                && user.sequence < draft.sequence,
            "Writer request does not precede the selected draft in its exchange"
        );
        anyhow::ensure!(
            source.document.document == capture.document()
                && self
                    .ledger
                    .expected(Namespace::Conversation, conversation)?
                    == selected.root,
            "Writer draft changed before page placement"
        );
        let association = self.ledger.resident_answer_association(capture)?;
        anyhow::ensure!(
            association.as_ref().map(|a| a.binding_revision())
                == association_before.map(|a| a.binding_revision()),
            "Writer page association changed during generation"
        );
        let request = user
            .text
            .as_deref()
            .context("Writer request text unavailable")?;
        let (title, body) = writer::parse_draft(
            draft
                .text
                .as_deref()
                .context("Writer draft text unavailable")?,
        )?;
        let text = if let Some(result) = result {
            format_exchange(request, format_args!("{result}\n\n{title}\n\n{body}"))
        } else {
            format_exchange(request, format_args!("{title}\n\n{body}"))
        };
        let output = if protect_source {
            ResidentOutputText::WriterSourceDraft(&text)
        } else {
            ResidentOutputText::Plain(&text)
        };
        self.workflow.place_resident_writer_draft(
            &self.ledger,
            conversation,
            output,
            association.as_ref(),
        )
    }

    #[cfg(all(target_os = "linux", feature = "installed-resident-output"))]
    fn blocked_unbound_insertion(&self) -> Option<remarkable_open_sdk::OperationId> {
        self.workflow
            .resident_source
            .as_ref()?
            .blocked_unbound_insertion()
    }

    fn prepare_model_images(&mut self, attempt: &mut Attempt) -> Result<Vec<Vec<u8>>> {
        anyhow::ensure!(
            self.handwriting_operation.is_none(),
            "handwriting context already captured for this operation"
        );
        #[cfg(all(target_os = "linux", feature = "installed-resident-output"))]
        if attempt.resident_output() && attempt.resident_answer.is_none() {
            if let Some(original) = self.blocked_unbound_insertion() {
                self.workflow.check_request_progress()?;
                attempt.outcome(
                    &self.ledger,
                    false,
                    Outcome::Failed,
                    AttemptReason::NativeOutputUnavailable,
                )?;
                anyhow::bail!(
                    "Reader cannot insert an answer page until original operation {} is reconciled",
                    original.0
                );
            }
        }
        let images = attempt.images(&self.ledger)?;
        anyhow::ensure!(!images.is_empty(), "prepared overview absent");
        #[cfg(any(target_os = "linux", test))]
        if attempt.selected_resident() {
            let targets = images
                .iter()
                .map(|image| crate::storage::digest(image))
                .collect();
            attempt.capture_selected_handwriting(&targets)?;
        }
        let handwriting = self.handwriting.as_ref().map(|runtime| {
            let targets = images
                .iter()
                .map(|image| crate::storage::digest(image))
                .collect();
            runtime.capture(self.ledger.store(), &targets, &mut || {
                attempt
                    .check_selected()
                    .and_then(|_| self.workflow.check_request_progress())
                    .map_err(|_| crate::handwriting::ContextFailure::Canceled)
            })
        });

        if let Some(capture) = &handwriting {
            log::debug!("Handwriting selection {}", capture.diagnostic());
        }
        self.handwriting_operation = handwriting;
        anyhow::ensure!(
            self.handwriting_operation
                .as_ref()
                .is_none_or(|c| c.failure.is_none()),
            "handwriting context unavailable"
        );

        self.workflow.check_request_progress()?;
        Ok(images)
    }

    fn run_prepared(&mut self, attempt: &mut Attempt) -> Result<()> {
        let personalization = self.personalization.clone();
        let images = self.prepare_model_images(attempt)?;
        let encoded = encode_model_images(images);
        let confirmed_context = self
            .handwriting_operation
            .as_ref()
            .and_then(|c| c.context.clone());

        // Step 3: Propose a question and answer, then independently verify
        // the question before navigating or writing:
        // - Detect outlined or highlighted region
        // - Extract question text
        // - Generate answer
        let (result, marked) =
            self.analyze_and_answer(attempt, encoded, confirmed_context.as_deref())?;

        match result {
            None => {
                if let Some(marked) = marked {
                    self.workflow.check_request_progress()?;
                    let fresh = attempt.interpret_marked_question(
                        &self.ledger,
                        &marked.text,
                        marked.region,
                        marked.target.as_ref(),
                        None,
                    )?;
                    if let (Some(fresh), Some(destination)) = (fresh, attempt.marked_destination) {
                        return self.workflow.present_marked_question(
                            &self.ledger,
                            &attempt.user,
                            fresh,
                            attempt.marked_source.as_deref(),
                            destination,
                        );
                    }
                    info!("Marked question retained. User-only presentation unavailable");
                    self.workflow.report_failure(Failure::Selection)?;
                    return Ok(());
                }
                info!("No clear selected region or readable question detected");
                // Record a non-ink diagnostic (no text output)
                self.workflow.report_failure(Failure::Selection)?;
                attempt.outcome(
                    &self.ledger,
                    false,
                    Outcome::Failed,
                    AttemptReason::NoSelection,
                )?;
                return Ok(());
            }
            Some(mut result) => {
                // Re-read the acknowledged stored batch; never query the device again.
                let verification = encode_model_images(attempt.images(&self.ledger)?);
                if let Some(failure) = self.verify_question(
                    attempt,
                    &result,
                    &verification,
                    confirmed_context.as_deref(),
                )? {
                    info!(
                        "Independent question reading disagreed or was uncertain; no answer written"
                    );
                    self.workflow.report_failure(failure)?;
                    attempt.outcome(
                        &self.ledger,
                        false,
                        Outcome::Failed,
                        if failure == Failure::Provider {
                            AttemptReason::ProviderFailure
                        } else {
                            AttemptReason::TranscriptionDisagreement
                        },
                    )?;
                    return Ok(());
                }
                info!("Got verified Q&A");

                let installed_selection = (self.workflow.uses_installed_reader_history()
                    && attempt.resident_output())
                .then_some(&result.selection);
                let mut fresh =
                    if let Some(marked) = marked.filter(|marked| marked.text == result.question) {
                        self.workflow.check_request_progress()?;
                        attempt.interpret_marked_question(
                            &self.ledger,
                            &result.question,
                            marked.region,
                            result.target.as_ref(),
                            installed_selection,
                        )?
                    } else {
                        attempt.interpret_source_target(
                            &self.ledger,
                            &result.question,
                            result.target.as_ref(),
                            installed_selection,
                        )?;
                        None
                    };
                if attempt.source_target_enabled() {
                    result.about = Some(match result.target.as_ref() {
                        Some(candidate) => candidate.about()?,
                        None => "About: Uncertain - Selected source could not be localized".into(),
                    });
                }
                let preserve_marked_user = fresh.is_some();
                let mut memory_view = None;
                if self.memory.is_some() || personalization.effective_text().is_some() {
                    let enriched =
                        self.enrich_answer(attempt, &mut result, &verification, &personalization);
                    if let Err(error) = enriched {
                        log::warn!("Contextual answer was not completed");
                        self.workflow.report_failure(Failure::Provider)?;
                        if preserve_marked_user {
                            return Err(error);
                        }
                        let canceled = self.workflow.input_failed();
                        attempt.outcome(
                            &self.ledger,
                            false,
                            if canceled {
                                Outcome::Canceled
                            } else {
                                Outcome::Failed
                            },
                            if canceled {
                                AttemptReason::Canceled
                            } else {
                                AttemptReason::ProviderFailure
                            },
                        )?;
                        return Err(error);
                    }
                    memory_view = enriched?;
                    info!("Got verified Q&A with optional subject memory");
                }
                drop(verification);
                let memory = self.memory.clone();
                let mut prepared = None;
                let mut publication_called = false;
                if let (Some(memory), Some(view)) = (memory, memory_view.as_ref()) {
                    let published = if attempt.is_selected() {
                        self.publish_answer(attempt, &result, false, Some((memory.as_ref(), view)))
                    } else {
                        memory.with_current(view, &mut || {
                            if preserve_marked_user {
                                anyhow::ensure!(
                                    !std::mem::replace(&mut publication_called, true),
                                    "marked answer publication callback already consumed"
                                );
                            }
                            prepared = self.prepare_marked_answer(
                                attempt,
                                &result,
                                fresh.take(),
                                !view.selected(),
                            )?;
                            if prepared.is_some() {
                                return Ok(());
                            }
                            self.publish_answer(attempt, &result, !view.selected(), None)
                        })
                    };
                    if let Err(error) = published {
                        if attempt.assistant.is_none() && !preserve_marked_user {
                            let canceled = self.workflow.input_failed();
                            attempt.outcome(
                                &self.ledger,
                                false,
                                if canceled {
                                    Outcome::Canceled
                                } else {
                                    Outcome::Failed
                                },
                                if canceled {
                                    AttemptReason::Canceled
                                } else {
                                    AttemptReason::ProviderFailure
                                },
                            )?;
                        }
                        return Err(error);
                    }
                } else {
                    prepared = self.prepare_marked_answer(attempt, &result, fresh.take(), true)?;
                    if prepared.is_none() {
                        self.publish_answer(attempt, &result, true, None)?;
                    }
                }
                if let Some(prepared) = prepared {
                    self.workflow.present_marked_answer(
                        &self.ledger,
                        &attempt.user,
                        prepared,
                        attempt.marked_source.as_deref(),
                    )?;
                }
            }
        }

        info!("=== Iteration Complete ===");
        Ok(())
    }

    fn compose_result(result: &AnalysisResult) -> String {
        let body = Workflow::compose_qa(&result.question, &result.answer, result.selection.center);
        Workflow::with_source_description(body, result.about.as_deref())
    }

    fn prepare_marked_answer(
        &mut self,
        attempt: &mut Attempt,
        result: &AnalysisResult,
        fresh: Option<crate::conversation::marked_question::FreshMarkedQuestion>,
        allow_native: bool,
    ) -> Result<Option<crate::conversation::marked_question::MarkedAnswer>> {
        let (Some(fresh), Some(target)) = (fresh, attempt.marked_destination) else {
            return Ok(None);
        };
        if !allow_native || attempt.is_selected() {
            return Ok(None);
        }
        self.workflow.check_request_progress()?;
        let Some(binding) = self.workflow.marked_answer_binding(
            &self.ledger,
            target,
            fresh.presentation_head().0,
        )?
        else {
            info!("Marked question erase refused: pair presentation unavailable");
            return Ok(None);
        };
        attempt
            .prepare_marked_answer(
                &self.ledger,
                fresh,
                Self::compose_result(result),
                target,
                binding,
            )
            .map(Some)
    }
    fn publish_answer(
        &mut self,
        attempt: &mut Attempt,
        result: &AnalysisResult,
        allow_native: bool,
        memory: Option<(&dyn crate::memory::Provider, &crate::memory::QueryResult)>,
    ) -> Result<()> {
        self.workflow.check_request_progress()?;
        attempt.check_selected()?;
        let installed = self.workflow.uses_installed_reader_history() && attempt.resident_output();
        let stored_answer = if installed {
            result.answer.clone()
        } else {
            Self::compose_result(result)
        };
        attempt.generated_with_memory(&self.ledger, stored_answer, memory)?;
        self.place_answer(attempt, result, allow_native, &mut |_| Ok(()))
    }

    fn place_answer(
        &mut self,
        attempt: &mut Attempt,
        result: &AnalysisResult,
        allow_native: bool,
        before_output: &mut dyn FnMut(&Ledger) -> Result<()>,
    ) -> Result<()> {
        #[cfg(not(target_os = "linux"))]
        let _ = before_output;
        if attempt.is_selected() {
            if attempt.selected_resident() {
                return Ok(());
            }
            anyhow::bail!("Selected Generated draft acknowledged; qualified Pending/native output unavailable");
        }
        if !allow_native {
            attempt.outcome(
                &self.ledger,
                true,
                Outcome::Failed,
                AttemptReason::NativeOutputUnavailable,
            )?;
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        if attempt.resident_output() {
            use super::resident_output::ReaderAbout;
            let installed = self.workflow.uses_installed_reader_history();
            let text = super::resident_output::ResidentOutputText::Reader {
                question: &result.question,
                answer: &result.answer,
                center: result.selection.center,
                about: result
                    .about
                    .as_deref()
                    .map(ReaderAbout::PreparedLine)
                    .or_else(|| {
                        result
                            .selection
                            .subject
                            .as_ref()
                            .filter(|_| installed)
                            .map(ReaderAbout::Subject)
                    }),
            };
            let conversation = attempt.user.conversation;
            let association = attempt.resident_answer.clone();
            let mut pending_root = None;
            let rendered = self.workflow.render_resident_answer(
                &self.ledger,
                conversation,
                text,
                association.as_ref(),
                &mut || {
                    if pending_root.is_none() {
                        before_output(&self.ledger)?;
                        pending_root = Some(attempt.start_resident_output(&self.ledger)?);
                    }
                    Ok(())
                },
            );
            let recorded = match &rendered {
                Ok(super::resident_output::ResidentOutputOutcome::Unavailable) => {
                    if let Some(root) = pending_root {
                        // Placement can refuse text after successfully probing
                        // and restoring the source, before an insertion or write.
                        attempt.refuse_after_restored_page(&self.ledger, root)
                    } else {
                        attempt.outcome(
                            &self.ledger,
                            true,
                            Outcome::Failed,
                            AttemptReason::NativeOutputUnavailable,
                        )
                    }
                }
                Ok(super::resident_output::ResidentOutputOutcome::InsertionRefused) => attempt
                    .outcome(
                        &self.ledger,
                        true,
                        Outcome::Failed,
                        AttemptReason::NoSuccessor,
                    ),
                Ok(super::resident_output::ResidentOutputOutcome::Unresolved) => attempt.outcome(
                    &self.ledger,
                    true,
                    Outcome::ReconcileRequired,
                    AttemptReason::DeviceUncertain,
                ),
                Ok(super::resident_output::ResidentOutputOutcome::SubmittedUnverified) => attempt
                    .outcome(
                        &self.ledger,
                        true,
                        Outcome::ReconcileRequired,
                        AttemptReason::SubmittedUnverified,
                    ),
                Ok(super::resident_output::ResidentOutputOutcome::ReusedWithoutWrite) => attempt
                    .outcome(
                        &self.ledger,
                        true,
                        Outcome::Failed,
                        AttemptReason::NativeOutputUnavailable,
                    ),
                Err(_)
                    if pending_root.is_none()
                        && self.workflow.input_failed()
                        && !self.workflow.runtime_failed() =>
                {
                    attempt.outcome(
                        &self.ledger,
                        true,
                        Outcome::Canceled,
                        AttemptReason::Canceled,
                    )
                }
                Err(_) => attempt.outcome(
                    &self.ledger,
                    true,
                    Outcome::ReconcileRequired,
                    AttemptReason::DeviceUncertain,
                ),
            };
            if let Err(recording) = recorded {
                return Err(anyhow::anyhow!(
                    "resident output {:?}; terminal recording failed: {recording:#}",
                    rendered.as_ref().err()
                ));
            }
            rendered?;
            return Ok(());
        }
        if !attempt.legacy_output() {
            attempt.outcome(
                &self.ledger,
                true,
                Outcome::Failed,
                AttemptReason::NativeOutputUnavailable,
            )?;
            return Ok(());
        }
        // Persist uncertainty before the first navigation/output effect.
        attempt.outcome(
            &self.ledger,
            true,
            Outcome::ReconcileRequired,
            AttemptReason::OutputPending,
        )?;
        let rendered = self.render_answer(result);
        let recorded = match &rendered {
            Ok(RenderOutcome::NoOutput(reason)) => {
                attempt.outcome(&self.ledger, true, Outcome::Failed, reason.clone())
            }
            Ok(RenderOutcome::SubmittedUnverified) => attempt.outcome(
                &self.ledger,
                true,
                Outcome::ReconcileRequired,
                AttemptReason::SubmittedUnverified,
            ),
            Err(_) => attempt.outcome(
                &self.ledger,
                true,
                Outcome::ReconcileRequired,
                AttemptReason::DeviceUncertain,
            ),
        };
        if let Err(recording) = recorded {
            return Err(anyhow::anyhow!(
                "output result {:?}; terminal recording failed: {recording:#}",
                rendered.as_ref().err()
            ));
        }
        if let Err(e) = rendered {
            error!("Error rendering answer: {}", e);
            if self.workflow.runtime_failed() {
                return Err(e);
            }
            // On error, retain the typed diagnostic without source feedback
            self.workflow.report_failure(Failure::Device)?;
            return Err(e);
        }
        Ok(())
    }

    /// First pass:
    /// 1. Detects outlined or highlighted content
    /// 2. Extracts handwritten question
    /// 3. Generates answer
    /// 4. Provides bounding boxes
    ///
    /// Returns None if no clear selection/question is found.
    fn analyze_and_answer(
        &mut self,
        attempt: &Attempt,
        images: Vec<String>,
        context: Option<&crate::handwriting::ConfirmedContext>,
    ) -> Result<(Option<AnalysisResult>, Option<MarkedFragment>)> {
        #[cfg(any(target_os = "linux", test))]
        let context = context.or_else(|| attempt.selected_handwriting());
        attempt.check_selected()?;
        info!("Sending analysis + answer proposal");

        let prompt = if self.workflow.uses_installed_reader_history()
            && attempt.resident_output()
            && !attempt.source_target_enabled()
        {
            std::borrow::Cow::Owned(format!("{ANALYSIS_PROMPT}\n\n{SOURCE_SUBJECT_PROMPT}"))
        } else {
            std::borrow::Cow::Borrowed(ANALYSIS_PROMPT)
        };
        let payload = context
            .map(|context| {
                crate::handwriting::runtime::reader_payload(&prompt, false, &images, context)
            })
            .transpose()?;
        let owned = ModelContent(&mut self.llm);
        owned.0.clear_content();
        if let Some(parts) = payload {
            crate::handwriting::runtime::send_parts(owned.0, &parts);
        } else {
            owned.0.add_text_content(&prompt);
            for image in &images {
                owned.0.add_image_content(image);
            }
        }
        drop(images);

        if attempt.source_target_enabled() {
            owned
                .0
                .add_text_content(crate::conversation::target::GEOMETRY_CONTRACT);
        }
        if !attempt.is_selected() {
            owned.0.add_text_content(MARKED_QUESTION_PROMPT);
        }
        let mut progress_failed = false;
        // Stored source readback, recipe verification, base64 encoding and model
        // payload assembly all consume the original acquisition budget. Retire
        // that deadline only at the actual first provider-admission boundary.
        self.workflow.deliver_reader_acquisition()?;
        let response = match attempt.execute_model(&mut *owned.0, &mut || {
            let result = attempt
                .model_progress()
                .and_then(|_| self.workflow.check_request_progress());
            progress_failed |= result.is_err();
            result
        }) {
            Ok(response) => response,
            Err(error) => {
                if !progress_failed {
                    self.workflow.report_failure(Failure::Provider)?;
                }
                return Err(error);
            }
        };
        self.workflow.check_request_progress()?;
        info!("Analysis response received ({} bytes)", response.len());
        attempt.check_selected()?;
        let marked = if attempt.is_selected() {
            None
        } else {
            Self::parse_marked_fragment(&response)
        };
        Ok((Self::parse_analysis_response(&response), marked))
    }

    fn analysis_separator(response: &str) -> Option<usize> {
        response.match_indices("---").find_map(|(start, marker)| {
            let header = &response[..start];
            let line = header.rsplit('\n').next()?.trim();
            let rest = &response[start + marker.len()..];
            let standalone = line.is_empty() && rest.split('\n').next()?.trim().is_empty();
            // A compact question can end the header only after the required selection center.
            // In the usual field order even literal "---ANSWER:" belongs to the handwritten question.
            let final_question = line.starts_with("QUESTION:")
                && header
                    .lines()
                    .any(|field| field.trim_start().starts_with("SELECTION_CENTER:"));
            // Older responses can attach the answer boundary to the final question or coordinate field.
            // The JSON and subject fields can themselves contain literal "---ANSWER:" text.
            let compact = (line.is_empty()
                || final_question
                || line.starts_with("QUESTION_BOX:")
                || line.starts_with("SELECTION_CENTER:"))
                && rest.trim_start().starts_with("ANSWER:");
            (standalone || compact).then_some(start)
        })
    }

    fn parse_marked_fragment(response: &str) -> Option<MarkedFragment> {
        let separator = Self::analysis_separator(response)?;
        let answer = response[separator + 3..]
            .trim()
            .strip_prefix("ANSWER:")?
            .trim();
        if answer.is_empty() {
            return None;
        }
        let header = &response[..separator];
        let field = |prefix: &str| {
            let mut values = header
                .lines()
                .filter_map(|line| line.trim().strip_prefix(prefix));
            let value = values.next()?.trim();
            values.next().is_none().then_some(value)
        };
        if field("HANDWRITTEN_QUESTION_MARK:")? != "present" {
            return None;
        }
        let question = field("QUESTION:")?;
        if question.trim().is_empty()
            || question.len() > 4096
            || question.eq_ignore_ascii_case("NONE")
            || question.chars().any(char::is_control)
        {
            return None;
        }
        let bounds = Self::parse_bounding_box(field("QUESTION_BOX:")?)?;
        let region = [bounds.x, bounds.y, bounds.width, bounds.height];
        crate::conversation::reader_plan::checked_region(region).ok()?;
        Some(MarkedFragment {
            text: question.to_owned(),
            region,
            target: field("TARGET_JSON:").and_then(Self::parse_target_candidate),
        })
    }

    fn parse_target_candidate(raw: &str) -> Option<crate::conversation::target::Candidate> {
        crate::conversation::target::parse_candidate(raw)
    }

    fn parse_analysis_response(response: &str) -> Option<AnalysisResult> {
        // Parse the response
        if response.trim().to_uppercase().starts_with("NONE") {
            return None;
        }

        // Parse the structured response
        // Missing fields or explicit abstention must never become typed Q&A.
        let separator = Self::analysis_separator(response)?;
        let (header, body) = (&response[..separator], &response[separator + 3..]);
        let answer_text = body.trim().strip_prefix("ANSWER:")?.trim();

        // Extract question text
        let question_text = Self::extract_field(header, "QUESTION:");
        if [question_text.as_str(), answer_text]
            .iter()
            .any(|value| value.is_empty() || value.eq_ignore_ascii_case("NONE"))
        {
            return None;
        }

        // Extract bounding boxes
        let question_box = Self::parse_bounding_box(&Self::extract_field(header, "QUESTION_BOX:"));
        let mut centers = header
            .lines()
            .filter_map(|line| line.trim().strip_prefix("SELECTION_CENTER:"));
        let raw_center = centers.next()?;
        if centers.next().is_some() {
            return None;
        }
        let parts: Vec<_> = raw_center.split(',').map(str::trim).collect();
        if parts.len() != 2 {
            return None;
        }
        let selection_center = SelectionCenter::from_pixels(
            parts[0].parse().ok()?,
            parts[1].parse().ok()?,
            SCREENSHOT_VIRTUAL_WIDTH,
            SCREENSHOT_VIRTUAL_HEIGHT,
        )?;

        let mut subjects = header
            .lines()
            .filter_map(|line| line.trim().strip_prefix(SOURCE_SUBJECT_FIELD));
        let subject = subjects
            .next()
            .map(str::trim)
            .filter(|text| subjects.next().is_none() && !text.eq_ignore_ascii_case(&question_text))
            .and_then(|text| {
                if text.eq_ignore_ascii_case("NONE") {
                    Some(ReaderSubject::Uncertain)
                } else {
                    ReaderSourceSubject::from_proposal(text).map(ReaderSubject::Recognized)
                }
            });

        debug!("Parsed question ({} bytes)", question_text.len());
        debug!("Question box: {:?}", question_box);
        debug!("Selected-content center: {}", selection_center);

        Some(AnalysisResult {
            question: question_text,
            answer: answer_text.to_string(),
            _question_box: question_box,
            selection: ReaderSelection {
                center: selection_center,
                subject,
            },
            target: {
                let mut fields = header
                    .lines()
                    .filter_map(|line| line.trim().strip_prefix("TARGET_JSON:"));
                fields.next().and_then(|value| {
                    if fields.next().is_none() {
                        Self::parse_target_candidate(value.trim())
                    } else {
                        None
                    }
                })
            },
            about: None,
        })
    }

    fn verify_question(
        &mut self,
        attempt: &Attempt,
        result: &AnalysisResult,
        images: &[String],
        context: Option<&crate::handwriting::ConfirmedContext>,
    ) -> Result<Option<Failure>> {
        #[cfg(any(target_os = "linux", test))]
        let context = context.or_else(|| attempt.selected_handwriting());
        attempt.check_selected()?;
        let Some(bounds) = &result._question_box else {
            return Ok(Some(Failure::Transcription));
        };
        if !(0..1024).contains(&bounds.y) || bounds.height <= 0 {
            return Ok(Some(Failure::Transcription));
        }
        let prompt = "Transcribe only the handwritten question in these images of the same page. \
             Use the outline or highlight and placement to identify the question, which may \
             be nearby or inside the outline and may refer to the selection without repeating it. \
             Distinguish the question from selected content and surrounding notes whether they \
             are handwritten or printed. Preserve only the visible question text, including \
             shorthand, fragments, and abbreviations. Do not add words from the selection \
             or expand or guess text from context. Do not answer the question. If an essential \
             handwritten word is uncertain, overstruck or unreadable, or the question is absent \
             or cannot be distinguished from the source, return only NONE. Otherwise \
             return one line: TRANSCRIPTION: [exact handwritten text].";
        let payload = context
            .map(|c| crate::handwriting::runtime::reader_payload(prompt, true, images, c))
            .transpose()?;
        let owned = ModelContent(&mut self.llm);
        owned.0.clear_content();
        if let Some(parts) = payload {
            crate::handwriting::runtime::send_parts(owned.0, &parts);
        } else {
            owned.0.add_text_content(prompt);
            for image in images {
                owned.0.add_image_content(image);
            }
        }
        let mut progress_failed = false;
        let reading = match attempt.execute_model(&mut *owned.0, &mut || {
            let result = attempt
                .model_progress()
                .and_then(|_| self.workflow.check_request_progress());
            progress_failed |= result.is_err();
            result
        }) {
            Ok(reading) => reading,
            Err(err) if progress_failed => {
                return Err(err.context("Question verification progress failed"));
            }
            Err(err) => {
                log::warn!("Question verification unavailable: {}", err);
                return Ok(Some(Failure::Provider));
            }
        };
        self.workflow.check_request_progress()?;
        info!(
            "Independent question reading received ({} bytes)",
            reading.len()
        );
        attempt.check_selected()?;
        Ok((!Self::transcriptions_agree(&result.question, &reading))
            .then_some(Failure::Transcription))
    }

    fn transcriptions_agree(question: &str, reading: &str) -> bool {
        let independent = Self::extract_field(reading, "TRANSCRIPTION:");
        let normalize = |text: &str| -> String {
            let chars: Vec<char> = text.to_lowercase().chars().collect();
            let mut normalized = String::new();
            for (i, &c) in chars.iter().enumerate() {
                let numeric_separator = matches!(c, '.' | ',')
                    && i > 0
                    && chars[i - 1].is_ascii_digit()
                    && chars.get(i + 1).is_some_and(char::is_ascii_digit);
                if c.is_alphanumeric() || numeric_separator {
                    normalized.push(c);
                } else if c.is_whitespace() || ".?,;:".contains(c) {
                    normalized.push(' ');
                } else {
                    // Keep operators/grouping distinct, regardless of spacing.
                    normalized.push(' ');
                    normalized.push(c);
                    normalized.push(' ');
                }
            }
            normalized.split_whitespace().collect::<Vec<_>>().join(" ")
        };
        let a = normalize(question);
        let b = normalize(&independent);
        !a.is_empty() && !b.is_empty() && b != "none" && a == b
    }

    /// Extract a field value from the response
    fn extract_field(text: &str, field_name: &str) -> String {
        for line in text.lines() {
            if let Some(value) = line.trim().strip_prefix(field_name) {
                return value.trim().to_string();
            }
        }
        "".to_string()
    }

    /// Parse bounding box from "x,y,width,height" format
    fn parse_bounding_box(text: &str) -> Option<BoundingBox> {
        let parts: Vec<&str> = text.split(',').collect();
        if parts.len() == 4 {
            if let (Ok(x), Ok(y), Ok(w), Ok(h)) = (
                parts[0].trim().parse::<i32>(),
                parts[1].trim().parse::<i32>(),
                parts[2].trim().parse::<i32>(),
                parts[3].trim().parse::<i32>(),
            ) {
                return Some(BoundingBox {
                    x,
                    y,
                    width: w,
                    height: h,
                });
            }
        }
        None
    }

    fn enrich_answer(
        &mut self,
        attempt: &Attempt,
        result: &mut AnalysisResult,
        images: &[String],
        preference: &crate::config::PersonalizationSnapshot,
    ) -> Result<Option<crate::memory::QueryResult>> {
        attempt.check_selected()?;
        self.workflow.check_request_progress()?;
        let retrieved = self
            .memory
            .as_ref()
            .map(|memory| -> Result<_> {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_millis()
                    .try_into()?;
                // Selected ownership supplies historical data scope only, never
                // native authority. Ordinary capture supplies no association.
                let mut query = crate::memory::Query::new(result.question.clone(), now);
                query.document = attempt.memory_document()?;
                attempt
                    .query_memory(memory.as_ref(), &query)
                    .map_err(|_| anyhow::anyhow!("subject memory retrieval failed"))
            })
            .transpose()?;
        attempt.check_selected()?;
        self.workflow.check_request_progress()?;
        use crate::memory::Status;
        if let Some(view) = &retrieved {
            match view.status {
                Status::Disabled | Status::CompleteEmpty
                    if preference.effective_text().is_none() =>
                {
                    return Ok(retrieved)
                }
                Status::Unavailable => anyhow::bail!("subject memory retrieval unavailable"),
                Status::Partial if view.context.facts.is_empty() => {
                    anyhow::bail!("subject memory retrieval incomplete without usable facts")
                }
                _ => {}
            }
        }
        let usable = retrieved.as_ref().filter(|view| {
            matches!(view.status, Status::Complete | Status::Partial)
                && !view.context.facts.is_empty()
        });
        let workflow = &mut self.workflow;
        #[cfg(any(target_os = "linux", test))]
        if attempt.selected_resident() {
            let joined = self.memory.as_deref().zip(retrieved.as_ref());
            result.answer = crate::memory::context::answer_with_resident_handoff(
                &mut self.llm,
                &result.question,
                images,
                usable,
                preference,
                &mut || {
                    attempt
                        .model_progress()
                        .and_then(|_| workflow.check_request_progress())
                },
                &mut |engine, progress| attempt.execute_enrichment(engine, progress, joined),
            )?;
            attempt.check_selected()?;
            self.workflow.check_request_progress()?;
            return Ok(retrieved);
        }
        result.answer = if let Some(view) = usable.filter(|view| view.selected()) {
            crate::memory::context::answer_with_selected_view(
                &mut self.llm,
                &result.question,
                images,
                view,
                self.ledger.store(),
                Some(preference),
                &mut || {
                    attempt
                        .check_selected()
                        .and_then(|_| workflow.check_request_progress())
                },
            )
        } else {
            crate::memory::context::answer_with_preference(
                &mut self.llm,
                &result.question,
                images,
                usable,
                Some(preference),
                &mut || {
                    attempt
                        .check_selected()
                        .and_then(|_| workflow.check_request_progress())
                },
            )
        }?;
        attempt.check_selected()?;
        self.workflow.check_request_progress()?;
        Ok(retrieved)
    }

    /// Render the answer on the next page
    ///
    /// Simplified flow:
    /// 1. Store original page screenshot for later comparison
    /// 2. Navigate right to next page
    /// 3. Compare to original to verify we actually moved
    /// 4. Check if page is valid (blank or existing QA page)
    /// 5. If not valid or didn't move â†’ verify the source and record a non-ink failure
    /// 6. If valid â†’ render Q&A on that page
    fn render_answer(&mut self, result: &AnalysisResult) -> Result<RenderOutcome> {
        anyhow::ensure!(
            !self.workflow.is_selected(),
            "Selected branch-dependent answer rendering unsupported"
        );
        info!("Attempting to render Q&A on next page");

        // Step 1: Store original page screenshot for comparison
        let original_img = self.workflow.capture_page()?;

        // Step 2: Attempt to navigate to next page
        let completion = self.workflow.navigate_to_next_page()?;
        if completion == crate::device::backend::NavigationCompletion::Legacy {
            // Unverified backends retain the bounded transition fallback.
            self.workflow.delay(std::time::Duration::from_millis(800));
        }

        let unchanged = self.workflow.verify_navigation_to(&original_img)?;
        if completion == crate::device::backend::NavigationCompletion::NoMovement && !unchanged {
            self.workflow.invalidate_captured_viewport();
            anyhow::bail!(
                "Unconfirmed source after navigation reported no movement; classification stopped"
            );
        }
        if unchanged {
            info!("No page movement detected; no source feedback input");
            self.workflow.report_failure(Failure::NoSuccessor)?;
            return Ok(RenderOutcome::NoOutput(AttemptReason::NoSuccessor));
        }

        // Step 4: Check if the page we navigated to is valid (blank or QA)
        if completion == crate::device::backend::NavigationCompletion::Legacy {
            self.workflow.delay(std::time::Duration::from_millis(500));
        }
        let page_type = self.workflow.is_valid_answer_page()?;

        match page_type {
            AnswerPageType::Invalid => {
                // Page exists but is not suitable - return to original
                info!(
                    "Next page is not valid (not blank and not a QA page) - returning to original"
                );

                // Navigate back and verify we're on original
                let returned = self.workflow.return_to_original_page(&original_img)?;
                self.workflow
                    .report_failure(if returned == ReturnOutcome::Unconfirmed {
                        Failure::Device
                    } else {
                        Failure::InvalidSuccessor
                    })?;
                anyhow::ensure!(
                    returned != ReturnOutcome::Unconfirmed,
                    "Unconfirmed return after invalid successor"
                );
                return Ok(RenderOutcome::NoOutput(AttemptReason::InvalidSuccessor));
            }
            AnswerPageType::Blank => {
                // Step 5a: Blank page - render header first, then Q&A
                info!("Blank page found, rendering header and Q&A");

                // Switch to body text mode once before all rendering
                self.workflow.set_body_text_mode()?;

                // Header with two blank lines before first Q&A block
                self.workflow
                    .render_text("=== Reader Buddy Answers ===\n\n\n")?;

                // Save header pattern for future detection (only on first blank page)
                self.workflow.delay(std::time::Duration::from_millis(500));
                let new_png = self.workflow.capture_page_data()?;
                if let Ok(new_img) = image::load_from_memory(&new_png) {
                    const HEADER_HEIGHT: u32 = 150; // Capture full header region from top
                    let header_img = new_img.crop_imm(
                        0,
                        0,
                        new_img.width(),
                        HEADER_HEIGHT.min(new_img.height()),
                    );
                    if let Err(e) = self.workflow.save_header_pattern(&header_img) {
                        log::warn!("Failed to save header pattern: {}", e);
                    }
                }
            }
            AnswerPageType::ExistingQA => {
                // Step 5b: Existing QA page - just append Q&A content (no header)
                info!("Existing QA page found, appending Q&A (no header needed)");

                // Switch to body text mode
                self.workflow.set_body_text_mode()?;
            }
        }

        // Render the Q&A
        let formatted_output = Self::compose_result(result);

        self.workflow.render_qa(&formatted_output)?;

        info!("Q&A rendered successfully");
        Ok(RenderOutcome::SubmittedUnverified)
    }

    pub(crate) fn set_local_handwriting(
        &mut self,
        dispatch: &super::conversation_dispatch::Dispatch<'_>,
    ) -> Result<()> {
        anyhow::ensure!(
            self.handwriting_operation.is_none(),
            "handwriting operation still active"
        );
        self.handwriting = dispatch.handwriting_runtime(&self.ledger)?;
        Ok(())
    }

    /// Run the main loop
    pub fn run_loop(&mut self) -> Result<()> {
        self.run_loop_with_settings(None, None)
    }

    pub fn run_local_loop(&mut self, dispatch: Dispatch<'_>) -> Result<()> {
        dispatch.require_ledger(&self.ledger)?;
        self.set_local_handwriting(&dispatch)?;
        self.run_loop_with_settings(Some(&dispatch), Some(dispatch.config))
    }

    pub fn run_reader_loop(&mut self, config: &Config) -> Result<()> {
        anyhow::ensure!(
            config.reader_enabled(),
            "Reader route requires the Reader module"
        );
        self.run_loop_with_settings(None, Some(config))
    }

    fn run_loop_with_settings(
        &mut self,
        dispatch: Option<&Dispatch<'_>>,
        config: Option<&Config>,
    ) -> Result<()> {
        info!("Starting Reader Buddy main loop");

        loop {
            let _run = crate::measurement::Run::new();
            // History remains owned until another trigger or invalidating input.
            if self.trigger_enabled && !self.workflow.wait_for_idle_batch(None, config)? {
                continue;
            }
            match self.run_iteration_with_trigger(false, dispatch, config) {
                Ok(_) => info!("Iteration completed successfully"),
                Err(e) => {
                    error!("Error in iteration: {}", e);
                    if self.workflow.runtime_failed() {
                        return Err(e);
                    }
                    // The iteration already attempted the non-ink failure diagnostic.
                    // Diagnostics belong in logs, never in the user document.
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Orchestrator, ReaderSubject};
    use crate::OpenAI;

    #[test]
    fn marked_fragment_preserves_nonterminal_shorthand_even_without_an_answer() {
        let response = "QUESTION: G? unc. here\nQUESTION_BOX: 12,24,48,16\nHANDWRITTEN_QUESTION_MARK: present\n---\nANSWER: NONE";
        let marked = Orchestrator::<OpenAI>::parse_marked_fragment(response).unwrap();
        assert_eq!(marked.text, "G? unc. here");
        assert_eq!(marked.region, [12, 24, 48, 16]);
        assert!(Orchestrator::<OpenAI>::parse_analysis_response(response).is_none());
        for (old, new) in [
            ("present", "absent"), // Printed punctuation alone remains absent.
            ("present", "unclear"),
            ("HANDWRITTEN_QUESTION_MARK: present\n", ""),
            ("present", "present\nHANDWRITTEN_QUESTION_MARK: present"),
            ("12,24,48,16", "12,24,48,16\nQUESTION_BOX: 12,24,48,16"),
            ("12,24,48,16", "-1,24,48,16"),
            ("12,24,48,16", "12,24,0,16"),
            ("12,24,48,16", "762,24,48,16"),
            ("12,24,48,16", "12,1020,48,16"),
            ("12,24,48,16", "2147483647,24,48,16"),
        ] {
            assert!(
                Orchestrator::<OpenAI>::parse_marked_fragment(&response.replace(old, new))
                    .is_none(),
                "{new}"
            );
        }
    }

    #[test]
    fn source_target_header_preserves_subject_dashes_and_refuses_duplicate_candidates() {
        let candidate = serde_json::json!({"subject":"A---B relation","geometry":{"components":[[[0,0],[1000000,0],[1000000,1000000],[0,1000000]]]},"understanding":{"status":"interpreted"}});
        let response=format!("QUESTION: Why?\nQUESTION_BOX: 1,2,3,4\nSELECTION_CENTER: 100,100\nTARGET_JSON: {candidate}\n---\nANSWER: Because.");
        let result = Orchestrator::<OpenAI>::parse_analysis_response(&response).unwrap();
        assert_eq!(result.target.unwrap().subject, "A---B relation");
        let duplicate = response.replace("\n---", &format!("\nTARGET_JSON: {candidate}\n---"));
        assert!(Orchestrator::<OpenAI>::parse_analysis_response(&duplicate)
            .unwrap()
            .target
            .is_none());
    }
    #[test]
    fn independent_transcription_requires_the_same_words() {
        assert!(Orchestrator::<OpenAI>::transcriptions_agree(
            "G unc.?",
            "TRANSCRIPTION: g unc?"
        ));
        assert!(!Orchestrator::<OpenAI>::transcriptions_agree(
            "why not atom?",
            "TRANSCRIPTION: why rot. attr.?"
        ));
        assert!(!Orchestrator::<OpenAI>::transcriptions_agree(
            "G unc?", "NONE"
        ));
        assert!(!Orchestrator::<OpenAI>::transcriptions_agree(
            "G unc?",
            "TRANSCRIPTION: NONE"
        ));
        assert!(!Orchestrator::<OpenAI>::transcriptions_agree(
            "G unc?",
            "TRANSCRIPTION: G value?"
        ));
        assert!(!Orchestrator::<OpenAI>::transcriptions_agree(
            "",
            "TRANSCRIPTION:"
        ));
        assert!(Orchestrator::<OpenAI>::transcriptions_agree(
            "Is G=5?",
            "TRANSCRIPTION: is G = 5?"
        ));
        for (first, second) in [
            ("2+2?", "2-2?"),
            ("Is G=1.0?", "Is G=10?"),
            ("why not able?", "why notable?"),
            ("(2+3)*4?", "2+3*4?"),
        ] {
            assert!(!Orchestrator::<OpenAI>::transcriptions_agree(
                first,
                &format!("TRANSCRIPTION: {second}")
            ));
        }
    }

    #[test]
    fn abstention_and_malformed_responses_never_render() {
        for response in [
            "NONE",
            "QUESTION: NONE\nQUESTION_BOX: 0,0,0,0\nOUTLINE_BOX: 0,0,0,0\n---\nANSWER: NONE",
            "QUESTION: readable?\n---\nANSWER: none",
            "QUESTION: NONE\n---\nANSWER: A plausible summary",
            "A plausible summary without a question",
            "QUESTION: readable?\n---\nAn unlabelled answer",
            "QUESTION_BOX: 1,2,3,4\n---\nANSWER: 4",
        ] {
            assert!(Orchestrator::<OpenAI>::parse_analysis_response(response).is_none());
        }
    }

    #[test]
    fn valid_indented_response_preserves_question_and_complete_answer() {
        let response = "  QUESTION: G unc.?\n  QUESTION_BOX: 1,2,3,4\n  SELECTION_CENTER: 384,512\n---\n  ANSWER: G = (6.674215 +/- 0.000092) * 10^-11 m^3 kg^-1 s^-2.\n---\nExtra answer line.";
        let result = Orchestrator::<OpenAI>::parse_analysis_response(response).unwrap();
        assert_eq!(result.question, "G unc.?");
        assert!(result.answer.ends_with("---\nExtra answer line."));
        assert!(result._question_box.is_some());
        assert_eq!(result.selection.center.to_string(), "(0.5, 0.5)");
    }

    #[test]
    fn source_subject_stays_separate_from_question_and_answer() {
        let first = "QUESTION: why?\nQUESTION_BOX: 130,35,200,50\nSELECTION_CENTER: 385,230";
        let parse = |field| {
            let response = format!("{first}\n{field}\n---\nANSWER: The result is 4.");
            Orchestrator::<OpenAI>::parse_analysis_response(&response).unwrap()
        };
        let result = parse("SELECTION_SUBJECT: 24 / 6");
        assert_eq!(result.question, "why?");
        assert_eq!(result.answer, "The result is 4.");
        assert_eq!(
            result.selection.recognized_subject().unwrap().as_str(),
            "24 / 6"
        );
        let uncertain = parse("SELECTION_SUBJECT: NONE");
        assert!(matches!(
            uncertain.selection.subject.as_ref(),
            Some(ReaderSubject::Uncertain)
        ));
        assert!(uncertain.selection.recognized_subject().is_none());
        assert_eq!(uncertain.question, "why?");
        assert_eq!(uncertain.answer, "The result is 4.");
        for invalid in [
            "",
            "SELECTION_SUBJECT: Why?",
            "SELECTION_SUBJECT: 24 / 6\nSELECTION_SUBJECT: NONE",
            "SELECTION_SUBJECT: 24\u{2028}/ 6",
        ] {
            let result = parse(invalid);
            assert!(result.selection.subject.is_none(), "{invalid:?}");
            assert_eq!(result.question, "why?");
            assert_eq!(result.answer, "The result is 4.");
        }
        assert!(parse(&format!("SELECTION_SUBJECT: {}", "x".repeat(161)))
            .selection
            .subject
            .is_none());
    }

    #[test]
    fn reader_keeps_literal_markers_in_the_subject_and_answer() {
        let header = "QUESTION: why?\nQUESTION_BOX: 130,35,200,50\nSELECTION_CENTER: 385,230";
        let answer = "Keep --- and\n---\nANSWER: literal text.";
        for (field, subject) in [
            ("", None),
            ("\nSELECTION_SUBJECT: A---B\n", Some("A---B")),
            (
                "\r\nSELECTION_SUBJECT: A---ANSWER: B\r\n",
                Some("A---ANSWER: B"),
            ),
        ] {
            let response = format!("{header}{field}---\nANSWER: {answer}");
            let result = Orchestrator::<OpenAI>::parse_analysis_response(&response).unwrap();
            assert_eq!(result.question, "why?");
            assert_eq!(result.answer, answer);
            assert_eq!(
                result
                    .selection
                    .recognized_subject()
                    .map(|subject| subject.as_str()),
                subject
            );
        }
    }

    #[test]
    fn compact_answer_can_follow_the_question_as_the_last_header() {
        let coordinates = "QUESTION_BOX: 130,35,200,50\nSELECTION_CENTER: 385,230";
        let literal = "Does A---ANSWER: B mean four?";
        for (response, question) in [
            (
                format!("{coordinates}\nQUESTION: How much?---ANSWER: Four."),
                "How much?",
            ),
            (
                format!("QUESTION: {literal}\n{coordinates}\n---\nANSWER: Four."),
                literal,
            ),
        ] {
            let result = Orchestrator::<OpenAI>::parse_analysis_response(&response).unwrap();
            assert_eq!(result.question, question);
            assert_eq!(result.answer, "Four.");
        }
    }

    #[test]
    fn missing_ambiguous_or_invalid_selection_centers_decline() {
        for center in [
            "",
            "SELECTION_CENTER: 1",
            "SELECTION_CENTER: 1,2,3",
            "SELECTION_CENTER: NaN,2",
            "SELECTION_CENTER: 1,inf",
            "SELECTION_CENTER: -1,2",
            "SELECTION_CENTER: 769,2",
            "SELECTION_CENTER: 1,1025",
            "SELECTION_CENTER: 384,512\nSELECTION_CENTER: 1,2",
            "OUTLINE_BOX: 1,2,3,4",
        ] {
            let response = format!(
                "QUESTION: G unc.?\nQUESTION_BOX: 130,35,200,50\n{center}\n---\nANSWER: About 14 ppm."
            );
            assert!(
                Orchestrator::<OpenAI>::parse_analysis_response(&response).is_none(),
                "{center}"
            );
        }
    }
}

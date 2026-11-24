//! Ordinary installed Reader on a verified original PDF page.
use super::{
    researched::{INTENT_INSTRUCTIONS, VERIFY_PROMPT},
    AnalysisResult, Attempt, ModelContent, Orchestrator, Workflow, ANALYSIS_PROMPT,
    SOURCE_SUBJECT_PROMPT,
};
use crate::{
    config::researched_reader::OrdinarySearch,
    conversation::{
        receiving::{ReceivingBudget, RequestSource},
        resolution::SourceDirective,
        ContextBudget,
    },
    llm::LLMEngine,
    retrieval::{pdf_worker::PendingIndex, IndexKey, Provider, Retrieved},
    storage::{captured::Captured, Namespace, Uuid},
    workflow::{
        conversation_dispatch::{followup_research, Dispatch},
        reader_intent::{self, Intent},
        resident_source::original_pdf::{
            CapturedOriginalPdf, ObservedOriginalPdf, OrdinaryPdfSource, OriginalPdfPin,
        },
        AcquiredEvidence,
    },
};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{
    cell::RefCell,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

type Acquisition = (AcquiredEvidence, Result<Option<OrdinaryPdfSource>>);

pub(super) fn acquire(
    workflow: &mut Workflow,
    local: Option<&Dispatch<'_>>,
) -> Result<Acquisition> {
    if workflow.uses_installed_reader_history()
        && local.is_some_and(|dispatch| {
            dispatch.config.reader_enabled() && dispatch.config.reader.researched_pdf.is_some()
        })
    {
        let owner = workflow
            .resident_source
            .as_mut()
            .context("Resident PDF owner absent")?;
        let health = workflow
            .service_health
            .as_mut()
            .context("Resident PDF health owner absent")?;
        let (capture, pdf) = owner.acquire_ordinary_pdf(health)?;
        return Ok((AcquiredEvidence::ResidentHistorical(Box::new(capture)), pdf));
    }
    Ok((workflow.acquire_reader_evidence()?, Ok(None)))
}

pub(super) fn classify(
    workflow: &mut Workflow,
    source: Option<OrdinaryPdfSource>,
) -> Result<Option<ObservedOriginalPdf>> {
    let Some(source) = source else {
        return Ok(None);
    };
    let owner = workflow
        .resident_source
        .as_mut()
        .context("Resident PDF owner absent")?;
    let health = workflow
        .service_health
        .as_mut()
        .context("Resident PDF health owner absent")?;
    owner.read_ordinary_pdf(health, source)
}

fn require_source_current(workflow: &mut Workflow, deadline: Instant) -> Result<()> {
    workflow.check_request_progress()?;
    let owner = workflow
        .resident_source
        .as_mut()
        .context("Resident PDF owner absent")?;
    let health = workflow
        .service_health
        .as_mut()
        .context("Resident PDF health owner absent")?;
    owner.require_ordinary_pdf_current(health, deadline)
}

struct OriginalPdf {
    pin: RefCell<OriginalPdfPin>,
    index: RefCell<Option<PendingIndex>>,
    key: IndexKey,
    deadline: Instant,
}

impl OriginalPdf {
    fn new(pdf: CapturedOriginalPdf, deadline: Instant) -> Self {
        let key = pdf.index.key().clone();
        Self {
            pin: RefCell::new(pdf.pin),
            index: RefCell::new(Some(pdf.index)),
            key,
            deadline,
        }
    }

    fn check_deadline(&self) -> Result<()> {
        ensure!(
            Instant::now() < self.deadline,
            "Original PDF request expired"
        );
        Ok(())
    }

    fn check_file(&self) -> Result<()> {
        self.pin.borrow_mut().check(self.deadline)
    }
}

struct ExactQuery<'a> {
    pdf: &'a OriginalPdf,
    question: &'a str,
    progress: RefCell<&'a mut dyn FnMut() -> Result<()>>,
}

impl Provider for ExactQuery<'_> {
    fn query(
        &self,
        expected: &IndexKey,
        question: &str,
        max_passages: usize,
        max_bytes: usize,
        neighbors: bool,
    ) -> Result<Retrieved> {
        ensure!(
            expected == &self.pdf.key && question == self.question,
            "Original PDF query changed"
        );
        self.pdf.check_file()?;
        let mut progress = self.progress.borrow_mut();
        progress()?;
        let index = self
            .pdf
            .index
            .borrow_mut()
            .take()
            .context("Original PDF index already used")?;
        let passages = index.query(
            expected,
            question,
            max_passages,
            max_bytes,
            neighbors,
            self.pdf.deadline,
            &mut **progress,
        )?;
        self.pdf.check_file()?;
        Ok(passages)
    }
}

impl<M: LLMEngine> Orchestrator<M> {
    pub(super) fn run_ordinary_pdf(
        &mut self,
        attempt: &mut Attempt,
        observed: ObservedOriginalPdf,
        dispatch: &Dispatch<'_>,
    ) -> Result<()> {
        dispatch.require_ledger(&self.ledger)?;
        let prepared = attempt.capture_pdf_reading(&self.ledger)?;
        let config = dispatch.config;
        let declaration = config
            .reader
            .researched_pdf
            .as_ref()
            .context("Ordinary PDF opt-in absent")?;
        let policy = config.research_policy_snapshot()?.policy();
        let images = self.prepare_model_images(attempt)?;
        let owner = self
            .workflow
            .resident_source
            .as_mut()
            .context("Resident PDF owner absent")?;
        let health = self
            .workflow
            .service_health
            .as_mut()
            .context("Resident PDF health owner absent")?;
        let deadline = owner.ordinary_pdf_deadline()?;
        let captured = owner.complete_ordinary_pdf(health, observed, deadline)?;
        let pdf = OriginalPdf::new(captured, deadline);
        let encoded = images
            .iter()
            .map(|image| STANDARD.encode(image))
            .collect::<Vec<_>>();
        let prompt = format!(
            "{}\n{INTENT_INSTRUCTIONS}\nTranscribe and locate the request only. Use ANSWER: Deferred as the format placeholder. The actual answer is generated later from verified context.",
            ANALYSIS_PROMPT.replace(
                "QUESTION: [question]\n",
                "QUESTION: [question]\nREQUEST_WEB_JSON: [one intent metadata JSON object]\n",
            )
        );
        let prompt = if attempt.source_target_enabled() {
            prompt
        } else {
            format!("{prompt}\n{SOURCE_SUBJECT_PROMPT}")
        };
        let proposed =
            self.read_ordinary_pdf(attempt, &prepared, &pdf, &prompt, &encoded, false)?;
        if proposed.trim().eq_ignore_ascii_case("NONE") {
            require_source_current(&mut self.workflow, pdf.deadline)?;
            prepared.with_current(self.ledger.store(), || Ok(()))?;
            self.workflow.report_failure(super::Failure::Selection)?;
            return attempt.outcome(
                &self.ledger,
                false,
                crate::conversation::Outcome::Failed,
                crate::conversation::AttemptReason::NoSelection,
            );
        }
        let separator =
            Self::analysis_separator(&proposed).context("PDF question header absent")?;
        let analysis = reader_intent::parse_ordinary_pdf_header(&proposed[..separator])?;
        let mut result = Self::parse_analysis_response(&proposed, images[0].clone())
            .context("PDF question or selection unavailable")?;
        let bounds = result
            ._question_box
            .as_ref()
            .context("PDF question bounds unavailable")?;
        ensure!(
            (0..1024).contains(&bounds.y) && bounds.height > 0,
            "PDF question bounds invalid"
        );

        // The independent reader receives the acknowledged images again and
        // none of the first reply, geometry or web-intent claims.
        let verification = attempt
            .images(&self.ledger)?
            .iter()
            .map(|image| STANDARD.encode(image))
            .collect::<Vec<_>>();
        let verified = self.read_ordinary_pdf(
            attempt,
            &prepared,
            &pdf,
            &format!("{VERIFY_PROMPT}\n{INTENT_INSTRUCTIONS}"),
            &verification,
            true,
        )?;
        let agreement = reader_intent::agree(
            analysis,
            reader_intent::parse_verification_response(&verified)?,
        )?;
        ensure!(
            agreement.question() == result.question,
            "Verified PDF question changed"
        );
        let directive = match agreement.intent() {
            Intent::NoInternet => {
                crate::conversation::receiving::memory::research::ResearchDirective::NoInternet
            }
            Intent::NoRestriction => {
                crate::conversation::receiving::memory::research::ResearchDirective::UsePolicy
            }
            Intent::Unclear => anyhow::bail!("PDF question web intent is unclear"),
        };
        pdf.check_file()?;
        require_source_current(&mut self.workflow, pdf.deadline)?;
        let interpreted_root = attempt.interpret_ordinary_pdf(
            &self.ledger,
            &result.question,
            result.target.as_ref(),
            &result.selection,
            &prepared,
        )?;
        if attempt.source_target_enabled() {
            result.about = Some(match result.target.as_ref() {
                Some(candidate) => candidate.about()?,
                None => "About: Uncertain - Selected source could not be localized".into(),
            });
        }
        let research = followup_research::ResearchRequest {
            directive,
            search: declaration
                .research_model
                .as_deref()
                .map(OrdinarySearch::for_installed_pdf),
            deadline,
        };
        self.answer_ordinary_pdf(
            attempt,
            &pdf,
            dispatch,
            policy,
            research,
            &mut result,
            (&interpreted_root, prepared.generation),
        )
    }

    fn read_ordinary_pdf(
        &mut self,
        attempt: &Attempt,
        prepared: &Captured,
        pdf: &OriginalPdf,
        prompt: &str,
        images: &[String],
        verification: bool,
    ) -> Result<String> {
        let context = self
            .handwriting_operation
            .as_ref()
            .and_then(|capture| capture.context.as_deref());
        let payload = context
            .map(|context| {
                crate::handwriting::runtime::researched_reader_payload(
                    prompt,
                    verification,
                    images,
                    context,
                )
            })
            .transpose()?;
        let model = ModelContent(&mut self.llm);
        model.0.clear_content();
        if let Some(parts) = payload {
            crate::handwriting::runtime::send_parts(model.0, &parts);
        } else {
            model.0.add_text_content(prompt);
            for image in images {
                model.0.add_image_content(image);
            }
        }
        if !verification && attempt.source_target_enabled() {
            model
                .0
                .add_text_content(crate::conversation::target::GEOMETRY_CONTRACT);
        }
        if !verification {
            pdf.check_file()?;
            self.workflow.deliver_reader_acquisition()?;
        }
        let workflow = RefCell::new(&mut self.workflow);
        let store = self.ledger.store();
        let progress = || {
            pdf.check_deadline()?;
            attempt.model_progress()?;
            workflow.borrow_mut().check_request_progress()?;
            prepared.with_current(store, || Ok(()))
        };
        let submit = |call: &mut dyn FnMut() -> Result<()>| {
            progress()?;
            pdf.check_file()?;
            require_source_current(&mut workflow.borrow_mut(), pdf.deadline)?;
            prepared.with_current(store, call)
        };
        let reply = followup_research::once(&submit, |handoff| {
            model
                .0
                .execute_with_deadline(&mut || progress(), handoff, pdf.deadline)
        })?;
        progress()?;
        Ok(reply)
    }

    #[allow(clippy::too_many_arguments)]
    fn answer_ordinary_pdf(
        &mut self,
        attempt: &mut Attempt,
        pdf: &OriginalPdf,
        dispatch: &Dispatch<'_>,
        policy: crate::conversation::receiving::memory::research::ResearchPolicy,
        research: followup_research::ResearchRequest<'_>,
        result: &mut AnalysisResult,
        (expected, generation): (&crate::conversation::ExpectedHeads, Uuid),
    ) -> Result<()> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_millis()
            .try_into()?;
        let ledger = &self.ledger;
        let conversation = attempt.user.conversation;
        let user = attempt.user.id;
        let prepared = {
            let mut progress = || {
                pdf.check_deadline()?;
                attempt.model_progress()?;
                self.workflow.check_request_progress()?;
                let (live_generation, heads) = ledger
                    .store()
                    .conditional_snapshot(Namespace::Conversation, conversation)?;
                ensure!(
                    live_generation == generation
                        && matches!(heads.as_slice(), [head] if expected.0.contains(&head.revision_id)),
                    "PDF conversation changed during retrieval"
                );
                Ok(())
            };
            let provider = ExactQuery {
                pdf,
                question: &result.question,
                progress: RefCell::new(&mut progress),
            };
            ledger.prepare_request_evidence_with_directive(
                conversation,
                user,
                None,
                SourceDirective::Explicit(RequestSource::Original {
                    origin_turn: user,
                    source_record: attempt.retained_source(),
                    expected: pdf.key.clone(),
                }),
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
                Some(&provider),
                |text| Ok(text.len()),
            )?
        };
        ensure!(
            prepared.context().request() == &attempt.user
                && &prepared.original_root() == expected
                && prepared.original_generation() == generation,
            "Interpreted PDF request changed"
        );
        pdf.check_file()?;
        require_source_current(&mut self.workflow, pdf.deadline)?;
        let (context, mut evidence) = ledger.persist_followup_evidence(
            crate::writer::stage(attempt.user.exchange, "reader-pdf-evidence"),
            timestamp,
            prepared,
        )?;
        attempt.advance_pdf_evidence(ledger, expected, evidence.root())?;
        let answer = {
            let workflow = RefCell::new(&mut self.workflow);
            let progress = || {
                pdf.check_deadline()?;
                workflow.borrow_mut().check_request_progress()
            };
            let check = || {
                progress()?;
                evidence.with_current(ledger, || Ok(()))
            };
            let submit = |call: &mut dyn FnMut() -> Result<()>| {
                progress()?;
                pdf.check_file()?;
                require_source_current(&mut workflow.borrow_mut(), pdf.deadline)?;
                attempt.check_selected()?;
                evidence.with_current(ledger, call)
            };
            followup_research::answer(
                ledger,
                &mut self.llm,
                context,
                dispatch.config,
                dispatch.local_memory().as_deref(),
                timestamp,
                policy,
                &research,
                &check,
                submit,
            )
        };
        result.answer = match answer {
            Ok(answer) => answer,
            Err(error) if error.is::<crate::llm::ProviderRefused>() => {
                pdf.check_file()?;
                require_source_current(&mut self.workflow, pdf.deadline)?;
                attempt.check_selected()?;
                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)?
                    .as_millis()
                    .try_into()?;
                pdf.check_deadline()?;
                attempt.user = evidence.record_provider_failure(
                    ledger,
                    crate::writer::stage(attempt.user.exchange, "reader-pdf-provider-failure"),
                    attempt.user.clone(),
                    timestamp,
                )?;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        pdf.check_file()?;
        require_source_current(&mut self.workflow, pdf.deadline)?;
        attempt.check_selected()?;
        attempt.generated_from_pdf(ledger, &mut evidence, result.answer.clone())?;
        self.place_answer(attempt, result, true, &mut |ledger| {
            pdf.check_file()?;
            evidence.with_current(ledger, || Ok(()))
        })
    }
}

//! Ordinary installed Reader and the retained original-file qualification path.
use super::{
    encode_model_images,
    researched::{INTENT_INSTRUCTIONS, VERIFY_PROMPT},
    source_submission::RefusedSubmission,
    AnalysisResult, Attempt, MarkedFragment, ModelContent, Orchestrator, Workflow, ANALYSIS_PROMPT,
    MARKED_QUESTION_PROMPT, SOURCE_SUBJECT_PROMPT,
};
use crate::{
    config::researched_reader::OrdinarySearch,
    conversation::{
        evidence::FollowupRequestHead,
        receiving::{memory::research::ResearchRefusal, ReceivingBudget, RequestSource},
        resolution::SourceDirective,
        AttemptReason, ContextBudget,
    },
    llm::LLMEngine,
    retrieval::{
        pdf_worker::{PendingIndex, Publication},
        IndexKey, Provider, Retrieved,
    },
    storage::{captured::Captured, Namespace},
    workflow::{
        conversation_dispatch::{followup_research, Dispatch},
        reader_intent::{self, Intent},
        resident_output::{format_exchange, ResidentOutputText},
        resident_source::original_pdf::{
            CapturedOriginalBook, ObservedOriginalBook, OrdinaryBookSource, OriginalFilePin,
        },
        AcquiredEvidence,
    },
};
use anyhow::{ensure, Context, Result};
use std::{
    cell::RefCell,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

type Acquisition = (AcquiredEvidence, Result<Option<OrdinaryBookSource>>);

pub(super) fn acquire(
    workflow: &mut Workflow,
    local: Option<&Dispatch<'_>>,
) -> Result<Acquisition> {
    #[cfg(test)]
    if workflow
        .resident_source
        .as_ref()
        .is_some_and(|owner| owner.original_book_fixture_selected())
    {
        return acquire_original_file(workflow, local);
    }
    #[cfg(not(test))]
    let _ = local;
    Ok((workflow.acquire_reader_evidence()?, Ok(None)))
}

// The original-file adapter needs explicit scope admission. Only the
// qualification fixture supplies that admission until a user flow exists.
#[cfg_attr(not(test), allow(dead_code))]
fn acquire_original_file(
    workflow: &mut Workflow,
    local: Option<&Dispatch<'_>>,
) -> Result<Acquisition> {
    if let Some(dispatch) = local.filter(|dispatch| {
        let reader = &dispatch.config.reader;
        workflow.uses_installed_reader_history()
            && dispatch.config.reader_enabled()
            && (reader.researched_pdf.is_some() || reader.researched_epub.is_some())
    }) {
        let owner = workflow
            .resident_source
            .as_mut()
            .context("Resident Reader owner absent")?;
        let health = workflow
            .service_health
            .as_mut()
            .context("Resident Reader health owner absent")?;
        let reader = &dispatch.config.reader;
        let (capture, book) = owner.acquire_ordinary_book(
            health,
            reader.researched_pdf.is_some(),
            reader.researched_epub.is_some(),
        )?;
        return Ok((
            AcquiredEvidence::ResidentHistorical(Box::new(capture)),
            book,
        ));
    }
    Ok((workflow.acquire_reader_evidence()?, Ok(None)))
}

pub(super) fn classify(
    workflow: &mut Workflow,
    source: Option<OrdinaryBookSource>,
) -> Result<Option<ObservedOriginalBook>> {
    let Some(source) = source else {
        return Ok(None);
    };
    let owner = workflow
        .resident_source
        .as_mut()
        .context("Resident Reader owner absent")?;
    let health = workflow
        .service_health
        .as_mut()
        .context("Resident Reader health owner absent")?;
    owner.read_ordinary_book(health, source)
}

struct OriginalBook {
    pin: RefCell<OriginalFilePin>,
    index: RefCell<Option<PendingIndex>>,
    key: IndexKey,
    deadline: Instant,
}

impl OriginalBook {
    fn new(book: CapturedOriginalBook, deadline: Instant) -> Self {
        let key = book.index.key().clone();
        Self {
            pin: RefCell::new(book.pin),
            index: RefCell::new(Some(book.index)),
            key,
            deadline,
        }
    }

    fn check_file(&self) -> Result<()> {
        self.pin.borrow_mut().check(self.deadline)
    }
}

struct OrdinarySource<'a> {
    book: Option<&'a OriginalBook>,
    deadline: Instant,
}

impl OrdinarySource<'_> {
    fn check_deadline(&self) -> Result<()> {
        ensure!(
            Instant::now() < self.deadline,
            "Ordinary Reader request expired"
        );
        Ok(())
    }

    fn check_file(&self) -> Result<()> {
        self.book.map_or(Ok(()), OriginalBook::check_file)
    }

    fn require_current(&self, workflow: &mut Workflow) -> Result<()> {
        workflow.check_request_progress()?;
        let owner = workflow
            .resident_source
            .as_mut()
            .context("Resident Reader owner absent")?;
        let health = workflow
            .service_health
            .as_mut()
            .context("Resident Reader health owner absent")?;
        owner.require_ordinary_book_current(health, self.deadline)?;
        // Observe can block even when it returns the original page and order.
        // Sample both files after that wait, just before the Store boundary.
        self.check_file()
    }
}

struct ExactQuery<'a> {
    book: &'a OriginalBook,
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
            expected == &self.book.key && question == self.question,
            "Original document query changed"
        );
        self.book.check_file()?;
        let mut progress = self.progress.borrow_mut();
        progress()?;
        let index = self
            .book
            .index
            .borrow_mut()
            .take()
            .context("Original document index already used")?;
        let passages = index.query(
            expected,
            question,
            max_passages,
            max_bytes,
            neighbors,
            self.book.deadline,
            &mut **progress,
        )?;
        self.book.check_file()?;
        Ok(passages)
    }
}

impl<M: LLMEngine> Orchestrator<M> {
    pub(super) fn run_ordinary_book(
        &mut self,
        attempt: &mut Attempt,
        observed: ObservedOriginalBook,
        dispatch: &Dispatch<'_>,
    ) -> Result<()> {
        // A refused or interrupted book request consumes the retained slot.
        let retained = self.ordinary_book_cache.take();
        dispatch.require_ledger(&self.ledger)?;
        let prepared = attempt.capture_pdf_reading(&self.ledger)?;
        let config = dispatch.config;
        let declaration = match observed.publication() {
            Publication::Pdf { .. } => config.reader.researched_pdf.as_ref(),
            Publication::Epub => config.reader.researched_epub.as_ref(),
        }
        .context("Ordinary document research is disabled for this format")?;
        let search = declaration
            .research_model
            .as_deref()
            .map(OrdinarySearch::for_installed_reader);
        let images = self.prepare_model_images(attempt)?;
        let owner = self
            .workflow
            .resident_source
            .as_mut()
            .context("Resident Reader owner absent")?;
        let health = self
            .workflow
            .service_health
            .as_mut()
            .context("Resident Reader health owner absent")?;
        let deadline = owner.ordinary_book_deadline()?;
        let (captured, cache) =
            owner.complete_ordinary_book(health, observed, deadline, retained)?;
        #[cfg(test)]
        super::researched_fixture::record_ordinary_pdf_worker(&captured.index);
        let book = OriginalBook::new(captured, deadline);
        self.run_ordinary_source(
            attempt,
            &prepared,
            &OrdinarySource {
                book: Some(&book),
                deadline,
            },
            dispatch,
            search,
            encode_model_images(images),
        )?;
        self.ordinary_book_cache = cache;
        Ok(())
    }

    pub(super) fn run_ordinary_captured_page(
        &mut self,
        attempt: &mut Attempt,
        dispatch: &Dispatch<'_>,
    ) -> Result<()> {
        dispatch.require_ledger(&self.ledger)?;
        let prepared = attempt.capture_pdf_reading(&self.ledger)?;
        let deadline = self
            .workflow
            .resident_source
            .as_ref()
            .context("Resident Reader owner absent")?
            .ordinary_book_deadline()?;
        let images = self.prepare_model_images(attempt)?;
        self.run_ordinary_source(
            attempt,
            &prepared,
            &OrdinarySource {
                book: None,
                deadline,
            },
            dispatch,
            Some(OrdinarySearch::for_installed_page()),
            encode_model_images(images),
        )
    }

    fn run_ordinary_source(
        &mut self,
        attempt: &mut Attempt,
        prepared: &Captured,
        source: &OrdinarySource<'_>,
        dispatch: &Dispatch<'_>,
        search: Option<OrdinarySearch<'_>>,
        images: Vec<String>,
    ) -> Result<()> {
        let policy = dispatch.config.research_policy_snapshot()?.policy();
        let prompt = format!(
            "{}\n{INTENT_INSTRUCTIONS}\nTranscribe and locate the request only. Use ANSWER: Deferred as the format placeholder. The actual answer is generated later from verified context.",
            ANALYSIS_PROMPT.replace(
                "QUESTION: [question]\n",
                "QUESTION: [question]\nREQUEST_WEB_JSON: [one intent metadata JSON object]\n",
            )
        );
        let source_page = attempt.current_reader_answer_page().is_none();
        let prompt = if source_page {
            format!("{prompt}\n{MARKED_QUESTION_PROMPT}\nIf a readable marked fragment has no usable selection, retain it with ANSWER: NONE. Otherwise use ANSWER: Deferred.")
        } else {
            prompt
        };
        let prompt = if attempt.source_target_enabled() {
            prompt
        } else {
            format!("{prompt}\n{SOURCE_SUBJECT_PROMPT}")
        };
        let proposed =
            self.read_ordinary_source(attempt, prepared, source, &prompt, images, false)?;
        if proposed.trim().eq_ignore_ascii_case("NONE") {
            source.require_current(&mut self.workflow)?;
            prepared.with_current(self.ledger.store(), || Ok(()))?;
            let opened = self.workflow.opened_submission().is_some();
            self.workflow.report_failure(super::Failure::Selection)?;
            attempt.outcome(
                &self.ledger,
                false,
                crate::conversation::Outcome::Failed,
                AttemptReason::NoSelection,
            )?;
            return if opened {
                Err(RefusedSubmission(AttemptReason::NoSelection).into())
            } else {
                Ok(())
            };
        }
        let marked = source_page
            .then(|| Self::parse_marked_fragment(&proposed))
            .flatten();
        let proposed_answer = (|| -> Result<_> {
            let separator =
                Self::analysis_separator(&proposed).context("Reader question header absent")?;
            let analysis = reader_intent::parse_ordinary_pdf_header(&proposed[..separator])?;
            let result = Self::parse_analysis_response(&proposed)
                .context("Reader question or selection unavailable")?;
            Ok((analysis, result))
        })();
        let (analysis, mut result) = match (proposed_answer, &marked) {
            (Ok(value), _) => value,
            (Err(_), Some(marked)) => {
                return self.preserve_ordinary_marked_question(attempt, prepared, source, marked);
            }
            (Err(error), None) => return Err(error),
        };
        let bounds = result
            ._question_box
            .as_ref()
            .context("Reader question bounds unavailable")?;
        ensure!(
            (0..1024).contains(&bounds.y) && bounds.height > 0,
            "Reader question bounds invalid"
        );

        // The independent reader receives the acknowledged images again and
        // none of the first reply, geometry or web-intent claims.
        let verification = encode_model_images(attempt.images(&self.ledger)?);
        let verification = self.read_ordinary_source(
            attempt,
            prepared,
            source,
            &format!("{VERIFY_PROMPT}\n{INTENT_INSTRUCTIONS}"),
            verification,
            true,
        );
        let agreement = match (verification, &marked) {
            (Ok(verified), _) => reader_intent::parse_verification_response(&verified)
                .and_then(|reading| reader_intent::agree(analysis, reading)),
            (Err(error), Some(marked)) if error.is::<crate::llm::ProviderRefused>() => {
                return self.preserve_ordinary_marked_question(attempt, prepared, source, marked);
            }
            (Err(error), _) => return Err(error),
        };
        let agreement = match (agreement, &marked) {
            (Ok(value), _) => value,
            (Err(_), Some(marked)) => {
                return self.preserve_ordinary_marked_question(attempt, prepared, source, marked);
            }
            (Err(error), None) => return Err(error.into()),
        };
        ensure!(
            agreement.question() == result.question,
            "Verified Reader question changed"
        );
        let directive = match agreement.intent() {
            Intent::NoInternet => {
                crate::conversation::receiving::memory::research::ResearchDirective::NoInternet
            }
            Intent::NoRestriction => {
                crate::conversation::receiving::memory::research::ResearchDirective::UsePolicy
            }
            Intent::Unclear => anyhow::bail!("Reader question web intent is unclear"),
        };
        source.require_current(&mut self.workflow)?;
        let head = attempt.interpret_ordinary_pdf(
            &self.ledger,
            &result.question,
            result.target.as_ref(),
            Some(&result.selection),
            prepared,
            marked.as_ref().map(|marked| marked.region),
        )?;
        #[cfg(test)]
        super::researched_fixture::after_ordinary_pdf_interpret(&self.ledger, &attempt.user)?;
        if attempt.source_target_enabled() {
            result.about = Some(match result.target.as_ref() {
                Some(candidate) => candidate.about()?,
                None => "About: Uncertain - Selected source could not be localized".into(),
            });
        }
        let research = followup_research::ResearchRequest {
            directive,
            search,
            deadline: source.deadline,
            original_image_only: source.book.is_none(),
        };
        self.answer_ordinary_source(
            attempt,
            source,
            dispatch,
            policy,
            research,
            &mut result,
            head,
            marked.is_some(),
        )
    }

    fn preserve_ordinary_marked_question(
        &mut self,
        attempt: &mut Attempt,
        prepared: &Captured,
        source: &OrdinarySource<'_>,
        marked: &MarkedFragment,
    ) -> Result<()> {
        source.require_current(&mut self.workflow)?;
        let head = attempt.interpret_ordinary_pdf(
            &self.ledger,
            &marked.text,
            marked.target.as_ref(),
            None,
            prepared,
            Some(marked.region),
        )?;
        let current = head.capture_interpreted(&self.ledger, &attempt.user)?;
        if self.workflow.opened_submission().is_some() {
            return Err(RefusedSubmission(AttemptReason::NoSelection).into());
        }
        self.present_ordinary_outcome(
            attempt,
            source,
            ResidentOutputText::ReaderQuestion(&marked.text),
            &mut |ledger| current.with_current(ledger.store(), || Ok(())),
        )
    }

    fn present_ordinary_outcome(
        &mut self,
        attempt: &Attempt,
        source: &OrdinarySource<'_>,
        output: ResidentOutputText<'_>,
        current: &mut dyn FnMut(&crate::conversation::Ledger) -> Result<()>,
    ) -> Result<()> {
        source.require_current(&mut self.workflow)?;
        let ledger = &self.ledger;
        current(ledger)?;
        let mut source_checked_at_output = false;
        let outcome = self.workflow.render_resident_answer(
            ledger,
            attempt.user.conversation,
            output,
            attempt.resident_answer.as_ref(),
            &mut || {
                current(ledger)?;
                // Bound placement cannot insert. Keep checking the original
                // file across navigation. Only a fresh marked placement can
                // change the document table as part of Buddy's own insertion.
                if attempt.resident_answer.is_some() || !source_checked_at_output {
                    source.check_file()?;
                    source_checked_at_output = true;
                }
                Ok(())
            },
        )?;
        log::info!("Saved Reader outcome Buddy placement: {outcome:?}");
        Ok(())
    }

    fn read_ordinary_source(
        &mut self,
        attempt: &Attempt,
        prepared: &Captured,
        source: &OrdinarySource<'_>,
        prompt: &str,
        images: Vec<String>,
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
                    &images,
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
            for image in &images {
                model.0.add_image_content(image);
            }
        }
        drop(images);
        if !verification && attempt.source_target_enabled() {
            model
                .0
                .add_text_content(crate::conversation::target::GEOMETRY_CONTRACT);
        }
        if !verification {
            source.check_file()?;
            self.workflow.deliver_reader_acquisition()?;
        }
        let workflow = RefCell::new(&mut self.workflow);
        let store = self.ledger.store();
        let progress = || {
            source.check_deadline()?;
            attempt.model_progress()?;
            workflow.borrow_mut().check_request_progress()?;
            prepared.with_current(store, || Ok(()))
        };
        let submit = |call: &mut dyn FnMut() -> Result<()>| {
            progress()?;
            source.require_current(&mut workflow.borrow_mut())?;
            prepared.with_current(store, call)
        };
        let reply = followup_research::once(&submit, |handoff| {
            model
                .0
                .execute_with_deadline(&mut || progress(), handoff, source.deadline)
        })?;
        progress()?;
        Ok(reply)
    }

    #[allow(clippy::too_many_arguments)]
    fn answer_ordinary_source(
        &mut self,
        attempt: &mut Attempt,
        source: &OrdinarySource<'_>,
        dispatch: &Dispatch<'_>,
        policy: crate::conversation::receiving::memory::research::ResearchPolicy,
        research: followup_research::ResearchRequest<'_>,
        result: &mut AnalysisResult,
        head: FollowupRequestHead,
        marked: bool,
    ) -> Result<()> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_millis()
            .try_into()?;
        let ledger = &self.ledger;
        let conversation = attempt.user.conversation;
        let user = attempt.user.id;
        let expected = head.expected_root();
        let generation = head.generation();
        let prepared = {
            let mut progress = || {
                source.check_deadline()?;
                attempt.model_progress()?;
                self.workflow.check_request_progress()?;
                let (live_generation, heads) = ledger
                    .store()
                    .conditional_snapshot(Namespace::Conversation, conversation)?;
                ensure!(
                    live_generation == generation
                        && matches!(heads.as_slice(), [head] if expected.0.contains(&head.revision_id)),
                    "Reader conversation changed during retrieval"
                );
                Ok(())
            };
            let provider = source.book.map(|book| ExactQuery {
                book,
                question: &result.question,
                progress: RefCell::new(&mut progress),
            });
            let source_record = attempt.retained_source();
            let directive = if let Some(book) = source.book {
                SourceDirective::Explicit(RequestSource::Original {
                    origin_turn: user,
                    source_record,
                    expected: book.key.clone(),
                })
            } else if let Some(binding) = attempt.current_reader_answer_page() {
                let original = ledger.bound_resident_reader_capture(&attempt.user, binding)?;
                SourceDirective::Explicit(original.map_or(RequestSource::NoDocument, |capture| {
                    RequestSource::CapturedPage {
                        origin_turn: capture.turn,
                        source_record: capture.id,
                    }
                }))
            } else {
                SourceDirective::Explicit(RequestSource::CapturedPage {
                    origin_turn: user,
                    source_record,
                })
            };
            ledger.prepare_declared_evidence(
                conversation,
                user,
                None,
                directive,
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
                provider.as_ref().map(|provider| provider as &dyn Provider),
                |text| Ok(text.len()),
                Some(head),
                None,
                if source.book.is_none() {
                    crate::conversation::receiving::FollowupMedia::BoundPage
                } else {
                    crate::conversation::receiving::FollowupMedia::All
                },
            )?
        };
        ensure!(
            prepared.context().request() == &attempt.user
                && prepared.original_root() == expected
                && prepared.original_generation() == generation,
            "Interpreted Reader request changed"
        );
        source.require_current(&mut self.workflow)?;
        let (context, mut evidence) = ledger.persist_followup_evidence(
            crate::writer::stage(attempt.user.exchange, "reader-pdf-evidence"),
            timestamp,
            prepared,
        )?;
        attempt.advance_pdf_evidence(ledger, &expected, evidence.root())?;
        let answer = {
            let workflow = RefCell::new(&mut self.workflow);
            let progress = || {
                source.check_deadline()?;
                workflow.borrow_mut().check_request_progress()
            };
            let check = || {
                progress()?;
                evidence.with_current(ledger, || Ok(()))
            };
            let submit = |call: &mut dyn FnMut() -> Result<()>| {
                progress()?;
                attempt.check_selected()?;
                source.require_current(&mut workflow.borrow_mut())?;
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
            Err(error)
                if error.is::<crate::llm::ProviderRefused>()
                    || ((source.book.is_none() || marked)
                        && matches!(
                            error.downcast_ref::<ResearchRefusal>(),
                            Some(ResearchRefusal::Unavailable)
                        )) =>
            {
                attempt.check_selected()?;
                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)?
                    .as_millis()
                    .try_into()?;
                source.require_current(&mut self.workflow)?;
                let (failed, fact) = evidence.record_provider_failure(
                    ledger,
                    crate::writer::stage(attempt.user.exchange, "reader-pdf-provider-failure"),
                    attempt.user.clone(),
                    timestamp,
                )?;
                attempt.user = failed;
                let current = &mut |ledger: &crate::conversation::Ledger| {
                    evidence.with_current(ledger, || Ok(()))
                };
                if self.workflow.opened_submission().is_some() {
                    return Err(error);
                }
                if marked {
                    self.present_ordinary_outcome(
                        attempt,
                        source,
                        ResidentOutputText::ReaderQuestion(&result.question),
                        current,
                    )
                    .with_context(|| format!("Reader provider refused: {error:#}"))?;
                } else if attempt.resident_answer.is_some() {
                    let feedback = crate::conversation_feedback::describe(&fact)?
                        .context("Saved Reader provider failure has no feedback")?;
                    let text = format_exchange(&result.question, feedback.text);
                    if let Err(output_error) = self.present_ordinary_outcome(
                        attempt,
                        source,
                        ResidentOutputText::Plain(&text),
                        current,
                    ) {
                        // Leave the typed failure in the owned conversation and
                        // preserve the original provider result for the caller.
                        log::warn!("Reader failure feedback placement: {output_error:#}");
                    }
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        attempt.check_selected()?;
        if self.workflow.opened_submission().is_some() {
            // A returned answer belongs to the retained source and its exact
            // conversation. Current page authority is checked separately at output.
            source.check_deadline()?;
            source.check_file()?;
        } else {
            source.require_current(&mut self.workflow)?;
        }
        attempt.generated_from_pdf(ledger, &mut evidence, result.answer.clone())?;
        self.place_answer(attempt, result, true, &mut |ledger| {
            source.check_file()?;
            evidence.with_current(ledger, || Ok(()))
        })
    }
}

//! Current acknowledged data only. This state never supplies native permission.
use super::*;
use crate::storage::selection::SelectionToken;
use crate::storage::sync::mode::SyncModeLease;
#[cfg(any(target_os = "linux", test))]
use crate::storage::sync::selected::{ReaderPhase, ReaderPreferences};
#[cfg(any(target_os = "linux", test))]
use crate::storage::Store;
use std::sync::Arc;
#[cfg(target_os = "linux")]
use {
    crate::{
        conversation::receiving::memory::research::{ResearchDirective, ResearchPolicy},
        retrieval::{pdf_worker::PendingIndex, IndexKey, Retrieved},
        workflow::{
            reader_intent::{Agreement, Intent},
            resident_source::original_pdf::{CapturedOriginalPdf, OriginalPdfPin},
        },
    },
    std::{
        cell::{Cell, RefCell},
        time::{Duration, Instant},
    },
};

type ReaderMemory<'a> = Option<(
    &'a dyn crate::memory::Provider,
    &'a crate::memory::QueryResult,
)>;

#[cfg(target_os = "linux")]
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::workflow) enum ResearchStage {
    Analysis,
    Verification,
    Research,
    Answer,
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum ResearchProgress {
    Prepared,
    Analyzed,
    Verified,
    Interpreted,
    ContextReady,
    Retrieved,
    Researched,
    Answered,
    Generated,
    Failed,
}

/// Exact selected source and request records admitted by a live Attempt.
#[cfg(target_os = "linux")]
#[allow(dead_code)]
pub(crate) struct ResearchContext {
    conversation: Uuid,
    user: Uuid,
    capture: Uuid,
    key: IndexKey,
    records: Vec<Record>,
    media: Vec<crate::storage::ObjectRef>,
}

#[cfg(target_os = "linux")]
#[allow(dead_code)]
impl ResearchContext {
    pub(crate) fn conversation(&self) -> Uuid {
        self.conversation
    }
    pub(crate) fn request_turn(&self) -> Uuid {
        self.user
    }
    pub(crate) fn capture(&self) -> Uuid {
        self.capture
    }
    pub(crate) fn index_key(&self) -> &IndexKey {
        &self.key
    }
    pub(crate) fn media_available(&self, media: &Media) -> bool {
        self.media
            .iter()
            .any(|object| object.sha256 == media.sha256 && object.bytes == media.bytes)
    }
    pub(crate) fn into_records(self) -> Vec<Record> {
        self.records
    }
    pub(crate) fn records(&self) -> &[Record] {
        &self.records
    }
}

#[cfg(target_os = "linux")]
struct RequestBinding {
    token: SelectionToken,
    root: IntentRecordRef,
    user: IntentRecordRef,
}

#[cfg(target_os = "linux")]
impl RequestBinding {
    fn capture(selected: &Selected) -> Self {
        Self {
            token: selected.token.clone(),
            root: selected.root.clone(),
            user: selected.user.clone(),
        }
    }

    fn matches(&self, selected: &Selected) -> bool {
        self.token == selected.token && self.root == selected.root && self.user == selected.user
    }
}

#[cfg(target_os = "linux")]
struct AdmittedIntent {
    agreement: Agreement,
    directive: ResearchDirective,
    prepared: RequestBinding,
    interpreted: Option<RequestBinding>,
    fingerprint: String,
}

#[cfg(target_os = "linux")]
struct ResearchAttempt {
    pin: RefCell<OriginalPdfPin>,
    index: RefCell<Option<PendingIndex>>,
    key: IndexKey,
    capture: IntentRecordRef,
    image_digests: Vec<String>,
    original_image_digest: String,
    policy: ResearchPolicy,
    memory_required: bool,
    pending: Cell<ResearchProgress>,
    images_checked: Cell<bool>,
    intent: RefCell<Option<AdmittedIntent>>,
    memory: RefCell<Option<crate::memory::QueryResult>>,
}

#[cfg(target_os = "linux")]
impl ResearchAttempt {
    fn check_pin(&self, deadline: Instant) -> Result<()> {
        self.pin
            .try_borrow_mut()
            .context("Selected Reader PDF pin reentered")?
            .check(deadline)
    }

    fn check_memory(&self, memory: ReaderMemory<'_>) -> Result<()> {
        let admitted = self.memory.borrow();
        match (self.memory_required, admitted.as_ref(), memory) {
            (false, None, None) => Ok(()),
            (true, Some(admitted), Some((_, supplied))) => {
                anyhow::ensure!(
                    supplied.selected()
                        && supplied.status == admitted.status
                        && supplied.diagnostics == admitted.diagnostics
                        && supplied.context == admitted.context,
                    "Selected Reader research memory changed"
                );
                Ok(())
            }
            _ => anyhow::bail!("Selected Reader research memory admission is absent or unexpected"),
        }
    }
}

struct Frozen {
    accepted: SelectionToken,
    request: SelectedReaderWrite,
    acknowledged: Option<SelectedReaderAcknowledgment>,
    #[cfg(target_os = "linux")]
    research_intent: Option<String>,
}
pub(super) struct Selected {
    admission: Arc<SelectedAdmission>,
    token: SelectionToken,
    root: IntentRecordRef,
    user: IntentRecordRef,
    capture: Uuid,
    targets: bool,
    mode: Option<SyncModeLease>,
    interpretation: Option<Frozen>,
    generation: Option<Frozen>,
    #[cfg(any(target_os = "linux", test))]
    resident: Option<ReaderPreferences>,
    #[cfg(any(target_os = "linux", test))]
    model_stage: std::cell::Cell<u8>,
    #[cfg(any(target_os = "linux", test))]
    handwriting: Option<crate::handwriting::ConfirmedContext>,
    #[cfg(target_os = "linux")]
    research: Option<ResearchAttempt>,
}
impl Selected {
    #[cfg(test)]
    pub(super) fn frozen_operation(&self, generated: bool) -> Uuid {
        let frozen = if generated {
            &self.generation
        } else {
            &self.interpretation
        };
        frozen.as_ref().unwrap().request.operation
    }
    pub(super) fn new(
        ledger: &Ledger,
        workflow: &super::super::Workflow,
        token: SelectionToken,
        root: IntentRecordRef,
        user: IntentRecordRef,
        capture: Uuid,
    ) -> Result<(Self, Turn)> {
        let mut selected = Self::input(ledger, workflow, token, root, user, capture)?;
        selected.mode = selected.admission.reader_mode_lease()?;
        let turn = selected.current_user()?;
        Self::require_prepared(&turn, capture)?;
        selected.images()?;
        Ok((selected, turn))
    }

    fn input(
        ledger: &Ledger,
        workflow: &super::super::Workflow,
        token: SelectionToken,
        root: IntentRecordRef,
        user: IntentRecordRef,
        capture: Uuid,
    ) -> Result<Self> {
        let admission = workflow.selected_admission()?;
        anyhow::ensure!(
            ledger.store().unavailable_commits()? == 0,
            "selected input Store incomplete"
        );
        anyhow::ensure!(
            admission.uses_store(ledger.store()),
            "selected Attempt has foreign Store"
        );
        Ok(Self {
            admission,
            token,
            root,
            user,
            capture,
            targets: ledger.store().source_target_generation()?.is_some(),
            mode: None,
            interpretation: None,
            generation: None,
            #[cfg(any(target_os = "linux", test))]
            resident: None,
            #[cfg(any(target_os = "linux", test))]
            model_stage: std::cell::Cell::new(0),
            #[cfg(any(target_os = "linux", test))]
            handwriting: None,
            #[cfg(target_os = "linux")]
            research: None,
        })
    }

    fn require_prepared(turn: &Turn, capture: Uuid) -> Result<()> {
        anyhow::ensure!(
            turn.role == Role::User
                && turn.mode == Mode::Reader
                && turn.outcome == Outcome::Prepared
                && turn.text.is_none()
                && turn.sources == [capture],
            "selected input is not acknowledged Prepared capture"
        );
        Ok(())
    }
    fn mode(&self) -> Result<()> {
        if let Some(mode) = &self.mode {
            mode.require_shared_scope(self.token.scope())?;
        } else {
            anyhow::ensure!(
                self.admission.reader_mode_lease()?.is_none(),
                "unconfigured selected mode changed"
            );
        }
        Ok(())
    }
    fn current_user(&self) -> Result<Turn> {
        self.mode()?;
        self.admission.with_current(&self.token, |snapshot| {
            self.project_user(&SelectedDomainProjection::from_snapshot(snapshot)?)
        })
    }

    fn project_user(&self, projection: &SelectedDomainProjection) -> Result<Turn> {
        projection.inspect(self.root.record_id)?;
        let Record::Root(root) = projection.exact_reference(&self.root)? else {
            anyhow::bail!("selected input root variant")
        };
        let Record::Turn(user) = projection.exact_reference(&self.user)? else {
            anyhow::bail!("selected input user variant")
        };
        for reference in [&self.root, &self.user] {
            let (head, Some(_)) = projection
                .head(reference.namespace, reference.record_id)
                .context("selected input head absent")?
            else {
                anyhow::bail!("selected input head deleted")
            };
            anyhow::ensure!(
                head.revision_id == reference.revision_id,
                "selected input head replaced"
            );
        }
        anyhow::ensure!(
            user.conversation == root.id && user.sources == [self.capture],
            "selected input capture link differs"
        );
        Ok(user.clone())
    }
    pub(super) fn check(&self) -> Result<()> {
        #[cfg(any(target_os = "linux", test))]
        if let Some(policy) = &self.resident {
            return self.with_resident(policy, ReaderPhase::Result, |_, _| Ok(()));
        }
        self.current_user().map(|_| ())
    }
    pub(super) fn document(&self) -> Result<Option<Uuid>> {
        let document = |snapshot: &crate::storage::selection::SelectedSnapshot| {
            let owners = SelectedDomainProjection::from_snapshot(snapshot)?.document_ownership()?;
            Ok(match owners.get(&self.root.record_id) {
                Some(DocumentOwnership::Document(document)) => Some(*document),
                _ => None,
            })
        };
        #[cfg(any(target_os = "linux", test))]
        if let Some(policy) = &self.resident {
            return self.with_resident(policy, ReaderPhase::Result, |store, reads| {
                let snapshot = store
                    .selected_snapshot_with_budget(
                        self.admission.scope(),
                        crate::storage::MAX_ITEMS,
                        reads,
                    )?
                    .context("selected Reader document unavailable")?
                    .snapshot;
                anyhow::ensure!(
                    snapshot.token == self.token,
                    "selected Reader document changed"
                );
                document(&snapshot)
            });
        }
        self.check()?;
        self.admission.with_current(&self.token, document)
    }

    #[cfg(any(target_os = "linux", test))]
    pub(super) fn query_memory(
        &self,
        memory: &dyn crate::memory::Provider,
        query: &crate::memory::Query,
    ) -> Result<crate::memory::QueryResult> {
        let Some(policy) = &self.resident else {
            return memory.query(query);
        };
        let reserved = {
            let mut reads = policy.reads(ReaderPhase::Result)?;
            crate::memory::selected::reserve_reader_query(&mut reads)?
        };
        #[cfg(target_os = "linux")]
        if let Some(research) = &self.research {
            let (question, _, _) = self.research_intent(true)?;
            anyhow::ensure!(
                research.pending.get() == ResearchProgress::Retrieved
                    && research.memory_required
                    && research.memory.borrow().is_none()
                    && query.document == Some(research.key.document)
                    && query.text == question,
                "Selected Reader research memory does not match the request"
            );
            research.pending.set(ResearchProgress::Failed);
            let value = memory.query_selected_reader(query, reserved)?;
            anyhow::ensure!(
                value.selected(),
                "Selected Reader research memory is unguarded"
            );
            research.memory.replace(Some(value.clone()));
            research.pending.set(ResearchProgress::Retrieved);
            return Ok(value);
        }
        memory.query_selected_reader(query, reserved)
    }
    pub(super) fn images(&self) -> Result<Vec<Vec<u8>>> {
        let images = self.capture_images()?;
        let bytes: Vec<Vec<u8>> = match images {
            SelectedCaptureImages::Sdk(images) => {
                images.images.into_iter().map(|image| image.bytes).collect()
            }
            SelectedCaptureImages::Development(images) => images.images,
            SelectedCaptureImages::Legacy(images) => images.images,
            SelectedCaptureImages::Resident(images) => images.images,
        };
        #[cfg(target_os = "linux")]
        if let Some(research) = &self.research {
            anyhow::ensure!(
                bytes
                    .iter()
                    .map(|image| digest(image))
                    .eq(research.image_digests.iter().cloned()),
                "Selected Reader PDF image batch changed"
            );
            research.images_checked.set(true);
        }
        Ok(bytes)
    }

    fn capture_images(&self) -> Result<SelectedCaptureImages> {
        #[cfg(any(target_os = "linux", test))]
        if let Some(policy) = &self.resident {
            return self.with_resident(policy, ReaderPhase::Result, |store, reads| {
                SelectedAdmission::capture_images_locked(store, &self.token, self.capture, reads)
            });
        }
        self.check()?;
        let images = self.admission.capture_images(&self.token, self.capture)?;
        self.check()?;
        Ok(images)
    }

    #[cfg(any(target_os = "linux", test))]
    pub(super) fn new_resident(
        ledger: &Ledger,
        workflow: &super::super::Workflow,
        token: SelectionToken,
        acknowledgment: crate::conversation::selected_reader_prepare::ResidentPreparedAcknowledgment,
        policy: ReaderPreferences,
    ) -> Result<(Self, Turn)> {
        Self::new_resident_with_source(ledger, workflow, token, acknowledgment, policy, |_, _| {
            Ok(())
        })
    }

    #[cfg(any(target_os = "linux", test))]
    fn new_resident_with_source(
        ledger: &Ledger,
        workflow: &super::super::Workflow,
        token: SelectionToken,
        acknowledgment: crate::conversation::selected_reader_prepare::ResidentPreparedAcknowledgment,
        policy: ReaderPreferences,
        capture_selected: impl FnOnce(&IntentRecordRef, &ResidentCaptureHistory) -> Result<()>,
    ) -> Result<(Self, Turn)> {
        let mut selected = Self::input(
            ledger,
            workflow,
            token,
            acknowledgment.root,
            acknowledgment.user,
            acknowledgment.capture.record_id,
        )?;
        let turn = selected.with_resident(&policy, ReaderPhase::Acquisition, |store, reads| {
            let snapshot = store
                .selected_snapshot_with_budget(
                    selected.admission.scope(),
                    crate::storage::MAX_ITEMS,
                    reads,
                )?
                .context("selected Reader Prepared input absent")?
                .snapshot;
            anyhow::ensure!(
                snapshot.token == selected.token,
                "selected Reader Prepared input changed"
            );
            let projection = SelectedDomainProjection::from_snapshot(&snapshot)?;
            anyhow::ensure!(
                projection.current_reference(acknowledgment.capture.namespace, selected.capture)?
                    == acknowledgment.capture,
                "selected Reader Prepared source differs"
            );
            let Record::ResidentCapture(capture) =
                projection.exact_reference(&acknowledgment.capture)?
            else {
                anyhow::bail!("selected Reader Prepared source differs");
            };
            capture_selected(&acknowledgment.capture, capture)?;
            let turn = selected.project_user(&projection)?;
            Self::require_prepared(&turn, selected.capture)?;
            Ok(turn)
        })?;
        selected.resident = Some(policy);
        Ok((selected, turn))
    }

    #[cfg(target_os = "linux")]
    pub(super) fn new_researched_resident(
        ledger: &Ledger,
        workflow: &super::super::Workflow,
        token: SelectionToken,
        acknowledgment: crate::conversation::selected_reader_prepare::ResidentPreparedAcknowledgment,
        policy: ReaderPreferences,
        pdf: CapturedOriginalPdf,
    ) -> Result<(Self, Turn)> {
        let research_policy = policy.config().research_policy_snapshot()?.policy();
        let memory_required = policy.config().memory.enabled;
        let CapturedOriginalPdf { pin, index } = pdf;
        let key = index.key().clone();
        anyhow::ensure!(
            policy.admitted_document()? == key.document,
            "Selected Reader PDF belongs to a different admitted document"
        );
        let mut source = None;
        let (mut selected, user) = Self::new_resident_with_source(
            ledger,
            workflow,
            token,
            acknowledgment,
            policy,
            |reference, capture| {
                anyhow::ensure!(
                    capture.before.document == key.document
                        && capture.after.document == key.document
                        && capture.images.len() == 4,
                    "Selected Reader PDF and Prepared capture differ"
                );
                source = Some((
                    reference.clone(),
                    capture.original_png.sha256.clone(),
                    capture
                        .images
                        .iter()
                        .map(|image| image.media.sha256.clone())
                        .collect(),
                ));
                Ok(())
            },
        )?;
        let (capture, original_image_digest, image_digests) =
            source.context("Selected Reader PDF source absent")?;
        selected.research = Some(ResearchAttempt {
            pin: RefCell::new(pin),
            index: RefCell::new(Some(index)),
            key,
            capture,
            image_digests,
            original_image_digest,
            policy: research_policy,
            memory_required,
            pending: Cell::new(ResearchProgress::Prepared),
            images_checked: Cell::new(false),
            intent: RefCell::new(None),
            memory: RefCell::new(None),
        });
        Ok((selected, user))
    }

    #[cfg(any(target_os = "linux", test))]
    pub(super) fn is_resident(&self) -> bool {
        self.resident.is_some()
    }

    #[cfg(any(target_os = "linux", test))]
    pub(super) fn capture_handwriting(
        &mut self,
        targets: &std::collections::BTreeSet<String>,
    ) -> Result<()> {
        if let Some(policy) = &self.resident {
            anyhow::ensure!(
                self.handwriting.is_none(),
                "selected handwriting already captured"
            );
            if let Some(captured) =
                policy.capture_handwriting(&self.admission, &self.token, targets)?
            {
                log::debug!(
                    "Selected handwriting selection {}",
                    serde_json::json!({
                        "policy": crate::handwriting::runtime::POLICY,
                        "selection": captured.selection,
                        "selection_status": captured.selection.status(),
                        "context": captured.context.identity(),
                    })
                );
                anyhow::ensure!(
                    captured.selection.status() != "partial-empty",
                    "selected handwriting context incomplete"
                );
                self.handwriting = Some(captured.context);
            }
        }
        Ok(())
    }

    #[cfg(any(target_os = "linux", test))]
    pub(super) fn handwriting(&self) -> Option<&crate::handwriting::ConfirmedContext> {
        self.handwriting
            .as_ref()
            .filter(|context| !context.identity().examples.is_empty())
    }

    #[cfg(any(target_os = "linux", test))]
    fn handwriting_owner(
        &self,
    ) -> Result<Option<&crate::storage::sync::selected::SelectedContextOwner>> {
        self.handwriting
            .as_ref()
            .map(crate::handwriting::ConfirmedContext::selected_owner)
            .transpose()
    }

    #[cfg(any(target_os = "linux", test))]
    fn with_resident<T>(
        &self,
        policy: &ReaderPreferences,
        phase: ReaderPhase,
        consume: impl FnOnce(&Store, &mut crate::storage::selection::SelectedReadBudget) -> Result<T>,
    ) -> Result<T> {
        let mut reads = policy.reads(phase)?;
        let [catalog, preference] = policy.gates();
        SelectedAdmission::with_gates(&[self.admission.as_ref(), catalog, preference], |store| {
            policy.verify_locked(store, &self.admission, &self.token, &mut reads)?;
            policy.require_phase_time(phase)?;
            consume(store, &mut reads)
        })
    }

    #[cfg(any(target_os = "linux", test))]
    pub(super) fn progress(&self) -> Result<()> {
        if let Some(policy) = &self.resident {
            return policy.check_progress(&self.admission, &self.token);
        }
        self.check()
    }

    #[cfg(target_os = "linux")]
    pub(super) fn researched(&self) -> bool {
        self.research.is_some()
    }

    #[cfg(target_os = "linux")]
    fn research(&self) -> Result<&ResearchAttempt> {
        self.research
            .as_ref()
            .context("Selected Reader research is absent")
    }

    #[cfg(target_os = "linux")]
    fn research_policy(&self) -> Result<&ReaderPreferences> {
        self.resident
            .as_ref()
            .context("Selected Reader research is not resident")
    }

    #[cfg(target_os = "linux")]
    pub(super) fn admit_research_intent(&self, agreement: Agreement) -> Result<()> {
        let research = self.research()?;
        anyhow::ensure!(
            research.pending.get() == ResearchProgress::Verified
                && research.intent.borrow().is_none(),
            "Selected Reader intent admission is not fresh"
        );
        research.pending.set(ResearchProgress::Failed);
        let directive = match agreement.intent() {
            Intent::NoRestriction => ResearchDirective::UsePolicy,
            Intent::NoInternet => ResearchDirective::NoInternet,
            Intent::Unclear => anyhow::bail!("Selected Reader intent is unclear"),
        };
        self.check()?;
        let fingerprint = digest(&serde_json::to_vec(&(
            self.token.scope(),
            self.token.store_generation(),
            self.token.aggregate_generation(),
            self.token.accepted_base_sha256(),
            self.token.selection_sha256(),
            &self.root,
            &self.user,
            &research.capture,
            &research.image_digests,
            &research.original_image_digest,
            &research.key,
            research.policy,
            directive,
            agreement.question_sha256(),
            agreement.readings_sha256(),
        ))?);
        research.intent.replace(Some(AdmittedIntent {
            agreement,
            directive,
            prepared: RequestBinding::capture(self),
            interpreted: None,
            fingerprint,
        }));
        research.pending.set(ResearchProgress::Verified);
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn research_intent(&self, interpreted: bool) -> Result<(String, ResearchDirective, String)> {
        let entry = self.research()?.intent.borrow();
        let intent = entry
            .as_ref()
            .context("Selected Reader intent not admitted")?;
        let binding = if interpreted {
            intent
                .interpreted
                .as_ref()
                .context("Selected Reader intent not freshly interpreted")?
        } else {
            anyhow::ensure!(
                intent.interpreted.is_none(),
                "Selected Reader intent already interpreted"
            );
            &intent.prepared
        };
        anyhow::ensure!(
            binding.matches(self),
            "Selected Reader intent custody changed"
        );
        Ok((
            intent.agreement.question().to_owned(),
            intent.directive,
            intent.fingerprint.clone(),
        ))
    }

    #[cfg(target_os = "linux")]
    pub(super) fn research_decision(&self) -> Result<(ResearchPolicy, ResearchDirective)> {
        let (_, directive, _) = self.research_intent(true)?;
        let research = self.research()?;
        anyhow::ensure!(
            research.pending.get() != ResearchProgress::Failed,
            "Selected Reader research refused"
        );
        Ok((research.policy, directive))
    }

    #[cfg(target_os = "linux")]
    pub(super) fn research_index_key(&self) -> Result<IndexKey> {
        self.research_intent(true)?;
        let research = self.research()?;
        anyhow::ensure!(
            matches!(
                research.pending.get(),
                ResearchProgress::Interpreted | ResearchProgress::ContextReady
            ),
            "Selected Reader retrieval is single-use"
        );
        Ok(research.key.clone())
    }

    #[cfg(target_os = "linux")]
    pub(super) fn research_context(&self) -> Result<ResearchContext> {
        let (question, _, _) = self.research_intent(true)?;
        let research = self.research()?;
        let policy = self.research_policy()?;
        anyhow::ensure!(
            research.pending.get() == ResearchProgress::Interpreted,
            "Selected Reader source context requires fresh Interpret"
        );
        research.pending.set(ResearchProgress::Failed);
        let context = self.with_resident(policy, ReaderPhase::Result, |store, reads| {
            let snapshot = store
                .selected_snapshot_with_budget(
                    self.admission.scope(),
                    crate::storage::MAX_ITEMS,
                    reads,
                )?
                .context("Selected Reader research selection absent")?
                .snapshot;
            anyhow::ensure!(
                snapshot.token == self.token,
                "Selected Reader research selection changed"
            );
            let projection = SelectedDomainProjection::from_snapshot(&snapshot)?;
            let user = self.project_user(&projection)?;
            anyhow::ensure!(
                user.outcome == Outcome::Interpreted
                    && user.text.as_deref() == Some(question.as_str())
                    && projection.current_reference(research.capture.namespace, self.capture)?
                        == research.capture,
                "Selected Reader research context changed"
            );
            Ok(ResearchContext {
                conversation: self.root.record_id,
                user: user.id,
                capture: self.capture,
                key: research.key.clone(),
                records: projection.inspect(user.conversation)?,
                media: snapshot.transaction.selected.media.clone(),
            })
        })?;
        research.pending.set(ResearchProgress::ContextReady);
        Ok(context)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn query_research_source(
        &self,
        expected: &IndexKey,
        question: &str,
        max_passages: usize,
        max_bytes: usize,
        neighbors: bool,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Retrieved> {
        let (admitted_question, _, _) = self.research_intent(true)?;
        let research = self.research()?;
        anyhow::ensure!(
            research.pending.get() == ResearchProgress::ContextReady
                && expected == &research.key
                && question == admitted_question,
            "Selected Reader retrieval input changed or was already consumed"
        );
        research.pending.set(ResearchProgress::Failed);
        let policy = self.research_policy()?;
        let deadline = policy.deadline();
        self.check()?;
        let index = research
            .index
            .borrow_mut()
            .take()
            .context("Selected Reader PDF index already consumed")?;
        let result = index.query(
            expected,
            question,
            max_passages,
            max_bytes,
            neighbors,
            deadline,
            &mut || {
                policy.require_result_time()?;
                progress()
            },
        )?;
        self.check()?;
        research.pending.set(ResearchProgress::Retrieved);
        Ok(result)
    }

    #[cfg(any(target_os = "linux", test))]
    pub(super) fn execute_model(
        &self,
        engine: &mut dyn crate::LLMEngine,
        progress: &mut dyn FnMut() -> Result<()>,
        memory: Option<(&dyn crate::memory::Provider, &crate::memory::QueryResult)>,
    ) -> Result<String> {
        #[cfg(target_os = "linux")]
        anyhow::ensure!(
            self.research.is_none(),
            "Selected Reader research requires named model stages"
        );
        let Some(policy) = &self.resident else {
            return engine.execute_with_progress(progress);
        };
        let stage = self.model_stage.get();
        anyhow::ensure!(stage < 3, "selected Reader model allowance consumed");
        let mut submitted = false;
        let result = engine.execute_with_deadline(
            progress,
            &mut |submit| {
                anyhow::ensure!(
                    self.model_stage.get() == stage,
                    "selected Reader handoff already consumed"
                );
                self.model_stage.set(stage + 1);
                let mut submit_once = || {
                    anyhow::ensure!(!submitted, "selected Reader model already submitted");
                    if stage == 0 {
                        policy.require_acquisition_time()?;
                    }
                    anyhow::ensure!(
                        std::time::Instant::now() < policy.deadline(),
                        "Reader request expired"
                    );
                    policy.start_model_wait()?;
                    submit()?;
                    submitted = true;
                    Ok(())
                };
                self.with_model_handoff(policy, memory, &mut submit_once)
            },
            policy.deadline(),
        );
        let value = result?;
        anyhow::ensure!(
            submitted,
            "selected Reader result has no guarded submission"
        );
        policy.finish_model_wait();
        Ok(value)
    }

    #[cfg(any(target_os = "linux", test))]
    fn with_model_handoff(
        &self,
        policy: &ReaderPreferences,
        memory: ReaderMemory<'_>,
        submit: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        let mut reads = policy.reads(ReaderPhase::Submission)?;
        let mut gates = policy.gates().to_vec();
        let handwriting = self.handwriting_owner()?;
        if let Some(owner) = handwriting {
            owner.join_gates(&mut gates);
        }
        let mut checkpoint = None;
        let mut verify = |store: &Store| {
            anyhow::ensure!(
                checkpoint.is_none(),
                "selected Reader proof already verified"
            );
            checkpoint = handwriting
                .map(|owner| owner.verify_locked(store, &mut reads))
                .transpose()?;
            policy.verify_locked(store, &self.admission, &self.token, &mut reads)
        };
        if let Some((provider, view)) = memory {
            provider.with_selected_reader_handoff(
                view,
                &self.admission,
                &gates,
                &mut verify,
                submit,
            )
        } else {
            gates.push(&self.admission);
            SelectedAdmission::with_gates(&gates, |store| {
                verify(store)?;
                submit()
            })
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn run_researched_stage<T>(
        &self,
        stage: ResearchStage,
        memory: ReaderMemory<'_>,
        current: &mut dyn FnMut() -> Result<()>,
        run: impl FnOnce(&mut crate::llm::ModelHandoff<'_>, Instant) -> Result<T>,
    ) -> Result<T> {
        let research = self.research()?;
        let policy = self.research_policy()?;
        let intent_fingerprint = match stage {
            ResearchStage::Analysis | ResearchStage::Verification => None,
            ResearchStage::Research | ResearchStage::Answer => Some(self.research_intent(true)?.2),
        };
        let after = match stage {
            ResearchStage::Analysis | ResearchStage::Verification => {
                let (before, after) = if stage == ResearchStage::Analysis {
                    (ResearchProgress::Prepared, ResearchProgress::Analyzed)
                } else {
                    (ResearchProgress::Analyzed, ResearchProgress::Verified)
                };
                anyhow::ensure!(
                    research.pending.get() == before
                        && research.images_checked.replace(false)
                        && memory.is_none(),
                    "Selected Reader recognition stage is out of order"
                );
                after
            }
            ResearchStage::Research | ResearchStage::Answer => {
                let (saved, directive) = self.research_decision()?;
                let required =
                    saved == ResearchPolicy::Required && directive == ResearchDirective::UsePolicy;
                let (before, after) = match (stage, required) {
                    (ResearchStage::Research, true) => {
                        (ResearchProgress::Retrieved, ResearchProgress::Researched)
                    }
                    (ResearchStage::Answer, true) => {
                        (ResearchProgress::Researched, ResearchProgress::Answered)
                    }
                    (ResearchStage::Answer, false) => {
                        (ResearchProgress::Retrieved, ResearchProgress::Answered)
                    }
                    _ => anyhow::bail!("Selected Reader external research is disabled"),
                };
                anyhow::ensure!(
                    research.pending.get() == before,
                    "Selected Reader provider stage is out of order"
                );
                research.check_memory(memory)?;
                after
            }
        };
        research.pending.set(ResearchProgress::Failed);
        let deadline = policy
            .deadline()
            .min(Instant::now() + Duration::from_secs(67));
        research.check_pin(deadline)?;
        let pinned = research.memory.borrow();
        let memory = memory
            .zip(pinned.as_ref())
            .map(|((provider, _), view)| (provider, view));
        let mut attempted = false;
        let mut submitted = false;
        let mut submit_attempted = false;
        let mut handoff_refused = false;
        let result = run(
            &mut |submit| {
                if attempted {
                    handoff_refused = true;
                    anyhow::bail!("Selected Reader provider handoff already consumed");
                }
                attempted = true;
                let outcome = self.with_model_handoff(policy, memory, &mut || {
                    if submit_attempted {
                        handoff_refused = true;
                        anyhow::bail!("Selected Reader provider submission already consumed");
                    }
                    submit_attempted = true;
                    anyhow::ensure!(
                        Instant::now() < deadline,
                        "Selected Reader provider stage expired"
                    );
                    if let Some(fingerprint) = &intent_fingerprint {
                        anyhow::ensure!(
                            &self.research_intent(true)?.2 == fingerprint,
                            "Selected Reader guarded intent changed"
                        );
                    }
                    if stage == ResearchStage::Analysis {
                        policy.require_acquisition_time()?;
                    }
                    research.check_pin(deadline)?;
                    current()?;
                    anyhow::ensure!(
                        Instant::now() < deadline,
                        "Selected Reader provider stage expired"
                    );
                    if stage == ResearchStage::Analysis {
                        policy.require_acquisition_time()?;
                    }
                    policy.start_model_wait()?;
                    submit()?;
                    submitted = true;
                    Ok(())
                });
                handoff_refused |= outcome.is_err();
                outcome
            },
            deadline,
        )?;
        anyhow::ensure!(
            submitted && !handoff_refused && Instant::now() < deadline,
            "Selected Reader provider result is unadmitted or late"
        );
        policy.finish_model_wait();
        self.check()?;
        research.pending.set(after);
        Ok(result)
    }
    pub(super) fn source_target_enabled(&self) -> bool {
        self.targets
    }

    #[cfg(target_os = "linux")]
    pub(super) fn require_store(&self, ledger: &Ledger) -> Result<()> {
        anyhow::ensure!(
            self.admission.uses_store(ledger.store()),
            "selected Attempt has foreign Store"
        );
        Ok(())
    }
    pub(super) fn write_target(
        &mut self,
        text: &str,
        candidate: &crate::conversation::target::Candidate,
    ) -> Result<SelectedReaderAcknowledgment> {
        anyhow::ensure!(self.targets, "selected target family absent");
        self.write_candidate(text, false, None, Some(candidate), None)
    }
    pub(super) fn write(
        &mut self,
        text: &str,
        generated: bool,
        memory: Option<(&dyn crate::memory::Provider, &crate::memory::QueryResult)>,
    ) -> Result<SelectedReaderAcknowledgment> {
        self.write_candidate(text, generated, memory, None, None)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn write_researched(
        &mut self,
        text: &str,
        generated: bool,
        memory: ReaderMemory<'_>,
        candidate: Option<&crate::conversation::target::Candidate>,
        current: &mut dyn FnMut() -> Result<()>,
    ) -> Result<SelectedReaderAcknowledgment> {
        anyhow::ensure!(self.researched(), "Selected Reader research is absent");
        anyhow::ensure!(
            candidate.is_none() || (!generated && self.targets),
            "selected target family absent"
        );
        self.write_candidate(text, generated, memory, candidate, Some(current))
    }
    fn write_candidate(
        &mut self,
        text: &str,
        generated: bool,
        memory: Option<(&dyn crate::memory::Provider, &crate::memory::QueryResult)>,
        candidate: Option<&crate::conversation::target::Candidate>,
        #[cfg_attr(not(target_os = "linux"), allow(unused_variables))] current: Option<
            &mut dyn FnMut() -> Result<()>,
        >,
    ) -> Result<SelectedReaderAcknowledgment> {
        #[cfg(target_os = "linux")]
        let mut current = current;
        #[cfg(target_os = "linux")]
        anyhow::ensure!(
            self.research.is_some() == current.is_some(),
            "Selected Reader research publication requires its currentness check"
        );
        #[cfg(target_os = "linux")]
        let research_intent = if let Some(research) = &self.research {
            let (question, _, fingerprint) = self.research_intent(generated)?;
            anyhow::ensure!(
                if generated {
                    research.pending.get() == ResearchProgress::Answered
                } else {
                    research.pending.get() == ResearchProgress::Verified && text == question
                },
                "Selected Reader research publication is out of order"
            );
            research.pending.set(ResearchProgress::Failed);
            if generated {
                research.check_memory(memory)?;
            }
            research.check_pin(self.research_policy()?.deadline())?;
            Some(fingerprint)
        } else {
            None
        };
        let slot = if generated {
            &mut self.generation
        } else {
            &mut self.interpretation
        };
        if let Some(frozen) = slot.as_ref() {
            #[cfg(target_os = "linux")]
            anyhow::ensure!(
                frozen.research_intent == research_intent,
                "Selected Reader original intent changed"
            );
            anyhow::ensure!(
                frozen.request.text == text
                    && match (&frozen.request.kind, candidate) {
                        (SelectedReaderText::InterpretTarget { candidate: prior }, Some(now)) =>
                            prior == now,
                        (SelectedReaderText::InterpretTarget { .. }, None) => false,
                        (_, Some(_)) => false,
                        (_, None) => true,
                    },
                "selected Attempt mutation changed"
            );
            if let Some(acknowledgment) = frozen.acknowledged.clone() {
                self.check()?;
                return Ok(acknowledgment);
            }
        } else {
            *slot = Some(Frozen {
                accepted: self.token.clone(),
                request: SelectedReaderWrite {
                    operation: Uuid::new_v4(),
                    root: self.root.clone(),
                    user: self.user.clone(),
                    updated_ms: now()?,
                    text: text.into(),
                    kind: if generated {
                        SelectedReaderText::Generate {
                            assistant: Uuid::new_v4(),
                        }
                    } else if let Some(candidate) = candidate {
                        SelectedReaderText::InterpretTarget {
                            candidate: candidate.clone(),
                        }
                    } else {
                        SelectedReaderText::Interpret
                    },
                },
                acknowledged: None,
                #[cfg(target_os = "linux")]
                research_intent: research_intent.clone(),
            });
        }
        let frozen = if generated {
            self.generation.as_ref()
        } else {
            self.interpretation.as_ref()
        }
        .unwrap();
        #[cfg(all(test, target_os = "linux", feature = "development-resident-bootstrap"))]
        if generated && self.researched() {
            crate::workflow::orchestrator::researched_fixture::record_generation(
                &frozen.accepted,
                &frozen.request,
            )?;
        }
        #[cfg(any(target_os = "linux", test))]
        let guarded = if let Some(policy) = &self.resident {
            let mut reads = policy.reads(ReaderPhase::Result)?;
            anyhow::ensure!(
                !self.admission.recover_reader_text(
                    &frozen.accepted,
                    &frozen.request,
                    &mut reads
                )?,
                "selected Reader text is already historical"
            );
            let verified = self.model_stage.get() >= 2;
            #[cfg(target_os = "linux")]
            let verified = verified || research_intent.is_some();
            anyhow::ensure!(verified, "selected Reader verification not submitted");
            let handwriting = self.handwriting_owner()?;
            let mut checkpoint = None;
            let mut verify =
                |store: &Store, reads: &mut crate::storage::selection::SelectedReadBudget| {
                    anyhow::ensure!(
                        checkpoint.is_none(),
                        "selected Reader proof already verified"
                    );
                    checkpoint = handwriting
                        .map(|owner| owner.verify_locked(store, reads))
                        .transpose()?;
                    policy.verify_locked(store, &self.admission, &frozen.accepted, reads)?;
                    #[cfg(target_os = "linux")]
                    if let Some(research) = &self.research {
                        let (_, _, fingerprint) = self.research_intent(generated)?;
                        anyhow::ensure!(
                            frozen.research_intent.as_ref() == Some(&fingerprint),
                            "Selected Reader original intent changed"
                        );
                        research.check_pin(policy.deadline())?;
                        let current = current
                            .as_deref_mut()
                            .context("Selected Reader research currentness check absent")?;
                        current()?;
                        anyhow::ensure!(
                            Instant::now() < policy.deadline(),
                            "Selected Reader research publication expired"
                        );
                    }
                    policy.require_result_time()
                };
            let mut gates = policy.gates().to_vec();
            if let Some(owner) = handwriting {
                owner.join_gates(&mut gates);
            }
            #[cfg(target_os = "linux")]
            let pinned = self
                .research
                .as_ref()
                .map(|research| research.memory.borrow());
            #[cfg(target_os = "linux")]
            let memory = if let Some(pinned) = &pinned {
                memory
                    .zip(pinned.as_ref())
                    .map(|((provider, _), view)| (provider, view))
            } else {
                memory
            };
            Some(match memory {
                Some((provider, view)) => provider.publish_selected_reader_text_joined(
                    view,
                    &self.admission,
                    &frozen.accepted,
                    frozen.request.clone(),
                    (&gates, &mut reads),
                    &mut verify,
                )?,
                None => self.admission.publish_resident_reader_text_joined(
                    &frozen.accepted,
                    frozen.request.clone(),
                    &gates,
                    &mut reads,
                    verify,
                )?,
            })
        } else {
            None
        };
        #[cfg(not(any(target_os = "linux", test)))]
        let guarded = None;
        let result = if let Some(result) = guarded {
            result
        } else {
            self.mode()?;
            match memory {
                Some((provider, view)) => provider.publish_selected_reader_text(
                    view,
                    &self.admission,
                    &frozen.accepted,
                    frozen.request.clone(),
                )?,
                None => self
                    .admission
                    .publish_reader_text(&frozen.accepted, frozen.request.clone())?,
            }
        };
        let SelectedReaderPublication::Published {
            publication,
            acknowledgment,
        } = result
        else {
            anyhow::bail!(
                "historical selected acknowledgment cannot install current Attempt state"
            );
        };
        #[cfg(all(test, target_os = "linux", feature = "development-resident-bootstrap"))]
        if generated && self.researched() {
            crate::workflow::orchestrator::researched_fixture::after_generated_ack()?;
        }
        self.token = publication.token;
        self.root = acknowledgment.root.clone();
        if !generated {
            self.user = acknowledgment.turn.clone();
        }
        let slot = if generated {
            &mut self.generation
        } else {
            &mut self.interpretation
        };
        slot.as_mut().unwrap().acknowledged = Some((*acknowledgment).clone());
        self.check()?;
        #[cfg(target_os = "linux")]
        if let Some(research) = &self.research {
            if generated {
                research.pending.set(ResearchProgress::Generated);
            } else {
                let mut intent = research.intent.borrow_mut();
                let intent = intent
                    .as_mut()
                    .context("Selected Reader intent absent at Interpret")?;
                anyhow::ensure!(
                    acknowledgment.written.outcome == Outcome::Interpreted
                        && acknowledgment.written.id == intent.prepared.user.record_id
                        && acknowledgment.root.record_id == intent.prepared.root.record_id
                        && acknowledgment.written.sources == [research.capture.record_id]
                        && acknowledgment.written.text.as_deref()
                            == Some(intent.agreement.question()),
                    "Selected Reader Interpret acknowledgment changed the request"
                );
                intent.interpreted = Some(RequestBinding::capture(self));
                research.pending.set(ResearchProgress::Interpreted);
            }
        }
        Ok(*acknowledgment)
    }
}

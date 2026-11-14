//! Live presentation ownership for one unified exchange. The SDK has no
//! native exact-range effect adapter for this controller.
use super::history::Action;
use crate::{
    conversation::{
        marked_question::{MarkedAnswerOutput, PairRevisions},
        CompletionEvidence, ContextBudget, HistoricalPageAssociationInspection, Ledger, Outcome,
        Role, Turn,
    },
    conversation_layout::{self, Geometry, PresentationIdentity},
    storage::{Namespace, Uuid},
};
use anyhow::{ensure, Context, Result};
use std::sync::{Arc, Weak};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresentedTurn {
    pub id: Uuid,
    pub revision: Uuid,
    pub text: String,
    pub about: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresentedExchange {
    pub id: Uuid,
    pub turns: Vec<PresentedTurn>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageOwner {
    pub document: Uuid,
    pub page: Uuid,
    pub visit: String,
    pub session: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    pub header: String,
    pub composer: String,
    pub status: String,
    pub unrelated: String,
    pub exchanges: Vec<PresentedExchange>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageState {
    pub owner: PageOwner,
    pub content: Content,
    pub seal: String,
}

pub struct PresentationReceipt {
    pub operation: Uuid,
    pub page: PageState,
    pub root_revision: Uuid,
    pub binding_revision: Uuid,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct UserPresentationTarget {
    pub conversation: Uuid,
    pub document: Uuid,
    pub page: Uuid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditAction {
    Undo,
    Redo,
    PresentUser,
    PresentAnswer,
}

pub struct Edit {
    pub operation: Uuid,
    pub action: EditAction,
    pub before: PageState,
    pub expected: Content,
}

pub enum Effect {
    Verified {
        operation: Uuid,
        page: Box<PageState>,
    },
    VerifiedPair {
        page: Box<PageState>,
        evidence: Box<CompletionEvidence>,
    },
    Refused,
    Unknown,
}

/// A backend must consume its live presentation receipt and report exact
/// observations. Stored page or turn IDs alone are insufficient.
pub trait PageBackend {
    /// Prior completed-exchange receipt for history admission.
    fn take_presentation(&mut self) -> Option<PresentationReceipt>;
    fn observe(&mut self) -> Result<PageState>;
    /// Compare the complete before-state and active owner at the effect boundary.
    /// Refuse actions the owner does not support before applying any mutation.
    /// Pair acceptance is synchronous and cannot call Store or wait for readback.
    fn apply(&mut self, edit: &Edit) -> Effect;
    /// Associate a verified modeled page state with the final durable revision.
    /// Synchronous metadata only. Implementations cannot re-enter Store.
    fn bind_completed_answer(
        &mut self,
        _operation: Uuid,
        _verified: &PageState,
        _draft: Uuid,
        _completed: Uuid,
    ) -> bool {
        false
    }
}

pub(crate) struct VerifiedPairReceipt {
    target: UserPresentationTarget,
    revisions: PairRevisions,
    page: Box<PageState>,
    evidence: Box<CompletionEvidence>,
}

impl VerifiedPairReceipt {
    pub(super) fn page(&self) -> &PageState {
        &self.page
    }

    pub(super) fn operation(&self) -> Uuid {
        self.evidence.native_operation
    }

    pub(crate) fn consume(
        self,
        target: UserPresentationTarget,
        revisions: PairRevisions,
        draft: &Turn,
    ) -> Result<CompletionEvidence> {
        ensure!(
            self.target == target
                && self.revisions == revisions
                && self.evidence.native_operation
                    == crate::writer::stage(draft.exchange, "reader-present-marked-answer")
                && draft.text.as_deref() == Some(self.evidence.exact_text.as_str()),
            "verified pair does not belong to the captured answer"
        );
        Ok(*self.evidence)
    }
}

pub(super) enum PairPresentation {
    Verified(VerifiedPairReceipt),
    Refused,
    Uncertain,
}

pub(crate) const MODELED_PAIR_PROCEDURE: &str = "modeled-pair-edit-readback";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeStatus {
    Applied,
    NoChange,
    Refused,
    ReconcileRequired,
}

#[derive(PartialEq, Eq)]
pub(crate) struct Projection {
    pub(crate) root: Uuid,
    pub(crate) binding: Uuid,
    pub(crate) exchanges: Vec<PresentedExchange>,
    latest_complete: bool,
}

struct OwnedExchange {
    store: Weak<crate::storage::Store>,
    conversation: Uuid,
    observed: PageState,
    projection: Projection,
    exchange: PresentedExchange,
    applied: bool,
}

#[derive(Default)]
pub struct History {
    owned: Option<OwnedExchange>,
}

fn current_turn(ledger: &Ledger, turn: &Turn) -> Result<PresentedTurn> {
    let envelope = ledger
        .store()
        .value(Namespace::Conversation, turn.id)?
        .context("presentation turn is unavailable")?;
    ensure!(
        matches!(Ledger::decode(&envelope)?, crate::conversation::Record::Turn(current) if current == *turn),
        "presentation turn changed"
    );
    Ok(PresentedTurn {
        about: None,
        id: turn.id,
        revision: envelope.revision_id,
        text: turn.text.clone().context("presentation text unavailable")?,
    })
}

pub(crate) fn project(
    ledger: &Ledger,
    conversation: Uuid,
    owner: &PageOwner,
    composer: &str,
) -> Result<Projection> {
    project_for(ledger, conversation, owner, composer, None)
}

fn project_for(
    ledger: &Ledger,
    conversation: Uuid,
    owner: &PageOwner,
    composer: &str,
    output: Option<&MarkedAnswerOutput>,
) -> Result<Projection> {
    let inspect = || ledger.inspect_page_association(conversation, owner.document, owner.page);
    let HistoricalPageAssociationInspection::HistoricalCandidate(before) = inspect()? else {
        anyhow::bail!("presentation binding unavailable")
    };
    let budget = ContextBudget {
        max_text_bytes: 64 * 1024,
        max_turns: 128,
        provider_token_limit: Some(64 * 1024),
    };
    let count = |turns: &[Turn]| {
        Ok(turns
            .iter()
            .map(|turn| turn.text.as_ref().map_or(0, String::len))
            .sum())
    };
    let context = if let Some(output) = output {
        output.presentation_context(ledger, &budget, count)?
    } else {
        ledger.presentation_context(conversation, &budget, count)?
    };
    ensure!(
        context.missing_media.is_empty(),
        "presentation source evidence unavailable"
    );
    let page = conversation_layout::plan(
        PresentationIdentity::Persisted {
            conversation,
            document: owner.document,
            page: owner.page,
        },
        &context,
        composer,
        Geometry::default(),
    )?;
    let exchanges = page
        .exchanges
        .iter()
        .map(|exchange| {
            let turns = exchange
                .turns
                .iter()
                .map(|turn| {
                    let mut presented = current_turn(ledger, turn)?;
                    presented.about = page
                        .about
                        .iter()
                        .find(|about| about.turn == turn.id)
                        .map(|about| about.text.to_owned());
                    Ok(presented)
                })
                .collect::<Result<_>>()?;
            Ok(PresentedExchange {
                id: exchange.id,
                turns,
            })
        })
        .collect::<Result<_>>()?;
    let latest_complete = page.exchanges.first().is_some_and(|exchange| {
        matches!(exchange.turns.as_slice(), [request, response]
            if request.role == Role::User && request.outcome == Outcome::Interpreted
                && response.role == Role::Assistant && response.outcome == Outcome::Completed)
    });
    let HistoricalPageAssociationInspection::HistoricalCandidate(after) = inspect()? else {
        anyhow::bail!("presentation binding became unavailable")
    };
    ensure!(before == after, "presentation binding or root changed");
    Ok(Projection {
        root: before.root_revision(),
        binding: before.binding_revision(),
        exchanges,
        latest_complete,
    })
}

fn verify_edit(
    backend: &mut dyn PageBackend,
    edit: &Edit,
) -> Result<Option<PageState>, OutcomeStatus> {
    let effect = backend.apply(edit);
    verify_edit_result(backend, edit, effect).map(|result| result.map(|(page, _)| *page))
}

type VerifiedEdit = (Box<PageState>, Option<Box<CompletionEvidence>>);

fn verify_edit_result(
    backend: &mut dyn PageBackend,
    edit: &Edit,
    effect: Effect,
) -> Result<Option<VerifiedEdit>, OutcomeStatus> {
    let verified = match effect {
        Effect::Refused
            if backend
                .observe()
                .is_ok_and(|current| current == edit.before) =>
        {
            return Ok(None);
        }
        Effect::Verified { operation, page }
            if operation == edit.operation && edit.action != EditAction::PresentAnswer =>
        {
            (page, None)
        }
        Effect::VerifiedPair { page, evidence }
            if evidence.native_operation == edit.operation
                && edit.action == EditAction::PresentAnswer =>
        {
            (page, Some(evidence))
        }
        _ => return Err(OutcomeStatus::ReconcileRequired),
    };
    let page = &verified.0;
    if page.owner != edit.before.owner
        || page.content != edit.expected
        || page.seal.is_empty()
        || page.seal == edit.before.seal
        || !backend.observe().is_ok_and(|current| current == **page)
    {
        return Err(OutcomeStatus::ReconcileRequired);
    }
    Ok(Some(verified))
}

pub(super) fn present_answer(
    ledger: &Ledger,
    backend: &mut dyn PageBackend,
    output: &mut MarkedAnswerOutput,
    user: &Turn,
) -> Result<PairPresentation> {
    let before = backend.observe()?;
    let target = output.target();
    let revisions = output.revisions();
    ensure!(
        before.owner.document == target.document
            && before.owner.page == target.page
            && !before.owner.visit.is_empty()
            && !before.owner.session.is_empty()
            && !before.seal.is_empty(),
        "marked answer destination owner differs"
    );
    let projection = project_for(
        ledger,
        target.conversation,
        &before.owner,
        &before.content.composer,
        Some(output),
    )?;
    let (first, earlier) = projection
        .exchanges
        .split_first()
        .context("marked answer exchange absent")?;
    let [question, answer] = first.turns.as_slice() else {
        anyhow::bail!("marked answer exchange does not contain the exact pair");
    };
    let draft = output.draft();
    ensure!(
        projection.root == revisions.root
            && projection.binding == revisions.binding
            && first.id == user.exchange
            && question.id == user.id
            && question.revision == revisions.user
            && user.text.as_deref() == Some(question.text.as_str())
            && answer.id == draft.id
            && answer.revision == revisions.draft
            && draft.text.as_deref() == Some(answer.text.as_str())
            && earlier == before.content.exchanges.as_slice(),
        "marked answer pair or earlier presentation changed"
    );
    let mut expected = before.content.clone();
    expected.exchanges.insert(0, first.clone());
    let edit = Edit {
        operation: crate::writer::stage(user.exchange, "reader-present-marked-answer"),
        action: EditAction::PresentAnswer,
        before,
        expected,
    };
    let effect = output.accept_pair(ledger, || backend.apply(&edit))?;
    let draft = output.draft();
    // Readback and all Store-facing validation run after synchronous acceptance.
    Ok(match verify_edit_result(backend, &edit, effect) {
        Ok(None) => PairPresentation::Refused,
        Ok(Some((page, Some(evidence))))
            if draft.text.as_deref() == Some(evidence.exact_text.as_str())
                && evidence.observation.document == page.owner.document
                && evidence.observation.page == page.owner.page
                && evidence.observation.session == page.owner.session
                && evidence.observation.visit == page.owner.visit
                && evidence.observation.content_revision.as_deref() == Some(page.seal.as_str())
                && evidence.procedure == MODELED_PAIR_PROCEDURE
                && !evidence.observation.evidence_procedure.is_empty()
                && !evidence.observation.capability_revision.is_empty() =>
        {
            PairPresentation::Verified(VerifiedPairReceipt {
                target,
                revisions: output.revisions(),
                page,
                evidence,
            })
        }
        _ => PairPresentation::Uncertain,
    })
}

pub(super) fn present_user(
    ledger: &Ledger,
    backend: &mut dyn PageBackend,
    target: UserPresentationTarget,
    user: &Turn,
    head: (Uuid, Uuid),
    binding_revision: Uuid,
) -> Result<OutcomeStatus> {
    let before = backend.observe()?;
    ensure!(
        user.conversation == target.conversation
            && before.owner.document == target.document
            && before.owner.page == target.page
            && !before.owner.session.is_empty()
            && !before.owner.visit.is_empty()
            && !before.seal.is_empty(),
        "user-only presentation owner differs"
    );
    let current_projection = || {
        project(
            ledger,
            target.conversation,
            &before.owner,
            &before.content.composer,
        )
    };
    let projection = current_projection()?;
    let (first, earlier) = projection
        .exchanges
        .split_first()
        .context("saved exchange absent")?;
    let [turn] = first.turns.as_slice() else {
        anyhow::bail!("saved exchange is not user-only");
    };
    ensure!(
        projection.root == head.0
            && projection.binding == binding_revision
            && first.id == user.exchange
            && turn.id == user.id
            && turn.revision == head.1
            && user.text.as_deref() == Some(turn.text.as_str())
            && earlier == before.content.exchanges.as_slice(),
        "saved user or earlier presentation changed"
    );
    let mut expected = before.content.clone();
    expected.exchanges.insert(0, first.clone());
    let edit = Edit {
        operation: crate::writer::stage(user.exchange, "reader-present-marked-question"),
        action: EditAction::PresentUser,
        before: before.clone(),
        expected,
    };
    Ok(match verify_edit(backend, &edit) {
        Ok(Some(_)) if current_projection().is_ok_and(|view| view == projection) => {
            OutcomeStatus::Applied
        }
        Ok(None) => OutcomeStatus::Refused,
        _ => OutcomeStatus::ReconcileRequired,
    })
}

impl History {
    pub fn handle(
        &mut self,
        event: crate::device::interaction::Interaction,
        ledger: &Ledger,
        backend: &mut impl PageBackend,
    ) -> OutcomeStatus {
        use crate::device::interaction::Interaction;
        match event {
            Interaction::Undo => self.perform(Action::Undo, ledger, backend),
            Interaction::Redo => self.perform(Action::Redo, ledger, backend),
            Interaction::Invalidated | Interaction::Reader | Interaction::BlankStart => {
                self.discard();
                OutcomeStatus::Refused
            }
        }
    }

    pub fn discard(&mut self) {
        self.owned = None;
    }

    pub fn arm(
        &mut self,
        ledger: &Ledger,
        conversation: Uuid,
        backend: &mut impl PageBackend,
    ) -> Result<()> {
        self.discard();
        let receipt = backend
            .take_presentation()
            .context("live presentation receipt unavailable")?;
        let observed = backend.observe()?;
        ensure!(
            !receipt.operation.is_nil()
                && receipt.page == observed
                && !observed.seal.is_empty()
                && !observed.owner.session.is_empty()
                && !observed.owner.visit.is_empty(),
            "live presentation changed"
        );
        let projection = project(
            ledger,
            conversation,
            &observed.owner,
            &observed.content.composer,
        )?;
        ensure!(
            projection.root == receipt.root_revision
                && projection.binding == receipt.binding_revision,
            "presentation receipt is stale"
        );
        ensure!(
            projection.exchanges == observed.content.exchanges,
            "page projection changed"
        );
        ensure!(
            projection.latest_complete,
            "latest exchange is not one complete pair"
        );
        let exchange = projection
            .exchanges
            .first()
            .context("no exchange to own")?
            .clone();
        self.owned = Some(OwnedExchange {
            store: Arc::downgrade(ledger.store()),
            conversation,
            observed,
            projection,
            exchange,
            applied: true,
        });
        Ok(())
    }

    fn perform(
        &mut self,
        action: Action,
        ledger: &Ledger,
        backend: &mut impl PageBackend,
    ) -> OutcomeStatus {
        let Some(mut owned) = self.owned.take() else {
            return OutcomeStatus::Refused;
        };
        if !owned
            .store
            .upgrade()
            .is_some_and(|store| Arc::ptr_eq(&store, ledger.store()))
        {
            return OutcomeStatus::Refused;
        }
        let current_projection = |page: &PageState| {
            project(
                ledger,
                owned.conversation,
                &page.owner,
                &page.content.composer,
            )
        };
        if !backend.observe().is_ok_and(|page| page == owned.observed)
            || !current_projection(&owned.observed).is_ok_and(|view| view == owned.projection)
        {
            return OutcomeStatus::Refused;
        }
        if owned.applied != (action == Action::Undo) {
            self.owned = Some(owned);
            return OutcomeStatus::NoChange;
        }
        let mut expected = owned.observed.content.clone();
        match action {
            Action::Undo => {
                expected.exchanges.remove(0);
            }
            Action::Redo => expected.exchanges.insert(0, owned.exchange.clone()),
        }
        let edit = Edit {
            operation: Uuid::new_v4(),
            action: match action {
                Action::Undo => EditAction::Undo,
                Action::Redo => EditAction::Redo,
            },
            before: owned.observed.clone(),
            expected,
        };
        match verify_edit(backend, &edit) {
            Ok(None) => OutcomeStatus::Refused,
            Ok(Some(page))
                if current_projection(&page).is_ok_and(|view| view == owned.projection) =>
            {
                owned.observed = page;
                owned.applied = action == Action::Redo;
                self.owned = Some(owned);
                OutcomeStatus::Applied
            }
            _ => OutcomeStatus::ReconcileRequired,
        }
    }
}

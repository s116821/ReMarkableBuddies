//! Live presentation ownership for one unified exchange. The SDK has no
//! native exact-range effect adapter for this controller.
use super::history::Action;
use crate::{
    conversation::{
        ContextBudget, HistoricalPageAssociationInspection, Ledger, Outcome, Role, Turn,
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

pub struct Edit {
    pub operation: Uuid,
    pub action: Action,
    pub before: PageState,
    pub expected: Content,
}

pub enum Effect {
    Verified {
        operation: Uuid,
        page: Box<PageState>,
    },
    Refused,
    Unknown,
}

/// A backend must consume its live presentation receipt and report exact
/// observations. Stored page or turn IDs alone are insufficient.
pub trait PageBackend {
    fn take_presentation(&mut self) -> Option<PresentationReceipt>;
    fn observe(&mut self) -> Result<PageState>;
    /// Compare the complete before-state at the effect boundary.
    fn apply(&mut self, edit: &Edit) -> Effect;
}

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
    let inspect = || ledger.inspect_page_association(conversation, owner.document, owner.page);
    let HistoricalPageAssociationInspection::HistoricalCandidate(before) = inspect()? else {
        anyhow::bail!("presentation binding unavailable")
    };
    let context = ledger.context(
        conversation,
        &ContextBudget {
            max_text_bytes: 64 * 1024,
            max_turns: 128,
            provider_token_limit: Some(64 * 1024),
        },
        |turns| {
            Ok(turns
                .iter()
                .map(|turn| turn.text.as_ref().map_or(0, String::len))
                .sum())
        },
    )?;
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
            action,
            before: owned.observed.clone(),
            expected,
        };
        match backend.apply(&edit) {
            Effect::Refused => {
                if backend
                    .observe()
                    .is_ok_and(|current| current == edit.before)
                {
                    OutcomeStatus::Refused
                } else {
                    OutcomeStatus::ReconcileRequired
                }
            }
            Effect::Unknown => OutcomeStatus::ReconcileRequired,
            Effect::Verified { operation, page } => {
                if operation != edit.operation
                    || page.owner != edit.before.owner
                    || page.content != edit.expected
                    || page.seal.is_empty()
                    || page.seal == edit.before.seal
                    || !backend.observe().is_ok_and(|current| current == *page)
                    || !current_projection(&page).is_ok_and(|view| view == owned.projection)
                {
                    return OutcomeStatus::ReconcileRequired;
                }
                owned.observed = *page;
                owned.applied = action == Action::Redo;
                self.owned = Some(owned);
                OutcomeStatus::Applied
            }
        }
    }
}

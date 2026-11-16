//! Development composer admission. All page facts remain historical; the native
//! owner must independently check its live visit and compare before any edit.
use super::{attempts, HistoricalAttempt};
use crate::{
    conversation::{ExpectedHeads, Ledger, Record, Role, Turn},
    storage::{Namespace, Store, Uuid},
};
use anyhow::{ensure, Result};
use remarkable_open_sdk::{
    native_adapter::{
        composer::{Command, Draft, Outcome, RangeIdentity, Reply as ComposerReply},
        Frame, Message, ReaderFacts, Rejection, Reply,
    },
    OperationId,
};
use std::sync::Arc;

pub(super) fn validate_reply(command: &Command, reply: &Reply) -> Result<()> {
    let valid = match (command, reply) {
        (
            _,
            Reply::Rejected(
                Rejection::Unsupported | Rejection::UnknownIdentity | Rejection::Capacity,
            ),
        ) => true,
        (Command::Provision(expected), Reply::Composer(ComposerReply::Provisioned(result))) => {
            match result {
                Outcome::Verified { value, .. } => value.identity.facts == *expected,
                Outcome::Refused { .. } | Outcome::Unknown { .. } => true,
            }
        }
        (Command::Clear(expected), Reply::Composer(ComposerReply::Cleared(result))) => match result
        {
            Outcome::Verified { value, .. } => {
                value.identity == expected.state.identity
                    && value.before_revision == expected.state.revision
            }
            Outcome::Refused { .. } | Outcome::Unknown { .. } => true,
        },
        _ => false,
    };
    ensure!(valid, "native composer reply mismatch");
    Ok(())
}

pub(super) fn refused(reply: Option<&Reply>) -> bool {
    matches!(
        reply,
        Some(Reply::Composer(
            ComposerReply::Provisioned(Outcome::Refused { .. })
                | ComposerReply::Cleared(Outcome::Refused { .. })
        ))
    )
}

pub(super) fn permits_followup(prior: &HistoricalAttempt, next: &Frame) -> Option<bool> {
    use remarkable_open_sdk::native_adapter::Command as NativeCommand;
    if matches!(
        prior.reply.as_ref(),
        Some(Reply::Composer(
            ComposerReply::Provisioned(Outcome::Verified { .. })
                | ComposerReply::Cleared(Outcome::Verified { .. })
        ))
    ) {
        // A settled composer effect is not a permanent page-navigation lock.
        // Each fresh command retains its own native/product admission, while
        // reserve still rejects every reused operation ID and unresolved prior.
        return Some(
            matches!(&next.message, Message::Request(command) if super::journal_effect(command)),
        );
    }
    let Message::Request(NativeCommand::Composer(next)) = &next.message else {
        return None;
    };
    let page = match next {
        Command::Provision(facts) => facts.page(),
        Command::Clear(draft) => draft.state.identity.facts.page(),
        _ => return Some(false),
    };
    Some(match prior.reply.as_ref() {
        Some(Reply::InsertionObserved { target, .. }) => *target == page,
        Some(Reply::TargetObserved(facts)) => facts.page() == page,
        _ => false,
    })
}

/// Retained native capability correspondence only. The endpoint still rechecks
/// the current runtime/visit; saved IDs never authorize a new native visit.
pub(super) fn require_page(
    store: &Arc<Store>,
    conversation: Uuid,
    facts: &ReaderFacts,
) -> Result<()> {
    let page = facts.page();
    let document = Uuid::from_bytes(*page.document.as_bytes());
    let page = Uuid::from_bytes(*page.page.as_bytes());
    let binding = Ledger::binding_id(document, page);
    let records = Ledger::new(store.clone()).inspect(conversation, false)?;
    ensure!(
        records.iter().any(|record| matches!(record,
        Record::Root(root) if root.id == conversation && root.binding == Some(binding))),
        "conversation has no matching page binding"
    );
    ensure!(
        records.iter().any(|record| matches!(record,
        Record::Binding(bound) if bound.id == binding && bound.conversation == conversation
            && bound.receipt.observed_document == document && bound.receipt.observed_page == page)),
        "composer page binding absent"
    );
    Ok(())
}

pub(super) fn require_range(
    store: &Arc<Store>,
    conversation: Uuid,
    identity: &RangeIdentity,
) -> Result<()> {
    require_page(store, conversation, &identity.facts)?;
    ensure!(attempts(store)?.iter().any(|attempt| {
        attempt.conversation == conversation && matches!(&attempt.reply,
            Some(Reply::Composer(ComposerReply::Provisioned(Outcome::Verified { value, .. }))) if value.identity == *identity)
    }), "composer range has no retained provision result");
    Ok(())
}

/// Proof that the exact draft is already a User turn in this Store. It grants no
/// native authority. Clear uses one deterministic operation per submitted turn.
pub struct RetainedUser {
    store: Arc<Store>,
    conversation: Uuid,
    turn: Uuid,
    revision: Uuid,
    operation: OperationId,
    draft: Draft,
}

impl RetainedUser {
    /// Append this submission before issuing its clear proof. An existing turn,
    /// even with identical text, cannot stand in for a new user interaction.
    pub fn append_user(
        ledger: &Ledger,
        expected_root: ExpectedHeads,
        user: Turn,
        draft: Draft,
    ) -> Result<Self> {
        let text = draft.text()?;
        ensure!(
            !text.trim().is_empty() && !text.contains('\0'),
            "invalid submitted composer text"
        );
        ensure!(
            user.role == Role::User && user.text.as_deref() == Some(text.as_str()),
            "submitted composer text differs"
        );
        ensure!(
            user.id == crate::writer::stage(user.exchange, "composer-user"),
            "composer submission identity differs"
        );
        ensure!(
            ledger
                .expected(Namespace::Conversation, user.id)?
                .0
                .is_empty(),
            "composer submission already retained"
        );
        require_range(ledger.store(), user.conversation, &draft.state.identity)?;
        let conversation = user.conversation;
        let turn = user.id;
        ledger.append_fresh(user.exchange, expected_root, user, vec![])?;
        Self::from_new_turn(ledger, conversation, turn, draft)
    }

    fn from_new_turn(
        ledger: &Ledger,
        conversation: Uuid,
        turn: Uuid,
        draft: Draft,
    ) -> Result<Self> {
        let text = draft.text()?;
        let user = ledger
            .inspect(conversation, false)?
            .into_iter()
            .find_map(|record| match record {
                Record::Turn(user) if user.id == turn => Some(user),
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("submitted composer turn absent"))?;
        ensure!(
            user.role == Role::User && user.text.as_deref() == Some(text.as_str()),
            "submitted composer text differs"
        );
        let envelope = ledger
            .store()
            .value(Namespace::Conversation, turn)?
            .ok_or_else(|| anyhow::anyhow!("submitted composer turn absent"))?;
        ensure!(
            matches!(Ledger::decode(&envelope)?, Record::Turn(ref current) if *current == user),
            "submitted composer turn changed"
        );
        let operation = crate::writer::stage(turn, "native-composer-clear");
        let proof = Self {
            store: ledger.store().clone(),
            conversation,
            turn,
            revision: envelope.revision_id,
            operation: OperationId(
                remarkable_open_sdk::Uuid::parse(&operation.to_string()).expect("canonical UUID"),
            ),
            draft,
        };
        proof.check(ledger.store(), proof.operation)?;
        Ok(proof)
    }

    pub fn operation(&self) -> OperationId {
        self.operation
    }
    pub fn conversation(&self) -> Uuid {
        self.conversation
    }
    pub fn draft(&self) -> &Draft {
        &self.draft
    }

    pub(super) fn check(&self, store: &Arc<Store>, operation: OperationId) -> Result<()> {
        ensure!(
            Arc::ptr_eq(store, &self.store) && operation == self.operation,
            "composer submission owner changed"
        );
        let ledger = Ledger::new(store.clone());
        let text = self.draft.text()?;
        let envelope = store
            .value(Namespace::Conversation, self.turn)?
            .ok_or_else(|| anyhow::anyhow!("submitted composer turn absent"))?;
        ensure!(
            envelope.revision_id == self.revision,
            "submitted composer turn changed"
        );
        ensure!(ledger.inspect(self.conversation, false)?.into_iter().any(|record| matches!(record,
            Record::Turn(user) if user.id == self.turn && user.role == Role::User && user.text.as_deref() == Some(text.as_str()))), "submitted composer turn changed");
        ensure!(
            store
                .value(Namespace::Conversation, self.turn)?
                .is_some_and(|current| current.revision_id == self.revision),
            "submitted composer turn changed"
        );
        require_range(store, self.conversation, &self.draft.state.identity)
    }
}

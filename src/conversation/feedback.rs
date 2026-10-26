//! Explicit historical metadata receiving; no pending queue or rendering authority.
use super::{Ledger, Record};
use crate::{
    conversation_feedback::{describe, FeedbackDescription},
    storage::{Kind, Namespace, Uuid, MAX_ITEMS},
};
use anyhow::{anyhow, bail, ensure, Context, Result};
use std::collections::BTreeSet;

pub const MAX_DESCRIPTION_BYTES: usize = 1024;

/// Resolve one original stored outcome under ordinary custody. No source images
/// are read. This explicit library call inherits the Store's full-index filter
/// traversal; its result bounds are not a global work/latency guarantee.
/// An older fact remains historical even if its turn later changes outcome.
pub fn describe_stored(
    ledger: &Ledger,
    conversation: Uuid,
    fact: Uuid,
) -> Result<FeedbackDescription> {
    ensure!(
        !conversation.is_nil() && !fact.is_nil(),
        "stored feedback identity unavailable"
    );
    let store = ledger.store();
    let current = store
        .sync_mode_snapshot()
        .map_err(|_| anyhow!("stored feedback mode unavailable"))?;
    let _ordinary = store
        .ordinary_mode_lease(&current.token)
        .map_err(|_| anyhow!("stored feedback ordinary custody unavailable"))?;
    ledger
        .complete_store()
        .map_err(|_| anyhow!("stored feedback store incomplete"))?;
    let identity = conversation.to_string();
    let snapshot = store
        .snapshot_heads_matching(&[Namespace::Conversation], MAX_ITEMS, |e| {
            let payload = &e.payload["record"];
            e.record_id == conversation
                || e.record_id == fact
                || payload["conversation"].as_str() == Some(&identity)
                || payload["acknowledgment"]["conversation"].as_str() == Some(&identity)
        })
        .map_err(|_| anyhow!("stored feedback snapshot unavailable"))?;

    // inspect_snapshot skips tombstones and trusts the snapshot selection. Check
    // every selected head first, including malformed/foreign explicit IDs and
    // siblings retained because another head matched the predicate.
    let mut seen = BTreeSet::new();
    for envelope in &snapshot {
        ensure!(
            envelope.kind == Kind::Value && envelope.domain_schema_version == 1,
            "unsupported stored feedback history"
        );
        ensure!(
            seen.insert((envelope.namespace, envelope.record_id)),
            "conflicted stored feedback history"
        );
        let owner = match Ledger::decode(envelope)
            .map_err(|_| anyhow!("invalid stored feedback record"))?
        {
            Record::Root(root) => root.id,
            Record::Turn(turn) => turn.conversation,
            Record::OutcomeFact(fact) => fact.conversation,
            Record::RequestEvidence(evidence) => evidence.conversation,
            Record::Binding(binding) => binding.conversation,
            Record::Receipt(receipt) => receipt.acknowledgment.conversation,
            _ => bail!("unsupported stored feedback record"),
        };
        ensure!(owner == conversation, "foreign stored feedback record");
    }
    let records = Ledger::inspect_snapshot(conversation, false, &snapshot)
        .map_err(|_| anyhow!("invalid stored feedback chronology"))?;
    let selected = records
        .iter()
        .find_map(|record| match record {
            Record::OutcomeFact(value) if value.id == fact => Some(value),
            _ => None,
        })
        .context("stored feedback fact unavailable")?;
    ensure!(
        records.iter().any(|record| matches!(record,
            Record::Turn(turn) if turn.id == selected.turn && turn.conversation == conversation)),
        "stored feedback turn unavailable"
    );
    let description = describe(selected)?.context("unsupported stored feedback outcome")?;
    ensure!(
        serde_json::to_vec(&description)?.len() <= MAX_DESCRIPTION_BYTES,
        "stored feedback description exceeds bound"
    );
    Ok(description)
}

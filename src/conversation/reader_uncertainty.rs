//! Locked reads only; never reacquire the canonical admission gate here.
use super::*;
use crate::storage::selection::SelectedSnapshot;

pub(super) fn validate(
    store: &Store,
    snapshot: &SelectedSnapshot,
    request: &PendingIntentRequest,
    own: Option<&HistoricalIntent>,
) -> Result<()> {
    let active = SelectedDomainProjection::from_snapshot(snapshot)?;
    ensure!(
        active.document_ownership()?.get(&request.conversation)
            == Some(&DocumentOwnership::Document(request.source.document)),
        "Reader current document ownership unavailable"
    );
    if let Some(original) = own {
        ensure!(
            original.receipt.acknowledgment.operation == request.operation,
            "Reader own pending operation differs"
        );
        for reference in &original.transaction.selected.records {
            ensure!(
                snapshot.transaction.selected.records.contains(reference),
                "Reader own original closure is no longer selected"
            );
        }
        ensure!(
            matches!(active.exact_reference(&original.receipt_reference)?, Record::Receipt(r) if r == &original.receipt),
            "Reader own receipt changed"
        );
        let intent = original
            .receipt
            .admitted_intent
            .as_ref()
            .context("Reader original intent absent")?;
        ensure!(
            intent.operation == request.operation
                && intent.conversation == request.conversation
                && intent.turn == request.turn
                && intent.selection == request.selection
                && intent.source == request.source
                && intent.evidence == request.evidence
                && intent.media == request.media,
            "Reader request differs from original admitted intent"
        );
        ensure!(
            matches!(active.exact_reference(&intent.root_revision)?, Record::Root(root)
            if root.id == request.conversation && root.updated_ms == request.updated_ms),
            "Reader original root chronology differs"
        );
        ensure!(
            matches!(active.exact_reference(&intent.turn_revision)?, Record::Turn(turn)
            if turn.id == request.turn && turn.conversation == request.conversation
                && turn.updated_ms == request.updated_ms && turn.outcome == Outcome::ReconcileRequired),
            "Reader original turn chronology or state differs"
        );
        for reference in &intent.evidence {
            active.exact_reference(reference)?;
        }
        ensure!(
            matches!(active.head(Namespace::Conversation, pending::pending_id(request.operation)),
            Some((_, Some(Record::OutcomeFact(f)))) if f.conversation == request.conversation
                && f.turn == request.turn && f.reason == AttemptReason::OutputPending
                && f.outcome == Outcome::ReconcileRequired && f.settlement.is_none()),
            "Reader own pending fact changed"
        );
    }
    let retained_refs: Vec<_> = snapshot
        .transaction
        .retained
        .iter()
        .flat_map(|m| &m.records)
        .collect();
    ensure!(
        retained_refs.len() == snapshot.retained_records.len(),
        "Reader retained reference count differs"
    );
    let mut records = snapshot.selected_records.clone();
    let mut references = snapshot.transaction.selected.records.clone();
    for (record, reference) in snapshot.retained_records.iter().zip(retained_refs) {
        if !references.contains(reference) {
            references.push(reference.clone());
            records.push(record.clone());
        }
    }
    let combined = SelectedDomainProjection::from_pinned_records(&records, &references)?;
    let owners = combined.document_ownership()?;
    let mut pending_operations = BTreeMap::new();
    let mut pending_facts = BTreeSet::new();
    for envelope in &records {
        if envelope.kind != Kind::Value {
            continue;
        }
        let record = Ledger::decode(envelope)?;
        let conversation = selected::conversation_id(&record);
        let owner = owners
            .get(&conversation)
            .context("Reader uncertainty ownership absent")?;
        let DocumentOwnership::Document(document) = owner else {
            bail!("Reader uncertainty document ownership unresolved")
        };
        if *document != request.source.document {
            continue;
        }
        match record {
            Record::Receipt(receipt) if receipt.admitted_intent.is_some() => {
                let operation = receipt.acknowledgment.operation;
                ensure!(
                    pending_operations.insert(operation, receipt).is_none(),
                    "Reader duplicate original pending receipt"
                );
            }
            Record::OutcomeFact(fact) if fact.reason == AttemptReason::OutputPending => {
                pending_facts.insert(fact.id);
            }
            _ => {}
        }
    }
    for fact in pending_facts {
        ensure!(
            pending_operations
                .keys()
                .any(|operation| pending::pending_id(*operation) == fact),
            "Reader uncertainty fact has no original receipt"
        );
    }
    for (operation, receipt) in pending_operations {
        if operation == request.operation && own.is_some() {
            continue;
        }
        let original = admission::recover_original(store, snapshot.token.scope(), operation)?
            .context("Reader uncertainty original publication absent")?;
        ensure!(
            original.receipt == receipt,
            "Reader uncertainty original receipt differs"
        );
        ensure!(
            !snapshot
                .transaction
                .selected
                .records
                .contains(&original.receipt_reference.object),
            "Reader earlier active pending intent unresolved"
        );
        ensure!(
            settlement::retained_uncertainty_in_store(store, snapshot, &original)?
                != IntentUncertainty::Unresolved,
            "Reader earlier retained pending intent unresolved"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pending::tests::{Fixture, MockSource};
    use std::cell::Cell;
    #[test]
    fn actual_store_accepts_only_exact_own_pending_and_blocks_another_operation() {
        let fixture = Fixture::new();
        let (_store, handle, token, request) = fixture.setup();
        handle
            .with_current_store(&token, |store, snapshot| {
                validate(store, snapshot, &request, None)
            })
            .unwrap();
        let source = MockSource {
            source: request.source.clone(),
            calls: Cell::new(0),
            refuse_at: None,
        };
        let PendingIntentPublication::Published { publication, .. } = handle
            .publish_pending(Some(&token), request.clone(), Some(&source))
            .unwrap()
        else {
            unreachable!()
        };
        handle
            .with_current_store(&publication.token, |store, snapshot| {
                let original =
                    admission::recover_original(store, handle.scope(), request.operation)?.unwrap();
                validate(store, snapshot, &request, Some(&original))?;
                let mut changed = request.clone();
                changed.source.page = Uuid::new_v4();
                assert!(validate(store, snapshot, &changed, Some(&original)).is_err());
                let mut changed = request.clone();
                changed.updated_ms += 1;
                assert!(validate(store, snapshot, &changed, Some(&original)).is_err());
                let mut different = request.clone();
                different.operation = Uuid::new_v4();
                assert!(validate(store, snapshot, &different, None).is_err());
                assert!(validate(store, snapshot, &different, Some(&original)).is_err());
                Ok(())
            })
            .unwrap();
    }
}

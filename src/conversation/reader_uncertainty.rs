//! Locked reads only; never reacquire the canonical admission gate here.
use super::*;
use crate::storage::selection::SelectedSnapshot;

#[cfg(test)]
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
    validate_document(store, snapshot, request.source.document, own)
}

struct PendingPair<'a> {
    receipt: Receipt,
    reference: &'a ObjectRef,
    fact: &'a ObjectRef,
    actor: Uuid,
}

fn collect_pairs<'a>(
    scope: &crate::storage::selection::SelectionScope,
    document: Uuid,
    records: &[Envelope],
    references: &'a [ObjectRef],
    pairs: &mut BTreeMap<Uuid, PendingPair<'a>>,
) -> Result<()> {
    ensure!(
        records.len() == references.len(),
        "uncertainty reference count differs"
    );
    let mut receipts = Vec::new();
    let mut facts = BTreeMap::new();
    for (envelope, reference) in records.iter().zip(references) {
        if envelope.namespace != Namespace::Conversation || envelope.kind != Kind::Value {
            continue;
        }
        match Ledger::decode(envelope)? {
            Record::Receipt(receipt) if receipt.admitted_intent.is_some() => {
                receipts.push((receipt, reference, envelope.actor_id));
            }
            Record::OutcomeFact(fact)
                if fact.reason == AttemptReason::OutputPending && fact.settlement.is_none() =>
            {
                let id = fact.id;
                ensure!(
                    facts
                        .insert(id, (fact, reference, envelope.actor_id))
                        .is_none(),
                    "duplicate original pending fact"
                );
            }
            _ => (),
        }
    }
    if receipts.is_empty() && facts.is_empty() {
        return Ok(());
    }

    // Project each original closure independently. Sibling revisions in two
    // manifests are alternatives and never make a combined active graph.
    let projection = SelectedDomainProjection::from_mixed_records(records, references, scope)?;
    let owners = projection.document_ownership()?;
    let matches_document = |conversation| -> Result<bool> {
        let owner = owners
            .get(&conversation)
            .context("uncertainty document ownership absent")?;
        let DocumentOwnership::Document(owner) = owner else {
            bail!("uncertainty document ownership unresolved");
        };
        Ok(*owner == document)
    };
    let mut relevant_facts = BTreeSet::new();
    for (id, (fact, _, _)) in &facts {
        if matches_document(fact.conversation)? {
            relevant_facts.insert(*id);
        }
    }
    for (receipt, reference, actor) in receipts {
        let intent = receipt.admitted_intent.as_ref().unwrap();
        if !matches_document(intent.conversation)? {
            continue;
        }
        let operation = intent.operation;
        let id = pending::pending_id(operation);
        let (fact, fact_reference, fact_actor) = facts
            .get(&id)
            .context("uncertainty receipt has no exact original pending fact")?;
        ensure!(
            intent.source.document == document
                && intent.selection.group == scope.group
                && intent.selection.key_sha256 == scope.key_sha256
                && intent.selection.binding_sha256 == scope.binding_sha256
                && fact.conversation == intent.conversation
                && fact.turn == intent.turn
                && fact.outcome == Outcome::ReconcileRequired
                && *fact_actor == actor,
            "uncertainty original receipt and fact differ"
        );
        relevant_facts.remove(&id);
        let pair = PendingPair {
            receipt,
            reference,
            fact: fact_reference,
            actor,
        };
        if let Some(previous) = pairs.insert(operation, pair) {
            ensure!(
                previous.reference == reference && previous.fact == *fact_reference,
                "uncertainty original receipt is ambiguous"
            );
        }
    }
    ensure!(
        relevant_facts.is_empty(),
        "uncertainty fact has no original receipt"
    );
    Ok(())
}

/// Check the document fence under the existing domain gate. This supplies
/// neither source admission nor a settlement capability.
pub(crate) fn validate_document(
    store: &Store,
    snapshot: &SelectedSnapshot,
    document: Uuid,
    own: Option<&HistoricalIntent>,
) -> Result<()> {
    let scope = snapshot.token.scope();
    let mut pairs = BTreeMap::new();
    collect_pairs(
        scope,
        document,
        &snapshot.selected_records,
        &snapshot.transaction.selected.records,
        &mut pairs,
    )?;
    let mut records = snapshot.retained_records.as_slice();
    for manifest in &snapshot.transaction.retained {
        ensure!(
            records.len() >= manifest.records.len(),
            "uncertainty retained reference count differs"
        );
        let (closure, tail) = records.split_at(manifest.records.len());
        collect_pairs(scope, document, closure, &manifest.records, &mut pairs)?;
        records = tail;
    }
    ensure!(
        records.is_empty(),
        "uncertainty retained reference count differs"
    );

    let mut foreign = Vec::new();
    for (operation, pair) in pairs {
        if pair.actor != store.actor_id {
            ensure!(
                store.selected_receipt(scope, operation)?.is_none(),
                "foreign pending intent conflicts with local operation custody"
            );
            foreign.push((pair.reference, pair.fact));
            continue;
        }
        let original = admission::recover_original(store, scope, operation)?
            .context("uncertainty original local publication absent")?;
        ensure!(
            original.receipt == pair.receipt
                && original.receipt_reference.object == *pair.reference,
            "uncertainty original local receipt differs"
        );
        if own.is_some_and(|own| own.receipt_reference == original.receipt_reference) {
            continue;
        }
        ensure!(
            !snapshot
                .transaction
                .selected
                .records
                .contains(pair.reference),
            "earlier active pending intent unresolved"
        );
        ensure!(
            settlement::retained_uncertainty_in_store(store, snapshot, &original)?
                != IntentUncertainty::Unresolved,
            "earlier retained pending intent unresolved"
        );
    }
    crate::storage::sync::selected::verify_received_intents(store, scope, &foreign)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pending::tests::{Fixture, MockSource};
    use std::cell::Cell;
    #[test]
    fn unbound_pending_fact_still_refuses_unknown_ownership() {
        let new_record = |actor_id, id, parents, record: Record, media| -> Result<Envelope> {
            let envelope = Envelope {
                envelope_version: FORMAT,
                namespace: Namespace::Conversation,
                domain_schema_version: SCHEMA,
                record_id: id,
                revision_id: Uuid::new_v4(),
                parents,
                operation_id: Uuid::new_v4(),
                actor_id,
                kind: Kind::Value,
                payload: serde_json::to_value(record)?,
                media_descriptors: media,
            };
            envelope.validate()?;
            Ledger::decode(&envelope)?;
            Ok(envelope)
        };
        let fixture = Fixture::new();
        let (store, handle, token, mut request) = fixture.setup();
        let conversation = Uuid::new_v4();
        let root = new_record(
            store.actor_id,
            conversation,
            BTreeSet::new(),
            Record::Root(Root {
                id: conversation,
                next_sequence: 1,
                binding: None,
                created_ms: 1,
                updated_ms: 1,
            }),
            vec![],
        )
        .unwrap();
        let turn_id = Uuid::new_v4();
        let turn = new_record(
            store.actor_id,
            turn_id,
            BTreeSet::new(),
            Record::Turn(Turn {
                id: turn_id,
                conversation,
                exchange: Uuid::new_v4(),
                sequence: 0,
                role: Role::Assistant,
                mode: Mode::Reader,
                outcome: Outcome::ReconcileRequired,
                text: Some("unbound pending draft".into()),
                sources: vec![],
                correction_of: None,
                created_ms: 1,
                updated_ms: 1,
                completion: None,
            }),
            vec![],
        )
        .unwrap();
        let fact_id = Uuid::new_v4();
        let fact = new_record(
            store.actor_id,
            fact_id,
            BTreeSet::new(),
            Record::OutcomeFact(OutcomeFact {
                id: fact_id,
                conversation,
                turn: turn_id,
                outcome: Outcome::ReconcileRequired,
                reason: AttemptReason::OutputPending,
                settlement: None,
            }),
            vec![],
        )
        .unwrap();
        let publication = store
            .commit_selected(
                &token,
                Uuid::new_v4(),
                vec![root, turn, fact],
                BTreeMap::new(),
            )
            .unwrap();
        request.selection = IntentSelectionEvidence::from_token(&publication.token);
        let error = handle
            .with_current_store(&publication.token, |store, snapshot| {
                validate(store, snapshot, &request, None)
            })
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("uncertainty document ownership unresolved"),
            "{error:#}"
        );
    }
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

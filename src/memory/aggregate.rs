//! Strict selected fact ownership; retained envelopes never supply current heads.
use super::{
    catalog::{Identity, Owner, Registration},
    Fact, Key, Scope,
};
use crate::{
    conversation::{DocumentOwnership, SelectedDomainProjection},
    storage::{
        selection::{AcceptedSelectionTransaction, SelectedSnapshot, SelectionMutation},
        Envelope, Kind, Namespace, ObjectRef, Uuid,
    },
};
use anyhow::{ensure, Context, Result};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct Aggregate {
    pub heads: Vec<Envelope>,
    pub history: BTreeMap<Uuid, Vec<Envelope>>,
}
impl Aggregate {
    pub fn project(
        identity: &Identity,
        registration: &Registration,
        snapshot: &SelectedSnapshot,
        lineage: &[AcceptedSelectionTransaction],
    ) -> Result<Self> {
        let scope = identity.aggregate_scope(&registration.owner)?;
        ensure!(
            snapshot.token.scope() == &scope && snapshot.transaction.scope == scope,
            "foreign memory aggregate selection"
        );
        ensure!(
            snapshot.selected_records.len() == snapshot.transaction.selected.records.len(),
            "incomplete aggregate references"
        );
        let genesis = lineage.last().context("aggregate genesis missing")?;
        ensure!(
            genesis.transaction.scope == scope
                && genesis.transaction.mutation == SelectionMutation::Initialize
                && genesis.transaction.operation == registration.genesis_operation
                && genesis.sha256 == registration.genesis_sha256,
            "memory aggregate genesis mismatch"
        );
        let mut history: BTreeMap<Uuid, Vec<Envelope>> = BTreeMap::new();
        let mut candidates: BTreeMap<Uuid, BTreeSet<Uuid>> = BTreeMap::new();
        let mut other_records = Vec::new();
        let mut other_refs: Vec<ObjectRef> = Vec::new();
        let mut immutable: BTreeMap<Uuid, (Key, String)> = BTreeMap::new();
        for (envelope, reference) in snapshot
            .selected_records
            .iter()
            .zip(&snapshot.transaction.selected.records)
        {
            envelope.validate()?;
            if envelope.namespace != Namespace::SubjectMemory {
                ensure!(
                    matches!(registration.owner, Owner::Document { .. })
                        && matches!(
                            envelope.namespace,
                            Namespace::Conversation
                                | Namespace::Source
                                | Namespace::ExportAssociation
                        ),
                    "unsupported mixed aggregate ownership"
                );
                other_records.push(envelope.clone());
                other_refs.push(reference.clone());
                continue;
            }
            ensure!(
                envelope.domain_schema_version == 1 && envelope.media_descriptors.is_empty(),
                "unsupported memory aggregate envelope"
            );
            if envelope.kind == Kind::Value {
                remember(
                    &mut immutable,
                    envelope.record_id,
                    owned_fact(envelope, registration)?,
                )?;
            } else {
                ensure!(
                    envelope.payload.is_null() && !envelope.parents.is_empty(),
                    "invalid selected forgetting"
                );
            }
            candidates
                .entry(envelope.record_id)
                .or_default()
                .insert(envelope.revision_id);
            history
                .entry(envelope.record_id)
                .or_default()
                .push(envelope.clone());
        }
        // Custody checks identity/claim only. These historical values never enter
        // current heads, query ranking, mutation parents or effect permission.
        let mut custody = immutable.clone();
        for accepted in lineage {
            ensure!(
                accepted.transaction.scope == scope
                    && accepted.selected_records.len()
                        == accepted.transaction.selected.records.len(),
                "incomplete accepted memory custody"
            );
            for envelope in &accepted.selected_records {
                if envelope.namespace != Namespace::SubjectMemory {
                    continue;
                }
                envelope.validate()?;
                ensure!(
                    envelope.domain_schema_version == 1 && envelope.media_descriptors.is_empty(),
                    "unsupported accepted memory custody"
                );
                if envelope.kind == Kind::Value {
                    remember(
                        &mut custody,
                        envelope.record_id,
                        owned_fact(envelope, registration)?,
                    )?;
                } else {
                    ensure!(
                        envelope.payload.is_null() && !envelope.parents.is_empty(),
                        "invalid historical forgetting"
                    );
                }
            }
        }
        for (record, envelopes) in &history {
            ensure!(
                immutable.contains_key(record),
                "memory tombstone has no owned ancestor"
            );
            for envelope in envelopes {
                for parent in &envelope.parents {
                    ensure!(
                        envelopes.iter().any(|prior| prior.revision_id == *parent),
                        "foreign memory causal parent"
                    );
                    candidates.get_mut(record).unwrap().remove(parent);
                }
            }
        }
        if let Owner::GlobalFact { key } = &registration.owner {
            ensure!(
                history.len() == 1
                    && history.contains_key(&key.record_id()?)
                    && snapshot.transaction.selected.media.is_empty()
                    && snapshot.transaction.selected.media_coverage.is_empty(),
                "incomplete independent global aggregate"
            );
        }
        if !other_records.is_empty() {
            let Owner::Document { document } = registration.owner else {
                unreachable!()
            };
            let projection =
                SelectedDomainProjection::from_pinned_records(&other_records, &other_refs)?;
            let owners = projection.document_ownership()?;
            ensure!(
                !owners.is_empty()
                    && owners
                        .values()
                        .all(|owner| *owner == DocumentOwnership::Document(document)),
                "mixed document ownership unavailable or ambiguous"
            );
        }
        let mut heads = Vec::new();
        for (record, revisions) in candidates {
            ensure!(revisions.len() == 1, "conflicted memory selected head");
            let revision = revisions.into_iter().next().unwrap();
            heads.push(
                history[&record]
                    .iter()
                    .find(|envelope| envelope.revision_id == revision)
                    .unwrap()
                    .clone(),
            );
        }
        Ok(Self { heads, history })
    }
}

fn owned_fact(envelope: &Envelope, registration: &Registration) -> Result<Fact> {
    let fact: Fact =
        serde_json::from_value(envelope.payload.clone()).context("unsupported memory fact")?;
    fact.validate()?;
    ensure!(
        fact.key.record_id()? == envelope.record_id,
        "memory fact identity mismatch"
    );
    match &registration.owner {
        Owner::GlobalFact { key } => ensure!(fact.key == *key, "foreign global fact"),
        Owner::Document { document } => ensure!(
            fact.key.scope
                == Scope::Document {
                    document: *document
                },
            "foreign document fact"
        ),
    }
    Ok(fact)
}
fn remember(immutable: &mut BTreeMap<Uuid, (Key, String)>, record: Uuid, fact: Fact) -> Result<()> {
    if let Some((key, claim)) = immutable.get(&record) {
        ensure!(
            *key == fact.key && *claim == fact.claim,
            "memory immutable key or claim changed"
        );
    } else {
        immutable.insert(record, (fact.key, fact.claim));
    }
    Ok(())
}

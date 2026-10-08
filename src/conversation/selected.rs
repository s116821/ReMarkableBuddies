//! Causal projection of one pinned selected closure. No all-history lookup.
use super::{IntentRecordRef, Ledger, Record};
use crate::storage::selection::SelectedSnapshot;
use crate::storage::{digest, Envelope, Kind, Namespace, Uuid, MAX_ITEMS, MAX_METADATA};
use anyhow::{ensure, Context, Result};
use std::collections::{BTreeMap, BTreeSet};

pub struct SelectedDomainProjection {
    ancestors: BTreeMap<Uuid, Envelope>,
    records: BTreeMap<Uuid, Record>,
    heads: BTreeMap<(Namespace, Uuid), Uuid>,
}
impl SelectedDomainProjection {
    /// Retained records deliberately do not enter current projection.
    pub fn from_snapshot(snapshot: &SelectedSnapshot) -> Result<Self> {
        Self::from_records(&snapshot.selected_records)
    }
    fn from_records(records: &[Envelope]) -> Result<Self> {
        ensure!(
            !records.is_empty() && records.len() <= MAX_ITEMS,
            "invalid selected domain bound"
        );
        let mut ancestors = BTreeMap::new();
        let mut decoded = BTreeMap::new();
        let mut candidates: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        let mut bytes = 0usize;
        for envelope in records {
            envelope.validate()?;
            ensure!(
                matches!(
                    envelope.namespace,
                    Namespace::Conversation | Namespace::Source | Namespace::ExportAssociation
                ),
                "unsupported selected domain namespace; preserve and defer"
            );
            bytes = bytes
                .checked_add(serde_json::to_vec(envelope)?.len())
                .context("selected domain size overflow")?;
            ensure!(bytes <= MAX_METADATA, "selected domain exceeds bound");
            ensure!(
                ancestors
                    .insert(envelope.revision_id, envelope.clone())
                    .is_none(),
                "duplicate selected revision"
            );
            if envelope.kind == Kind::Value {
                decoded.insert(envelope.revision_id, Ledger::decode(envelope)?);
            }
            candidates
                .entry((envelope.namespace, envelope.record_id))
                .or_default()
                .insert(envelope.revision_id);
        }
        for envelope in records {
            for parent in &envelope.parents {
                let prior = ancestors.get(parent).context("selected ancestor missing")?;
                ensure!(
                    prior.namespace == envelope.namespace && prior.record_id == envelope.record_id,
                    "foreign selected parent"
                );
                candidates
                    .get_mut(&(envelope.namespace, envelope.record_id))
                    .unwrap()
                    .remove(parent);
            }
        }
        let mut outstanding: BTreeMap<_, _> = records
            .iter()
            .map(|e| (e.revision_id, e.parents.len()))
            .collect();
        let mut children: BTreeMap<Uuid, Vec<Uuid>> = BTreeMap::new();
        for envelope in records {
            for parent in &envelope.parents {
                children
                    .entry(*parent)
                    .or_default()
                    .push(envelope.revision_id);
            }
        }
        let mut ready: Vec<_> = outstanding
            .iter()
            .filter_map(|(id, count)| (*count == 0).then_some(*id))
            .collect();
        let mut processed = 0usize;
        while let Some(revision) = ready.pop() {
            processed += 1;
            for child in children.get(&revision).into_iter().flatten() {
                let count = outstanding.get_mut(child).unwrap();
                *count -= 1;
                if *count == 0 {
                    ready.push(*child);
                }
            }
        }
        ensure!(processed == ancestors.len(), "selected causal cycle");
        let mut heads = BTreeMap::new();
        for (key, revisions) in candidates {
            ensure!(revisions.len() == 1, "selected causal fork or cycle");
            heads.insert(key, *revisions.first().unwrap());
        }
        // Every ancestor must be reachable from a current head. A disconnected
        // cycle cannot hide beside a valid head and pass the head-count check.
        let mut reached = BTreeSet::new();
        let mut pending: Vec<_> = heads.values().copied().collect();
        while let Some(revision) = pending.pop() {
            if reached.insert(revision) {
                pending.extend(&ancestors[&revision].parents);
            }
        }
        ensure!(
            reached.len() == ancestors.len(),
            "unreachable selected causal history"
        );
        Ok(Self {
            ancestors,
            records: decoded,
            heads,
        })
    }
    pub fn head(&self, namespace: Namespace, id: Uuid) -> Option<(&Envelope, Option<&Record>)> {
        let revision = self.heads.get(&(namespace, id))?;
        Some((&self.ancestors[revision], self.records.get(revision)))
    }
    pub fn exact_reference(&self, reference: &IntentRecordRef) -> Result<&Record> {
        reference.validate()?;
        let envelope = self
            .ancestors
            .get(&reference.revision_id)
            .context("intent revision outside selected closure")?;
        let bytes = serde_json::to_vec(envelope)?;
        ensure!(
            envelope.namespace == reference.namespace
                && envelope.record_id == reference.record_id
                && bytes.len() as u64 == reference.object.bytes
                && digest(&bytes) == reference.object.sha256,
            "intent immutable reference mismatch"
        );
        self.records
            .get(&reference.revision_id)
            .context("intent reference is a tombstone")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{Root, SCHEMA};
    use crate::storage::{ObjectRef, FORMAT};
    fn root() -> Envelope {
        let id = Uuid::new_v4();
        Envelope {
            envelope_version: FORMAT,
            namespace: Namespace::Conversation,
            domain_schema_version: SCHEMA,
            record_id: id,
            revision_id: Uuid::new_v4(),
            parents: BTreeSet::new(),
            operation_id: Uuid::new_v4(),
            actor_id: Uuid::new_v4(),
            kind: Kind::Value,
            payload: serde_json::to_value(Record::Root(Root {
                id,
                next_sequence: 1,
                binding: None,
                created_ms: 1,
                updated_ms: 1,
            }))
            .unwrap(),
            media_descriptors: vec![],
        }
    }
    fn edit(prior: &Envelope) -> Envelope {
        let mut next = prior.clone();
        next.revision_id = Uuid::new_v4();
        next.operation_id = Uuid::new_v4();
        next.parents = BTreeSet::from([prior.revision_id]);
        next
    }
    #[test]
    fn ordinary_chain_has_one_head_and_exact_ancestor_lookup() {
        let first = root();
        let second = edit(&first);
        let third = edit(&second);
        let view = SelectedDomainProjection::from_records(&[third.clone(), first.clone(), second])
            .unwrap();
        assert_eq!(
            view.head(first.namespace, first.record_id)
                .unwrap()
                .0
                .revision_id,
            third.revision_id
        );
        let bytes = serde_json::to_vec(&first).unwrap();
        let mut reference = IntentRecordRef {
            namespace: first.namespace,
            record_id: first.record_id,
            revision_id: first.revision_id,
            object: ObjectRef {
                sha256: digest(&bytes),
                bytes: bytes.len() as u64,
            },
        };
        assert!(view.exact_reference(&reference).is_ok());
        reference.object.sha256 = "0".repeat(64);
        assert!(view.exact_reference(&reference).is_err());
        reference.revision_id = Uuid::new_v4();
        assert!(view.exact_reference(&reference).is_err());
    }
    #[test]
    fn fork_missing_foreign_and_duplicate_parents_refuse() {
        let first = root();
        let second = edit(&first);
        let fork = edit(&first);
        assert!(
            SelectedDomainProjection::from_records(&[first.clone(), second.clone(), fork]).is_err()
        );
        assert!(SelectedDomainProjection::from_records(std::slice::from_ref(&second)).is_err());
        let foreign = root();
        let mut wrong = second.clone();
        wrong.parents = BTreeSet::from([foreign.revision_id]);
        assert!(SelectedDomainProjection::from_records(&[first.clone(), foreign, wrong]).is_err());
        assert!(SelectedDomainProjection::from_records(&[first.clone(), first]).is_err());
    }
    #[test]
    fn selected_tombstone_does_not_resolve_a_historical_value_as_current() {
        let first = root();
        let mut deleted = edit(&first);
        deleted.kind = Kind::Tombstone;
        deleted.payload = serde_json::Value::Null;
        let view = SelectedDomainProjection::from_records(&[first.clone(), deleted]).unwrap();
        let (head, record) = view.head(first.namespace, first.record_id).unwrap();
        assert_eq!(head.kind, Kind::Tombstone);
        assert!(record.is_none());
        assert!(view.head(first.namespace, Uuid::new_v4()).is_none());
    }
    #[test]
    fn cycle_behind_a_single_apparent_head_refuses() {
        let mut first = root();
        let second = edit(&first);
        let tail = edit(&second);
        first.parents.insert(second.revision_id);
        assert!(SelectedDomainProjection::from_records(&[first, second, tail]).is_err());
    }
}

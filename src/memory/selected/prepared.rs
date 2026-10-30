use super::*;
use crate::memory::prepared::{self as evidence, Domain, Intent, Pin};
use crate::storage::conditional::Refused;

fn pins(view: &View) -> Vec<Pin> {
    let mut pins = vec![Pin::from_read(&view.catalog_read)];
    pins.extend(view.reads.values().map(Pin::from_read));
    pins.sort_by(|a, b| a.scope.key_sha256.cmp(&b.scope.key_sha256));
    pins
}
fn seal(mut view: View, key: &Key) -> Result<super::super::Inspection> {
    let heads = view
        .aggregates
        .values()
        .flat_map(|a| &a.heads)
        .filter(|e| key.record_id().is_ok_and(|id| id == e.record_id))
        .cloned()
        .collect();
    let mut admission = QueryResult::empty(Status::Complete);
    view.pins.result_sha256 = Some(result_identity(&admission)?);
    admission.pins = Some(Arc::new(Mutex::new(view.pins)));
    super::super::Inspection::new(heads, Some(admission))
}
fn prepared_receipt(
    read: &SelectedReadSnapshot,
    intent: &Intent,
    target: &Pin,
    envelope: &Envelope,
) -> Result<Option<Uuid>> {
    let lineage: Vec<_> = read
        .accepted_history
        .iter()
        .map(|a| a.transaction.clone())
        .collect();
    let prior = original_receipt(&lineage, envelope).map_err(|_| Refused)?;
    if let Some(index) = lineage.iter().position(|t| t.operation == intent.operation) {
        let predecessor = read
            .accepted_history
            .get(index + 1)
            .context("prepared predecessor absent")?;
        if !target.matches_history(predecessor)? {
            return Err(Refused.into());
        }
        evidence::verify_parents(intent, &predecessor.selected_records).map_err(|_| Refused)?;
    }
    Ok(prior)
}
impl SelectedMemory {
    pub(crate) fn prepare_forget_view(
        &self,
        key: &Key,
    ) -> Result<(Vec<Pin>, super::super::Inspection)> {
        self.own(key)?;
        let view = self.read()?;
        let evidence = pins(&view);
        Ok((evidence, seal(view, key)?))
    }
    fn check_other_pins(&self, view: &View, intent: &Intent, target: &str) -> Result<()> {
        let Domain::Selected {
            identity,
            pins: expected,
        } = &intent.binding.domain
        else {
            return Err(Refused.into());
        };
        if identity != &self.identity {
            return Err(Refused.into());
        }
        let actual = pins(view);
        if actual.len() != expected.len()
            || actual
                .iter()
                .zip(expected)
                .any(|(a, b)| a.scope != b.scope || (a.scope.key_sha256 != target && a != b))
        {
            return Err(Refused.into());
        }
        Ok(())
    }
    pub(crate) fn prepared_forget(
        &self,
        intent: &Intent,
        receipt_only: bool,
        attempted: &mut bool,
    ) -> Result<(Uuid, super::super::Inspection)> {
        self.prepared_forget_handoff(intent, receipt_only, attempted, || Ok(()))
    }
    // The no-op production continuation keeps the two phases explicit. Tests
    // exercise a legitimate concurrent publication at this exact boundary.
    fn prepared_forget_handoff(
        &self,
        intent: &Intent,
        receipt_only: bool,
        attempted: &mut bool,
        before_handoff: impl FnOnce() -> Result<()>,
    ) -> Result<(Uuid, super::super::Inspection)> {
        self.own(&intent.key)?;
        let envelope = intent.envelope(&self.store)?;
        let owner = match intent.key.scope {
            super::super::Scope::Global => Owner::GlobalFact {
                key: intent.key.clone(),
            },
            super::super::Scope::Document { document } => Owner::Document { document },
        };
        let target = owner.key_sha256()?;
        let mut view = self.read()?;
        self.check_other_pins(&view, intent, &target)?;
        let Domain::Selected { pins: expected, .. } = &intent.binding.domain else {
            return Err(Refused.into());
        };
        let expected_target = expected
            .iter()
            .find(|p| p.scope.key_sha256 == target)
            .ok_or(Refused)?;
        let read = view
            .reads
            .get(&target)
            .context("prepared target registration absent")?;
        let token = read.snapshot.token.clone();
        let prior = prepared_receipt(read, intent, expected_target, &envelope)?;
        if prior.is_none() {
            if receipt_only {
                anyhow::bail!("original receipt unresolved")
            }
            if *expected_target != Pin::from_read(read) {
                return Err(Refused.into());
            }
            let heads = &view
                .aggregates
                .get(&target)
                .context("prepared aggregate absent")?
                .heads;
            let current: Vec<_> = heads
                .iter()
                .filter(|p| p.record_id == envelope.record_id)
                .cloned()
                .collect();
            if current
                .iter()
                .map(|e| e.revision_id)
                .collect::<std::collections::BTreeSet<_>>()
                != envelope.parents
            {
                return Err(Refused.into());
            }
            evidence::verify_parents(intent, &current).map_err(|_| Refused)?;
        }
        let transaction = view.pins.guard(&self.store, || {
            if let Some(prior) = prior {
                return Ok(prior);
            }
            // Actual selected CAS still checks this fresh token under the Store mutex.
            *attempted = true;
            Ok(self
                .store
                .commit_selected(
                    &token,
                    intent.operation,
                    vec![envelope.clone()],
                    BTreeMap::new(),
                )?
                .transaction
                .selected
                .transaction_id)
        })?;
        // Seal a fresh post-operation admission through stdout. Own commit changed the
        // target pin; all other original pins must still be current, not substituted.
        before_handoff()?;
        let fresh = self.read()?;
        self.check_other_pins(&fresh, intent, &target)?;
        let fresh_target = fresh
            .reads
            .get(&target)
            .context("fresh prepared target absent")?;
        if prepared_receipt(fresh_target, intent, expected_target, &envelope)? != Some(transaction)
        {
            return Err(Refused.into());
        }
        Ok((transaction, seal(fresh, &intent.key)?))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        memory::{
            self,
            prepared::{Binding, Mode},
        },
        storage::{self, StorePaths},
    };
    #[test]
    fn prepared_fresh_handoff_revalidates_original_receipt_after_target_activation() {
        let root = std::env::temp_dir().join(format!("prepared-fresh-handoff-{}", Uuid::new_v4()));
        let store = Arc::new(
            Store::open(StorePaths {
                data: root.join("data"),
                cache: root.join("cache"),
                credentials: root.join("credentials"),
            })
            .unwrap(),
        );
        let identity = Identity {
            group: Uuid::new_v4(),
            profile: Uuid::new_v4(),
            binding_sha256: storage::digest(b"fresh handoff binding"),
        };
        store
            .set_sync_mode(
                &store.sync_mode_snapshot().unwrap().token,
                Uuid::new_v4(),
                storage::sync::mode::SyncModeChoice::SharedGroup {
                    group: identity.group,
                    binding_sha256: identity.binding_sha256.clone(),
                    root_id: "fixture".into(),
                },
            )
            .unwrap();
        let memory = SelectedMemory::new(store.clone(), identity.clone()).unwrap();
        let key = memory::Key {
            profile: identity.profile,
            scope: memory::Scope::Global,
            fact: Uuid::new_v4(),
        };
        let fact = memory::Fact {
            key: key.clone(),
            claim: "units".into(),
            value: "SI".into(),
            subject: "physics".into(),
            tags: vec![],
            provenance: memory::Provenance {
                basis: memory::Basis::UserConfirmed,
                description: "fixture".into(),
                conversation: None,
                turn: None,
                source: None,
            },
            confidence_basis_points: 8000,
            observed_ms: 1,
            expires_ms: None,
        };
        let registration = memory
            .initialize_aggregate(InitializeAggregate {
                operation: Uuid::new_v4(),
                accepted_base_sha256: storage::digest(b"aggregate"),
                fact,
            })
            .unwrap()
            .registration
            .unwrap();
        memory
            .initialize_selection(InitializeSelection {
                operation: Uuid::new_v4(),
                accepted_base_sha256: storage::digest(b"catalog"),
                registrations: vec![registration],
            })
            .unwrap();
        let token = store.sync_mode_snapshot().unwrap().token;
        let _mode = store.sync_mode_lease(&token).unwrap();
        let (pins, inspection) = memory.prepare_forget_view(&key).unwrap();
        let intent = Intent {
            prepared_version: 1,
            operation: Uuid::new_v4(),
            key: key.clone(),
            expected: evidence::expected(&inspection.envelopes).unwrap(),
            binding: Binding {
                config_sha256: storage::digest(b"direct domain fixture"),
                mode: serde_json::from_value::<Mode>(serde_json::to_value(&token).unwrap())
                    .unwrap(),
                actor: store.actor_id,
                store_generation: pins[0].store_generation,
                domain: Domain::Selected {
                    identity: identity.clone(),
                    pins,
                },
            },
        };
        intent.validate(identity.profile).unwrap();
        let scope = identity
            .aggregate_scope(&Owner::GlobalFact { key: key.clone() })
            .unwrap();
        let replacement_base = storage::digest(b"different accepted target after acknowledgement");
        let mut attempted = false;
        let (transaction, sealed) = memory
            .prepared_forget_handoff(&intent, false, &mut attempted, || {
                let current = store
                    .selected_snapshot(&scope, 4096)?
                    .context("fixture target absent")?;
                let publication = store.activate_selected(
                    &current.token,
                    storage::selection::SelectionChange {
                        operation: Uuid::new_v4(),
                        accepted_base_sha256: replacement_base.clone(),
                        selected: current.transaction.selected.clone(),
                        retained: current.transaction.retained.clone(),
                    },
                    BTreeMap::new(),
                )?;
                ensure!(
                    publication.transaction.aggregate_generation
                        != current.token.aggregate_generation(),
                    "fixture replacement did not occur"
                );
                Ok(())
            })
            .unwrap();
        assert!(attempted);
        let mut consumed = false;
        sealed
            .with_current(&memory, &mut || {
                consumed = true;
                Ok(())
            })
            .unwrap();
        assert!(consumed);
        let read = store
            .selected_snapshot_with_budget(&scope, 4096, &mut budget())
            .unwrap()
            .unwrap();
        assert_eq!(
            read.snapshot.transaction.accepted_base_sha256,
            replacement_base
        );
        let Domain::Selected { pins, .. } = &intent.binding.domain else {
            panic!("selection absent")
        };
        let original = pins.iter().find(|p| p.scope == scope).unwrap();
        assert_eq!(
            prepared_receipt(&read, &intent, original, &intent.envelope(&store).unwrap()).unwrap(),
            Some(transaction)
        );
        // A different current accepted base is not silently treated as the original;
        // only the exact historical logical receipt/direct predecessor is acknowledged.
        assert_ne!(
            original.accepted_base_sha256,
            read.snapshot.token.accepted_base_sha256()
        );
        drop(_mode);
        drop(memory);
        drop(store);
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        let _ = std::fs::remove_dir_all(root);
    }
}

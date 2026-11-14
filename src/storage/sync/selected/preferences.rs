//! Portable declaration custody in the existing Worker. No request activation.
use super::*;
use crate::config::portable::record;
use crate::storage::selection::{
    SelectedReadBudget, SelectionChange, SelectionMutation, SelectionPublication,
};

fn allowance() -> SelectedReadBudget {
    SelectedReadBudget::new(64, MAX_ITEMS as u64, MAX_METADATA as u64, MAX_MEDIA)
}
fn scope(group: Uuid, binding: &str) -> SelectionScope {
    SelectionScope {
        group,
        key_sha256: record::key_sha256(),
        binding_sha256: binding.into(),
    }
}
pub(super) fn project(
    store: &Store,
    scope: &SelectionScope,
    manifest: &Manifest,
    budget: &mut SelectedReadBudget,
) -> Result<record::Projection> {
    let generation = catalog::current_generation(store)?;
    let directory = store
        .paths
        .data
        .join("generations")
        .join(generation.to_string())
        .join("objects");
    record::project(scope, manifest, |reference| {
        budget.charge(0, 1, 0, 0)?;
        files::read_with_budget(
            &directory.join(&reference.sha256),
            reference.bytes,
            Some(budget),
        )
    })
}
fn original(
    store: &Store,
    scope: &SelectionScope,
    root: &str,
    budget: &mut SelectedReadBudget,
) -> Result<Option<PortableRegistration>> {
    let Some(read) = store.selected_snapshot_with_budget(scope, MAX_ITEMS, budget)? else {
        return Ok(None);
    };
    project(store, scope, &read.snapshot.transaction.selected, budget)?;
    let genesis = read
        .accepted_history
        .last()
        .context("portable registration genesis absent")?;
    ensure!(
        genesis.transaction.mutation == SelectionMutation::Initialize
            && genesis.transaction.accepted_base_sha256 == genesis_base(root, &scope.key_sha256)
            && genesis.selected_records.len() == 1,
        "portable registration belongs to another root"
    );
    project(store, scope, &genesis.transaction.selected, budget)?;
    let record = &genesis.selected_records[0];
    ensure!(
        record.actor_id == store.actor_id && record.operation_id == genesis.transaction.operation,
        "portable registration requires an explicit local original"
    );
    Ok(Some(PortableRegistration {
        genesis_operation: genesis.transaction.operation,
        genesis_sha256: genesis.sha256.clone(),
    }))
}
pub(super) fn discover_registration(
    store: &Arc<Store>,
    binding: &Binding,
    view: &CatalogView,
    budget: &mut SelectedReadBudget,
) -> Result<Option<CatalogPending>> {
    let scope = scope(binding.group, &binding.digest);
    let key = scope.key_sha256.clone();
    if let Some(existing) = view.entries.get(&key) {
        ensure!(
            existing.owner == Owner::preferences(),
            "portable catalog owner differs"
        );
        return Ok(None);
    }
    let admission = SelectedAdmission::new(store.clone(), scope.clone())?;
    SelectedAdmission::with_gates(&[&admission], |store| {
        let Some(portable) = original(store, &scope, &view.root_sha, budget)? else {
            return Ok(None);
        };
        Ok(Some(CatalogPending {
            operation: Uuid::new_v5(
                &portable.genesis_operation,
                format!("shared-catalog-register:v1:{key}").as_bytes(),
            ),
            source_generation: catalog::current_generation(store)?,
            registration: None,
            portable: Some(portable),
            handwriting: None,
            genesis_slot: None,
            proposal: None,
        }))
    })
}
pub(super) fn require_registration(
    store: &Store,
    group: Uuid,
    binding: &str,
    expected: &PortableRegistration,
    root: &str,
    budget: &mut SelectedReadBudget,
) -> Result<()> {
    let current = original(store, &scope(group, binding), root, budget)?
        .context("portable registration source absent")?;
    ensure!(
        current.genesis_operation == expected.genesis_operation
            && current.genesis_sha256 == expected.genesis_sha256,
        "portable registration original changed"
    );
    Ok(())
}
pub(super) fn require_initial_publication(
    store: &Store,
    scope: &SelectionScope,
    root: &str,
    budget: &mut SelectedReadBudget,
) -> Result<()> {
    original(store, scope, root, budget)?.context("portable local original absent")?;
    Ok(())
}

pub(super) struct Winner {
    scope: SelectionScope,
    correction: Option<HandwritingOwner>,
    remote: String,
    manifest: Manifest,
    objects: BTreeMap<String, Vec<u8>>,
}
impl Winner {
    pub(super) fn validate(
        store: &Store,
        scope: SelectionScope,
        remote: String,
        manifest: Manifest,
        objects: BTreeMap<String, Vec<u8>>,
        correction: Option<HandwritingOwner>,
    ) -> Result<Self> {
        ensure!(valid_digest(&remote), "remote descriptor absent");
        if correction.is_some() {
            let generation = catalog::current_generation(store)?;
            crate::storage::compound_generation::validate_ready(
                &store.paths,
                store.actor_id,
                generation,
            )?;
        }
        store.stage_selected_candidate(&manifest, &objects)?;
        let load = |reference: &ObjectRef| {
            objects
                .get(&reference.sha256)
                .cloned()
                .context("incoming original absent")
        };
        if let Some(owner) = correction {
            ensure!(
                scope.key_sha256 == owner.domain()?.key_sha256(),
                "correction scope differs"
            );
            corrections::read(owner, &manifest, load)?;
        } else {
            record::project(&scope, &manifest, load)?;
        }
        Ok(Self {
            scope,
            correction,
            remote,
            manifest,
            objects,
        })
    }

    fn project(
        &self,
        store: &Store,
        manifest: &Manifest,
        budget: &mut SelectedReadBudget,
    ) -> Result<()> {
        if let Some(owner) = self.correction {
            corrections::project(store, owner, manifest, budget)?;
        } else {
            project(store, &self.scope, manifest, budget)?;
        }
        Ok(())
    }

    pub(super) fn apply(
        self,
        store: &Arc<Store>,
        operation: Uuid,
        expected: Option<&SelectionToken>,
    ) -> Result<SelectionPublication> {
        let admission = SelectedAdmission::new(store.clone(), self.scope.clone())?;
        let _mode = admission.reader_mode_lease()?;
        let catalog = self
            .correction
            .map(|owner| corrections::catalog_gate(store, &self.scope, owner.profile))
            .transpose()?;
        let mut gates = vec![&admission];
        gates.extend(catalog.iter());
        SelectedAdmission::with_gates(&gates, |store| {
            if let Some(receipt) = store.selected_receipt(&self.scope, operation)? {
                ensure!(
                    matches!(
                        receipt.mutation,
                        SelectionMutation::Initialize | SelectionMutation::Activate
                    ) && receipt.accepted_base_sha256 == self.remote
                        && shared::same_membership(&receipt.selected, &self.manifest),
                    "portable receiving original differs"
                );
                let predecessor = if receipt.mutation == SelectionMutation::Initialize {
                    None
                } else {
                    store.historical_predecessor_token(&self.scope, operation)?
                };
                ensure!(
                    predecessor.as_ref() == expected,
                    "portable receiving predecessor differs"
                );
                let token = store
                    .historical_selection_token(&self.scope, operation)?
                    .context("portable receiving historical token absent")?;
                return Ok(SelectionPublication {
                    token,
                    transaction: receipt,
                    replayed: true,
                });
            }
            let mut budget = allowance();
            let current = store
                .selected_snapshot_with_budget(&self.scope, MAX_ITEMS, &mut budget)?
                .map(|read| read.snapshot);
            ensure!(
                match (&current, expected) {
                    (None, None) => true,
                    (Some(current), Some(expected)) => current.token == *expected,
                    _ => false,
                },
                "portable receiving current condition changed"
            );
            let mut retained = vec![];
            if let Some(current) = &current {
                self.project(store, &current.transaction.selected, &mut budget)?;
                retained.extend(current.transaction.retained.clone());
                retained.push(current.transaction.selected.clone());
                if self.correction.is_some() {
                    for prior in &current.transaction.retained {
                        self.project(store, prior, &mut budget)?;
                    }
                }
            }
            retained.retain(|prior| {
                prior
                    .records
                    .iter()
                    .any(|reference| !self.manifest.records.contains(reference))
                    || prior
                        .media
                        .iter()
                        .any(|reference| !self.manifest.media.contains(reference))
                    || prior
                        .media_coverage
                        .iter()
                        .any(|coverage| !self.manifest.media_coverage.contains(coverage))
            });
            let mut seen = BTreeSet::new();
            let mut unique = vec![];
            for manifest in retained {
                if seen.insert(digest(&serde_json::to_vec(&manifest)?)) {
                    if self.correction.is_none() {
                        self.project(store, &manifest, &mut budget)?;
                    }
                    unique.push(manifest);
                }
            }
            let change = SelectionChange {
                operation,
                accepted_base_sha256: self.remote,
                selected: self.manifest,
                retained: unique,
            };
            match expected {
                Some(expected) => store.activate_selected(expected, change, self.objects),
                None => store.initialize_selected(&self.scope, change, self.objects),
            }
        })
    }
}

#[cfg(test)]
mod tests;

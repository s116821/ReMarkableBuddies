//! Additive catalog publication; user-content candidates are never rebased.
use super::*;
use crate::memory::catalog::{Catalog as LocalCatalog, Identity, Registration};
use crate::storage::selection::SelectedReadBudget;

fn allowance() -> SelectedReadBudget {
    SelectedReadBudget::new(64, MAX_ITEMS as u64, MAX_METADATA as u64, MAX_MEDIA)
}
fn identity(binding: &Binding, view: &CatalogView) -> Identity {
    Identity {
        group: binding.group,
        profile: view.root.profile,
        binding_sha256: binding.digest.clone(),
    }
}
pub(super) fn current_generation(store: &Store) -> Result<Uuid> {
    Ok(store
        .inner
        .lock()
        .map_err(|_| anyhow::anyhow!("catalog Store mutex unavailable"))?
        .generation)
}
fn local_catalog(
    store: &Store,
    identity: &Identity,
    budget: &mut SelectedReadBudget,
) -> Result<Option<LocalCatalog>> {
    store
        .selected_snapshot_with_budget(&identity.scope()?, MAX_ITEMS, budget)?
        .map(|read| LocalCatalog::project(identity, &read.snapshot, &read.accepted_history))
        .transpose()
}
/// Caller owns both ordered gates. A local registration and its original raw
/// genesis must agree; current or imported all-history heads are not discovery.
fn require_registration(
    store: &Store,
    identity: &Identity,
    registration: &Registration,
    root: &str,
    generation: Uuid,
    budget: &mut SelectedReadBudget,
) -> Result<()> {
    ensure!(
        current_generation(store)? == generation,
        "catalog source generation changed"
    );
    registration.validate(identity)?;
    let catalog =
        local_catalog(store, identity, budget)?.context("catalog local registration absent")?;
    ensure!(
        catalog.registrations.get(&registration.key_sha256) == Some(registration),
        "catalog local registration changed"
    );
    let scope = identity.aggregate_scope(&registration.owner)?;
    let read = store
        .selected_snapshot_with_budget(&scope, MAX_ITEMS, budget)?
        .context("catalog local aggregate absent")?;
    crate::memory::aggregate::Aggregate::project(
        identity,
        registration,
        &read.snapshot,
        &read.accepted_history,
    )?;
    let genesis = read
        .accepted_history
        .last()
        .context("catalog original genesis absent")?;
    ensure!(
        genesis.transaction.accepted_base_sha256 == genesis_base(root, &scope.key_sha256),
        "catalog local genesis belongs to another root"
    );
    Ok(())
}
fn admissions(
    store: &Arc<Store>,
    identity: &Identity,
    registration: &Registration,
) -> Result<(SelectedAdmission, SelectedAdmission)> {
    crate::memory::selected::receiving_gates(store, identity, &registration.owner)
}
fn source_admissions(
    store: &Arc<Store>,
    identity: &Identity,
    pending: &CatalogPending,
) -> Result<Vec<SelectedAdmission>> {
    match (
        &pending.registration,
        &pending.portable,
        &pending.handwriting,
    ) {
        (Some(registration), None, None) => {
            let (catalog, aggregate) = admissions(store, identity, registration)?;
            Ok(vec![catalog, aggregate])
        }
        (None, Some(_), None) => Ok(vec![SelectedAdmission::new(
            store.clone(),
            SelectionScope {
                group: identity.group,
                key_sha256: crate::config::portable::record::key_sha256(),
                binding_sha256: identity.binding_sha256.clone(),
            },
        )?]),
        (None, None, Some(correction)) => {
            let scope = correction
                .owner
                .domain()?
                .scope(identity.group, identity.binding_sha256.clone())
                .map_err(|error| anyhow::anyhow!("invalid handwriting scope: {error:?}"))?;
            Ok(vec![
                corrections::catalog_gate(store, &scope, identity.profile)?,
                SelectedAdmission::new(store.clone(), scope)?,
            ])
        }
        _ => anyhow::bail!("catalog original source is ambiguous"),
    }
}
fn require_source(
    store: &Store,
    identity: &Identity,
    pending: &CatalogPending,
    root: &str,
    budget: &mut SelectedReadBudget,
) -> Result<()> {
    ensure!(
        current_generation(store)? == pending.source_generation,
        "catalog source generation changed"
    );
    match (
        &pending.registration,
        &pending.portable,
        &pending.handwriting,
    ) {
        (Some(registration), None, None) => require_registration(
            store,
            identity,
            registration,
            root,
            pending.source_generation,
            budget,
        ),
        (None, Some(portable), None) => preferences::require_registration(
            store,
            identity.group,
            &identity.binding_sha256,
            portable,
            root,
            budget,
        ),
        (None, None, Some(correction)) => {
            corrections::require_registration(store, identity, correction, root, budget)
        }
        _ => anyhow::bail!("catalog original source is ambiguous"),
    }
}

impl Engine {
    fn discover_registration(
        &self,
        view: &CatalogView,
        budget: &mut SelectedReadBudget,
    ) -> Result<Option<CatalogPending>> {
        let identity = identity(&self.binding, view);
        let catalog_gate = SelectedAdmission::new(self.store.clone(), identity.scope()?)?;
        let registrations = SelectedAdmission::with_gates(&[&catalog_gate], |store| {
            Ok(local_catalog(store, &identity, budget)?
                .map(|catalog| catalog.registrations)
                .unwrap_or_default())
        })?;
        for (key, registration) in registrations {
            if let Some(existing) = view.entries.get(&key) {
                ensure!(
                    existing.owner == Owner::Memory(registration.owner.clone()),
                    "catalog registered owner collision"
                );
                continue;
            }
            let generation = current_generation(&self.store)?;
            let (catalog, aggregate) = admissions(&self.store, &identity, &registration)?;
            SelectedAdmission::with_gates(&[&catalog, &aggregate], |store| {
                require_registration(
                    store,
                    &identity,
                    &registration,
                    &view.root_sha,
                    generation,
                    budget,
                )
            })?;
            return Ok(Some(CatalogPending {
                operation: Uuid::new_v5(
                    &registration.genesis_operation,
                    format!("shared-catalog-register:v1:{key}").as_bytes(),
                ),
                source_generation: generation,
                registration: Some(registration),
                portable: None,
                handwriting: None,
                genesis_slot: None,
                proposal: None,
            }));
        }
        if let Some(pending) =
            preferences::discover_registration(&self.store, &self.binding, view, budget)?
        {
            return Ok(Some(pending));
        }
        corrections::discover_registration(&self.store, &identity, view, budget)
    }
    /// One bounded step in the existing worker. A missing initial catalog is
    /// handled by the caller as Joining, never as permission to seed emptiness.
    pub(super) fn catalog_step(&mut self, view: &CatalogView, budget: &mut usize) -> Result<bool> {
        ensure!(
            view.complete && !self.checkpoint.catalog.is_empty(),
            "catalog registration requires verified bootstrap"
        );
        let mut local_budget = allowance();
        if self.checkpoint.catalog_pending.is_none() {
            let Some(pending) = self.discover_registration(view, &mut local_budget)? else {
                return Ok(false);
            };
            self.checkpoint.catalog_pending = Some(pending);
            self.save()?;
        }
        let mut pending = self
            .checkpoint
            .catalog_pending
            .clone()
            .context("catalog original absent")?;
        let identity = identity(&self.binding, view);
        validate_pending(&pending, &identity)?;
        let gates = source_admissions(&self.store, &identity, &pending)?;
        let handles: Vec<_> = gates.iter().collect();
        SelectedAdmission::with_gates(&handles, |store| {
            require_source(
                store,
                &identity,
                &pending,
                &view.root_sha,
                &mut local_budget,
            )
        })?;
        if let Some(proposal) = &pending.proposal {
            if let Some(winner) = self
                .checkpoint
                .catalog
                .iter()
                .find(|record| record.slot == proposal.slot)
            {
                let accepted: Catalog = serde_json::from_slice(&winner.bytes)?;
                if let Some(entry) = accepted.entries.get(&pending.key()?) {
                    ensure!(
                        entry.owner == pending.owner()?,
                        "catalog winning owner differs"
                    );
                    self.retire_catalog_proposal(pending.proposal.take().unwrap())?;
                    self.checkpoint.catalog_pending = None;
                    self.save()?;
                    return Ok(true);
                }
                // Only an additive catalog proposal is extended after a different
                // key wins. No domain manifest, original content or slot moves.
                self.retire_catalog_proposal(pending.proposal.take().unwrap())?;
                self.checkpoint.catalog_pending = Some(pending.clone());
                self.save()?;
            } else {
                ensure!(
                    proposal.slot == view.slot && proposal.base == view.base,
                    "catalog original slot left verified chain"
                );
            }
        }
        if let Some(entry) = view.entries.get(&pending.key()?) {
            ensure!(
                entry.owner == pending.owner()?,
                "catalog accepted owner differs"
            );
            self.checkpoint.catalog_pending = None;
            self.save()?;
            return Ok(true);
        }
        if pending.genesis_slot.is_none() {
            if *budget == 0 {
                return Ok(true);
            }
            *budget -= 1;
            pending.genesis_slot = Some(self.transport.allocate()?);
            self.checkpoint.catalog_pending = Some(pending.clone());
            self.save()?;
        }
        if pending.proposal.is_none() {
            if *budget == 0 {
                return Ok(true);
            }
            *budget -= 1;
            let next_slot = self.transport.allocate()?;
            let mut entries = view.entries.clone();
            entries.insert(
                pending.key()?,
                Entry {
                    owner: pending.owner()?,
                    genesis_slot: pending
                        .genesis_slot
                        .clone()
                        .context("catalog original genesis slot absent")?,
                },
            );
            let operation = Uuid::new_v5(&pending.operation, view.base.as_bytes());
            let proposed = Catalog {
                schema: WIRE,
                group: self.binding.group,
                root_sha256: view.root_sha.clone(),
                base_sha256: view.base.clone(),
                slot: view.slot.clone(),
                operation,
                entries,
                next_slot,
            };
            proposed.validate(
                &view.root,
                &view.root_sha,
                &view.slot,
                &view.base,
                &view.entries,
            )?;
            let bytes = serde_json::to_vec(&proposed)?;
            ensure!(bytes.len() <= 64 * 1024, "catalog descriptor byte bound");
            pending.proposal = Some(CatalogProposal {
                key: pending.key()?,
                slot: view.slot.clone(),
                base: view.base.clone(),
                bytes,
                descriptor: None,
            });
            self.checkpoint.catalog_pending = Some(pending.clone());
            self.save()?;
        }
        let mut proposal = pending.proposal.clone().unwrap();
        let proposed: Catalog = serde_json::from_slice(&proposal.bytes)?;
        proposed.validate(
            &view.root,
            &view.root_sha,
            &proposal.slot,
            &proposal.base,
            &view.entries,
        )?;
        let mut expected_entries = view.entries.clone();
        expected_entries.insert(
            pending.key()?,
            Entry {
                owner: pending.owner()?,
                genesis_slot: pending
                    .genesis_slot
                    .clone()
                    .context("catalog original genesis absent")?,
            },
        );
        ensure!(
            proposed.entries == expected_entries,
            "catalog candidate is not its original additive mapping"
        );
        if proposal.descriptor.is_none() {
            // The complete original body was durable before this upload could
            // send; upload() separately persists its pre-generated provider ID.
            let Some((id, reference)) = self.upload(proposal.bytes.clone(), budget)? else {
                return Ok(true);
            };
            proposal.descriptor = Some(Recorded {
                slot: proposal.slot.clone(),
                descriptor_id: id,
                sha256: reference.sha256,
                bytes: proposal.bytes.clone(),
            });
            pending.proposal = Some(proposal.clone());
            self.checkpoint.catalog_pending = Some(pending.clone());
            self.save()?;
        }
        if *budget == 0 {
            return Ok(true);
        }
        *budget -= 1;
        let recorded = proposal.descriptor.as_ref().unwrap();
        let prepared = self.transport.prepare_slot(
            self.slot(catalog_key(self.binding.group), proposal.slot.clone()),
            recorded.descriptor_id.clone(),
            ObjectRef {
                sha256: recorded.sha256.clone(),
                bytes: recorded.bytes.len() as u64,
            },
        )?;
        SelectedAdmission::with_gates(&handles, |store| {
            require_source(
                store,
                &identity,
                &pending,
                &view.root_sha,
                &mut local_budget,
            )?;
            // Mode lease belongs to step(), ordered domain gates span this real
            // create, and each validation above has released the Store mutex.
            self.transport.create_or_read_slot(&prepared).map(|_| ())
        })?;
        // refresh_root_catalog validates/adopts the exact accepted mapping on
        // the next step. An uncertain response retains this exact proposal.
        Ok(true)
    }
    fn retire_catalog_proposal(&mut self, proposal: CatalogProposal) -> Result<()> {
        ensure!(
            self.checkpoint.retired_catalog.len() < MAX_ITEMS,
            "catalog retired original bound"
        );
        self.checkpoint.retired_catalog.push(proposal);
        Ok(())
    }
}
fn validate_pending(pending: &CatalogPending, identity: &Identity) -> Result<()> {
    match (
        &pending.registration,
        &pending.portable,
        &pending.handwriting,
    ) {
        (Some(registration), None, None) => registration.validate(identity)?,
        (None, Some(portable), None) => ensure!(
            !portable.genesis_operation.is_nil() && valid_digest(&portable.genesis_sha256),
            "invalid portable catalog original"
        ),
        (None, None, Some(correction)) => {
            correction.owner.domain()?;
            ensure!(
                correction.owner.profile == identity.profile
                    && !correction.genesis_operation.is_nil()
                    && valid_digest(&correction.genesis_sha256),
                "invalid handwriting catalog original"
            );
        }
        _ => anyhow::bail!("catalog original source is ambiguous"),
    }
    ensure!(
        !pending.source_generation.is_nil()
            && pending.operation
                == Uuid::new_v5(
                    &pending.genesis_operation()?,
                    format!("shared-catalog-register:v1:{}", pending.key()?).as_bytes()
                ),
        "catalog original identity differs"
    );
    if let Some(id) = &pending.genesis_slot {
        drive::id(id)?;
    }
    if let Some(proposal) = &pending.proposal {
        ensure!(
            pending.genesis_slot.is_some() && proposal.key == pending.key()?,
            "catalog proposal precedes original genesis"
        );
        let body: Catalog = serde_json::from_slice(&proposal.bytes)?;
        ensure!(
            body.operation == Uuid::new_v5(&pending.operation, proposal.base.as_bytes())
                && body.entries.get(&proposal.key)
                    == Some(&Entry {
                        owner: pending.owner()?,
                        genesis_slot: pending.genesis_slot.clone().unwrap()
                    }),
            "catalog original mapping differs"
        );
    }
    Ok(())
}
pub(super) fn validate_checkpoint(checkpoint: &Checkpoint, binding: &Binding) -> Result<()> {
    if checkpoint.catalog_pending.is_none() && checkpoint.retired_catalog.is_empty() {
        return Ok(());
    }
    let root = checkpoint
        .root
        .as_ref()
        .context("catalog original has no root")?;
    root.check(&binding.root_id)?;
    let parsed: Root = serde_json::from_slice(&root.bytes)?;
    parsed.validate(binding)?;
    let identity = Identity {
        group: binding.group,
        profile: parsed.profile,
        binding_sha256: binding.digest.clone(),
    };
    if let Some(pending) = &checkpoint.catalog_pending {
        validate_pending(pending, &identity)?;
    }
    let mut ancestry = BTreeMap::new();
    let mut prior = BTreeMap::new();
    let mut base = root.sha256.clone();
    let mut slot = parsed.catalog_slot.clone();
    for accepted in &checkpoint.catalog {
        ensure!(
            ancestry
                .insert(slot.clone(), (base.clone(), prior.clone()))
                .is_none(),
            "catalog original ancestry cycle"
        );
        accepted.check(&slot)?;
        let body: Catalog = serde_json::from_slice(&accepted.bytes)?;
        body.validate(&parsed, &root.sha256, &slot, &base, &prior)?;
        prior = body.entries;
        slot = body.next_slot;
        base = accepted.sha256.clone();
    }
    ensure!(
        ancestry.insert(slot, (base, prior)).is_none(),
        "catalog original successor cycle"
    );
    for proposal in checkpoint.retired_catalog.iter().chain(
        checkpoint
            .catalog_pending
            .iter()
            .filter_map(|p| p.proposal.as_ref()),
    ) {
        drive::id(&proposal.slot)?;
        ensure!(
            valid_digest(&proposal.key)
                && valid_digest(&proposal.base)
                && !proposal.bytes.is_empty()
                && proposal.bytes.len() <= 64 * 1024,
            "catalog staged proposal bound"
        );
        let body: Catalog = serde_json::from_slice(&proposal.bytes)?;
        let (base, prior) = ancestry
            .get(&proposal.slot)
            .context("catalog proposal left original ancestry")?;
        ensure!(*base == proposal.base, "catalog proposal base changed");
        body.validate(&parsed, &root.sha256, &proposal.slot, &proposal.base, prior)?;
        ensure!(
            body.entries.contains_key(&proposal.key),
            "catalog proposal omits original key"
        );
        if let Some(recorded) = &proposal.descriptor {
            recorded.check(&proposal.slot)?;
            ensure!(
                recorded.bytes == proposal.bytes,
                "catalog staged descriptor differs"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

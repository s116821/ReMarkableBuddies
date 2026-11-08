//! Confirmed handwriting scopes in the existing shared catalog and Worker.
use super::*;
use crate::handwriting::{self, selected::Projection};
use crate::memory::catalog::Identity;
use crate::storage::selection::{SelectedReadBudget, SelectedReadSnapshot, SelectionMutation};
pub(super) mod command;
mod query;
pub(crate) use command::run;

pub(super) fn allowance() -> SelectedReadBudget {
    SelectedReadBudget::new(
        MAX_ITEMS as u64,
        MAX_ITEMS as u64,
        MAX_METADATA as u64,
        MAX_MEDIA,
    )
}

pub(super) fn catalog_gate(
    store: &Arc<Store>,
    scope: &SelectionScope,
    profile: Uuid,
) -> Result<SelectedAdmission> {
    let identity = Identity {
        group: scope.group,
        binding_sha256: scope.binding_sha256.clone(),
        profile,
    };
    SelectedAdmission::new(store.clone(), identity.scope()?)
}

pub(super) fn read(
    owner: HandwritingOwner,
    manifest: &Manifest,
    load: impl FnMut(&ObjectRef) -> Result<Vec<u8>>,
) -> Result<Projection> {
    Projection::read(owner.domain()?, manifest, load)
        .map_err(|error| anyhow::anyhow!("invalid handwriting closure: {error:?}"))
}

pub(super) fn project(
    store: &Store,
    owner: HandwritingOwner,
    manifest: &Manifest,
    budget: &mut SelectedReadBudget,
) -> Result<Projection> {
    let generation = catalog::current_generation(store)?;
    let objects = store
        .paths
        .data
        .join("generations")
        .join(generation.to_string())
        .join("objects");
    read(owner, manifest, |reference| {
        let path = objects.join(&reference.sha256);
        if manifest.media.contains(reference) {
            budget.charge(0, 1, 0, reference.bytes)?;
            return files::read(&path, reference.bytes);
        }
        budget.charge(0, 1, 0, 0)?;
        files::read_with_budget(&path, reference.bytes, Some(budget))
    })
}

fn original(
    store: &Store,
    scope: &SelectionScope,
    profile: Uuid,
    root: &str,
    budget: &mut SelectedReadBudget,
) -> Result<Option<HandwritingRegistration>> {
    let Some(snapshot) = store.selected_snapshot_with_budget(scope, MAX_ITEMS, budget)? else {
        return Ok(None);
    };
    original_from_read(store, &snapshot, profile, root, budget)
}

fn original_from_read(
    store: &Store,
    snapshot: &SelectedReadSnapshot,
    profile: Uuid,
    root: &str,
    budget: &mut SelectedReadBudget,
) -> Result<Option<HandwritingRegistration>> {
    let scope = snapshot.snapshot.token.scope();
    let genesis = snapshot
        .accepted_history
        .last()
        .context("correction genesis missing")?;
    let transaction = &genesis.transaction;
    if transaction.accepted_base_sha256 != genesis_base(root, &scope.key_sha256) {
        // An imported receive receipt is not a local catalog original.
        return Ok(None);
    }
    let [envelope] = genesis.selected_records.as_slice() else {
        anyhow::bail!("correction genesis is not a single original");
    };
    ensure!(
        transaction.mutation == SelectionMutation::Initialize
            && envelope.actor_id == store.actor_id
            && envelope.operation_id == transaction.operation
            && envelope.parents.is_empty(),
        "correction genesis lacks a local confirmation"
    );
    let value = handwriting::decode(envelope)
        .map_err(|error| anyhow::anyhow!("invalid correction genesis: {error:?}"))?
        .context("correction genesis is not a confirmation")?;
    let owner = HandwritingOwner::new(profile, value.correction);
    ensure!(
        value.profile == profile && owner.domain()?.key_sha256() == scope.key_sha256,
        "correction genesis owner differs"
    );
    project(store, owner, &transaction.selected, budget)?;
    Ok(Some(HandwritingRegistration {
        owner,
        genesis_operation: transaction.operation,
        genesis_sha256: genesis.sha256.clone(),
    }))
}

/// Verify the selected read against the accepted Worker receipt or the exact
/// locally authored genesis before projecting a current correction.
fn current_projection(
    store: &Store,
    owner: HandwritingOwner,
    read: &SelectedReadSnapshot,
    root: &str,
    progress: Option<&Progress>,
    budget: &mut SelectedReadBudget,
) -> Result<Projection> {
    let token = &read.snapshot.token;
    ensure!(
        token.scope().key_sha256 == owner.domain()?.key_sha256(),
        "correction scope differs"
    );
    if let Some(applied) = progress.and_then(|progress| progress.applied.as_ref()) {
        ensure!(
            token.accepted_base_sha256() == applied.descriptor,
            "correction applied base differs"
        );
        let receipt = read
            .accepted_history
            .iter()
            .find(|entry| entry.transaction.operation == applied.operation)
            .context("correction applied receipt absent")?;
        ensure!(
            matches!(
                receipt.transaction.mutation,
                SelectionMutation::Initialize | SelectionMutation::Activate
            ) && receipt.transaction.accepted_base_sha256 == applied.descriptor
                && receiving::membership(&receipt.transaction.selected)? == applied.membership,
            "correction applied receipt differs"
        );
    } else {
        let original = original_from_read(store, read, owner.profile, root, budget)?
            .context("correction local original absent")?;
        ensure!(
            original.owner == owner
                && token.accepted_base_sha256() == genesis_base(root, &token.scope().key_sha256),
            "correction local base or owner differs"
        );
    }
    project(store, owner, &read.snapshot.transaction.selected, budget)
}

pub(super) fn require_initial_publication(
    store: &Store,
    scope: &SelectionScope,
    owner: HandwritingOwner,
    root: &str,
    budget: &mut SelectedReadBudget,
) -> Result<()> {
    let genesis = original(store, scope, owner.profile, root, budget)?
        .context("correction local original absent")?;
    ensure!(genesis.owner == owner, "correction owner differs");
    Ok(())
}

pub(super) fn discover_registration(
    store: &Arc<Store>,
    identity: &Identity,
    view: &CatalogView,
    budget: &mut SelectedReadBudget,
) -> Result<Option<CatalogPending>> {
    let scopes = store.selected_scope_candidates(
        identity.group,
        &identity.binding_sha256,
        Namespace::Handwriting,
        budget,
    )?;
    for scope in scopes {
        let key = &scope.key_sha256;
        if let Some(entry) = view.entries.get(key) {
            ensure!(
                matches!(&entry.owner, Owner::Handwriting(_)),
                "correction catalog owner differs"
            );
            continue;
        }
        let catalog = catalog_gate(store, &scope, identity.profile)?;
        let correction = SelectedAdmission::new(store.clone(), scope.clone())?;
        let Some(pending) = SelectedAdmission::with_gates(&[&catalog, &correction], |store| {
            let Some(original) = original(store, &scope, identity.profile, &view.root_sha, budget)?
            else {
                return Ok(None);
            };
            Ok(Some(CatalogPending {
                operation: Uuid::new_v5(
                    &original.genesis_operation,
                    format!("shared-catalog-register:v1:{key}").as_bytes(),
                ),
                source_generation: catalog::current_generation(store)?,
                registration: None,
                portable: None,
                handwriting: Some(original),
                genesis_slot: None,
                proposal: None,
            }))
        })?
        else {
            continue;
        };
        return Ok(Some(pending));
    }
    Ok(None)
}

pub(super) fn require_registration(
    store: &Store,
    identity: &Identity,
    expected: &HandwritingRegistration,
    root: &str,
    budget: &mut SelectedReadBudget,
) -> Result<()> {
    let scope = expected
        .owner
        .domain()?
        .scope(identity.group, identity.binding_sha256.clone())
        .map_err(|error| anyhow::anyhow!("invalid correction scope: {error:?}"))?;
    let current = original(store, &scope, identity.profile, root, budget)?
        .context("correction registration source absent")?;
    ensure!(
        current.owner == expected.owner
            && current.genesis_operation == expected.genesis_operation
            && current.genesis_sha256 == expected.genesis_sha256,
        "correction registration original changed"
    );
    Ok(())
}

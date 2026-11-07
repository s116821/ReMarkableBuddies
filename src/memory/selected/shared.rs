//! Receiving-local index publication. Caller owns one ordered catalog/scope set.
use super::*;
use crate::storage::selection::{SelectionChange, SelectionScope};

pub(crate) fn register_received_locked(
    store: &Store,
    identity: &Identity,
    owner: &Owner,
    receive: Uuid,
    remote_base: &str,
) -> Result<Registration> {
    identity.validate()?;
    let scope = identity.aggregate_scope(owner)?;
    let mut allowance = budget();
    let local = store
        .selected_snapshot_with_budget(&scope, MAX_ITEMS, &mut allowance)?
        .context("received aggregate absent")?;
    let genesis = local
        .accepted_history
        .last()
        .context("received local genesis absent")?;
    let registration = Registration {
        format: 1,
        catalog: identity.record_id()?,
        key_sha256: scope.key_sha256.clone(),
        owner: owner.clone(),
        genesis_operation: genesis.transaction.operation,
        genesis_sha256: genesis.sha256.clone(),
    };
    registration.validate(identity)?;
    Aggregate::project(
        identity,
        &registration,
        &local.snapshot,
        &local.accepted_history,
    )?;
    let catalog_scope = identity.scope()?;
    if store
        .selected_snapshot(&catalog_scope, MAX_ITEMS)?
        .is_none()
    {
        let operation = Uuid::new_v5(&receive, b"shared-receive:local-catalog-initialize:v1");
        let root = Record::root(identity)?.envelope(identity, store.actor_id, operation)?;
        let bytes = serde_json::to_vec(&root)?;
        let hash = digest(&bytes);
        store.initialize_selected(
            &catalog_scope,
            SelectionChange {
                operation,
                accepted_base_sha256: remote_base.into(),
                selected: crate::storage::Manifest {
                    format: 1,
                    transaction_id: operation,
                    scope: crate::storage::Scope::SelectedRecords,
                    records: vec![ObjectRef {
                        sha256: hash.clone(),
                        bytes: bytes.len() as u64,
                    }],
                    record_namespaces: BTreeMap::from([(hash.clone(), Namespace::SubjectMemory)]),
                    media: vec![],
                    media_coverage: vec![],
                },
                retained: vec![],
            },
            BTreeMap::from([(hash, bytes)]),
        )?;
    }
    let catalog = store
        .selected_snapshot_with_budget(&catalog_scope, MAX_ITEMS, &mut allowance)?
        .context("local receiving catalog absent")?;
    let projection = Catalog::project(identity, &catalog.snapshot, &catalog.accepted_history)?;
    if let Some(existing) = projection.registrations.get(&scope.key_sha256) {
        ensure!(
            *existing == registration,
            "received registration conflicts with local genesis"
        );
        return Ok(registration);
    }
    let operation = Uuid::new_v5(&receive, b"shared-receive:local-catalog-register:v1");
    let envelope =
        Record::registration(registration.clone()).envelope(identity, store.actor_id, operation)?;
    store.commit_selected(
        &catalog.snapshot.token,
        operation,
        vec![envelope],
        BTreeMap::new(),
    )?;
    Ok(registration)
}

pub(crate) fn receiving_gates(
    store: &Arc<Store>,
    identity: &Identity,
    owner: &Owner,
) -> Result<(SelectedAdmission, SelectedAdmission)> {
    let aggregate: SelectionScope = identity.aggregate_scope(owner)?;
    Ok((
        SelectedAdmission::new(store.clone(), identity.scope()?)?,
        SelectedAdmission::new(store.clone(), aggregate)?,
    ))
}

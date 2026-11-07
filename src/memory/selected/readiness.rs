//! Product readiness derives from exact local catalog/genesis, never a worker file.
use super::*;
use crate::storage::{selection::SelectedSnapshot, Kind};
use std::collections::BTreeSet;

/// Discover candidate catalogs for this document, without treating fact profiles
/// or historical index entries as readiness. The caller rechecks discovery under
/// the ordered gates and validates a current catalog and exact local genesis.
pub(crate) fn mixed_identity(
    store: &Store,
    snapshot: &SelectedSnapshot,
) -> Result<Option<Vec<Identity>>> {
    let records = &snapshot.selected_records;
    if !records
        .iter()
        .any(|e| e.namespace == Namespace::SubjectMemory)
        || !records
            .iter()
            .any(|e| e.namespace == Namespace::Conversation)
    {
        return Ok(None);
    }

    let scope = snapshot.token.scope();
    let candidates =
        store.snapshot_revisions_matching(&[Namespace::SubjectMemory], MAX_ITEMS, |envelope| {
            if envelope.kind != Kind::Value {
                return false;
            }
            let payload = &envelope.payload;
            match payload["kind"].as_str() {
                Some("catalog-root") => {
                    payload["group"]
                        .as_str()
                        .and_then(|value| Uuid::parse_str(value).ok())
                        == Some(scope.group)
                        && payload["binding_sha256"].as_str() == Some(&scope.binding_sha256)
                }
                Some("aggregate-registration") => {
                    payload["key_sha256"].as_str() == Some(&scope.key_sha256)
                }
                _ => false,
            }
        })?;
    let mut registered = BTreeSet::new();
    let mut roots = BTreeMap::new();
    for envelope in candidates {
        let Ok(record) = serde_json::from_value::<Record>(envelope.payload) else {
            continue;
        };
        match record {
            Record::CatalogRoot {
                group,
                profile,
                binding_sha256,
                ..
            } => {
                let identity = Identity {
                    group,
                    profile,
                    binding_sha256,
                };
                if identity.record_id().ok() == Some(envelope.record_id) {
                    roots.insert(envelope.record_id, identity);
                }
            }
            Record::AggregateRegistration { catalog, .. } => {
                registered.insert(catalog);
            }
        }
    }
    Ok(Some(
        roots
            .into_iter()
            .filter_map(|(record, identity)| registered.contains(&record).then_some(identity))
            .collect(),
    ))
}

/// Caller owns the ordered catalog + document gates. No gate acquisition here.
pub(crate) fn require_mixed_ready(
    store: &Store,
    snapshot: &SelectedSnapshot,
    identities: &[Identity],
) -> Result<()> {
    let mut allowance = budget();
    let current = store
        .selected_snapshot_with_budget(snapshot.token.scope(), MAX_ITEMS, &mut allowance)?
        .context("mixed aggregate unavailable")?;
    ensure!(
        current.snapshot.token == snapshot.token,
        "mixed aggregate changed"
    );
    let genesis = current
        .accepted_history
        .last()
        .context("mixed aggregate genesis missing")?;
    for identity in identities {
        let Some(catalog) =
            store.selected_snapshot_with_budget(&identity.scope()?, MAX_ITEMS, &mut allowance)?
        else {
            continue;
        };
        let catalog = Catalog::project(identity, &catalog.snapshot, &catalog.accepted_history)?;
        let Some(registration) = catalog
            .registrations
            .get(&snapshot.token.scope().key_sha256)
        else {
            continue;
        };
        if !matches!(registration.owner, Owner::Document { .. })
            || registration.genesis_operation != genesis.transaction.operation
            || registration.genesis_sha256 != genesis.sha256
        {
            continue;
        }
        Aggregate::project(
            identity,
            registration,
            &current.snapshot,
            &current.accepted_history,
        )?;
        return Ok(());
    }
    anyhow::bail!("mixed aggregate is not receive-ready")
}

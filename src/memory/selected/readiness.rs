//! Product readiness derives from exact local catalog/genesis, never a worker file.
use super::*;
use crate::storage::{selection::SelectedSnapshot, Kind};

pub(crate) fn mixed_identity(snapshot: &SelectedSnapshot) -> Result<Option<Identity>> {
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
    let mut profile = None;
    for envelope in records
        .iter()
        .filter(|e| e.namespace == Namespace::SubjectMemory && e.kind == Kind::Value)
    {
        let fact: super::super::Fact = serde_json::from_value(envelope.payload.clone())?;
        fact.validate()?;
        ensure!(
            fact.key.record_id()? == envelope.record_id,
            "mixed fact identity differs"
        );
        if let Some(prior) = profile {
            ensure!(prior == fact.key.profile, "mixed fact profiles differ");
        }
        profile = Some(fact.key.profile);
    }
    let scope = snapshot.token.scope();
    let identity = Identity {
        group: scope.group,
        profile: profile.context("mixed fact profile unavailable")?,
        binding_sha256: scope.binding_sha256.clone(),
    };
    identity.validate()?;
    Ok(Some(identity))
}

/// Caller owns the ordered catalog + document gates. No gate acquisition here.
pub(crate) fn require_mixed_ready(
    store: &Store,
    snapshot: &SelectedSnapshot,
    identity: &Identity,
) -> Result<()> {
    ensure!(
        mixed_identity(snapshot)?.as_ref() == Some(identity),
        "mixed identity changed"
    );
    let mut allowance = budget();
    let catalog = store
        .selected_snapshot_with_budget(&identity.scope()?, MAX_ITEMS, &mut allowance)?
        .context("mixed aggregate catalog unavailable")?;
    let catalog = Catalog::project(identity, &catalog.snapshot, &catalog.accepted_history)?;
    let registration = catalog
        .registrations
        .get(&snapshot.token.scope().key_sha256)
        .context("mixed aggregate is not receive-ready")?;
    ensure!(
        matches!(registration.owner, Owner::Document { .. }),
        "mixed owner is not a document"
    );
    let current = store
        .selected_snapshot_with_budget(snapshot.token.scope(), MAX_ITEMS, &mut allowance)?
        .context("mixed aggregate unavailable")?;
    ensure!(
        current.snapshot.token == snapshot.token,
        "mixed aggregate changed"
    );
    Aggregate::project(
        identity,
        registration,
        &current.snapshot,
        &current.accepted_history,
    )?;
    Ok(())
}

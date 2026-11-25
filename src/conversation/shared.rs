//! Domain-owned winner validation/atomic retention. No native effect permission.
use super::*;
use crate::memory::catalog::{Identity, Owner};
use crate::storage::{selection::*, Manifest};

pub(crate) struct SharedWinner {
    scope: SelectionScope,
    owner: Owner,
    profile: Uuid,
    remote: String,
    manifest: Manifest,
    objects: BTreeMap<String, Vec<u8>>,
}
pub(crate) enum SharedAdoption {
    Applied(Box<SelectionTransaction>),
    CommittedNotApplied,
}
pub(crate) fn shared_owner(snapshot: &SelectedSnapshot, profile: Uuid) -> Result<Owner> {
    let scope = snapshot.token.scope();
    let domain = snapshot
        .selected_records
        .iter()
        .any(|e| e.namespace == Namespace::Conversation);
    let owner = if domain {
        let projection = SelectedDomainProjection::from_snapshot(snapshot)?;
        let owners = projection.document_ownership()?;
        let Some(DocumentOwnership::Document(document)) = owners.values().next() else {
            bail!("shared document ownership unavailable")
        };
        ensure!(
            owners
                .values()
                .all(|o| *o == DocumentOwnership::Document(*document)),
            "shared document ownership ambiguous"
        );
        Owner::Document {
            document: *document,
        }
    } else {
        let first = snapshot
            .selected_records
            .iter()
            .find(|e| e.namespace == Namespace::SubjectMemory && e.kind == Kind::Value)
            .context("shared global fact absent")?;
        let fact: crate::memory::Fact = serde_json::from_value(first.payload.clone())?;
        fact.validate()?;
        match fact.key.scope {
            crate::memory::Scope::Global => {
                ensure!(fact.key.profile == profile, "shared global profile differs");
                crate::memory::aggregate::validate_global_partition(
                    &snapshot.selected_records,
                    &fact.key,
                )?;
                Owner::GlobalFact { key: fact.key }
            }
            crate::memory::Scope::Document { document } => {
                crate::memory::aggregate::validate_document_partition(
                    &snapshot.selected_records,
                    document,
                )?;
                Owner::Document { document }
            }
        }
    };
    ensure!(
        owner.key_sha256()? == scope.key_sha256,
        "shared aggregate key differs"
    );
    Ok(owner)
}
impl SharedWinner {
    /// Bytes are staged/verified with storage's actual exact closure and notebook
    /// semantic checks, then domain ownership is checked before minting this value.
    pub(crate) fn validate(
        store: &Store,
        scope: SelectionScope,
        owner: Owner,
        profile: Uuid,
        remote: String,
        manifest: Manifest,
        objects: BTreeMap<String, Vec<u8>>,
    ) -> Result<Self> {
        scope.validate()?;
        ensure!(
            owner.key_sha256()? == scope.key_sha256 && crate::storage::valid_digest(&remote),
            "shared winner identity differs"
        );
        for reference in &manifest.records {
            let bytes = objects
                .get(&reference.sha256)
                .context("winning raw record absent")?;
            // Validate the enclosing original bytes before Value can collapse
            // duplicate payload fields. ObjectRef identity still names these bytes.
            crate::config::inspection::strict_json(bytes)?;
        }
        let records = store.stage_selected_candidate(&manifest, &objects)?;
        match &owner {
            Owner::Document { document } => {
                if records
                    .iter()
                    .all(|record| record.namespace == Namespace::SubjectMemory)
                {
                    // A document fact can precede its first conversation. The
                    // Store already verified its complete causal closure; this
                    // branch grants memory custody only, no conversation/native
                    // projection or effect permission.
                    crate::memory::aggregate::validate_document_partition(&records, *document)?;
                } else {
                    let projection = SelectedDomainProjection::from_mixed_records(
                        &records,
                        &manifest.records,
                        &scope,
                    )?;
                    let owners = projection.document_ownership()?;
                    ensure!(
                        !owners.is_empty()
                            && owners
                                .values()
                                .all(|o| *o == DocumentOwnership::Document(*document)),
                        "winning document partition differs"
                    );
                    // A document key is profile-independent. The mixed projector
                    // validates every fact's document and immutable identity; foreign
                    // profiles remain exact custody and do not require local catalogs.
                }
            }
            Owner::GlobalFact { key } => {
                ensure!(key.profile == profile, "winning global profile differs");
                crate::memory::aggregate::validate_global_partition(&records, key)?;
            }
        }
        Ok(Self {
            scope,
            owner,
            profile,
            remote,
            manifest,
            objects,
        })
    }
    pub(crate) fn apply(
        self,
        store: &Arc<Store>,
        operation: Uuid,
        expected: Option<&SelectionToken>,
    ) -> Result<SharedAdoption> {
        let identity = Identity {
            group: self.scope.group,
            profile: self.profile,
            binding_sha256: self.scope.binding_sha256.clone(),
        };
        let (catalog, aggregate) =
            crate::memory::selected::receiving_gates(store, &identity, &self.owner)?;
        let _mode = aggregate.reader_mode_lease()?;
        SelectedAdmission::with_gates(&[&catalog, &aggregate], |store| {
            // Recover the exact receiving receipt before any conditional retry.
            if let Some(receipt) = store.selected_receipt(&self.scope, operation)? {
                ensure!(
                    matches!(
                        receipt.mutation,
                        SelectionMutation::Initialize | SelectionMutation::Activate
                    ) && receipt.accepted_base_sha256 == self.remote
                        && same_membership(&receipt.selected, &self.manifest),
                    "receiving operation collision"
                );
                match expected {
                    None => ensure!(
                        receipt.mutation == SelectionMutation::Initialize,
                        "receiving initialization replay differs"
                    ),
                    Some(expected) => ensure!(
                        store
                            .historical_predecessor_token(&self.scope, operation)?
                            .as_ref()
                            == Some(expected),
                        "receiving original predecessor differs"
                    ),
                }
                crate::memory::selected::register_received_locked(
                    store,
                    &identity,
                    &self.owner,
                    operation,
                    &self.remote,
                )?;
                return Ok(SharedAdoption::Applied(Box::new(receipt)));
            }
            let current = store.selected_snapshot(&self.scope, MAX_ITEMS)?;
            match (&current, expected) {
                (None, None) => {}
                (Some(snapshot), Some(pin)) => {
                    ensure!(snapshot.token == *pin, "receiving local selection changed")
                }
                _ => bail!("receiving absence or current pin changed"),
            }
            let mut retained = Vec::new();
            if let Some(snapshot) = &current {
                let mut originals = BTreeSet::new();
                for envelope in snapshot
                    .selected_records
                    .iter()
                    .chain(&snapshot.retained_records)
                {
                    if envelope.namespace != Namespace::Conversation || envelope.kind != Kind::Value
                    {
                        continue;
                    }
                    if let Record::Receipt(receipt) = Ledger::decode(envelope)? {
                        if receipt.admitted_intent.is_some() && envelope.actor_id == store.actor_id
                        {
                            originals.insert(receipt.acknowledgment.operation);
                        }
                    }
                }
                for original in originals {
                    let original = admission::recover_original(store, &self.scope, original)?
                        .context("local admitted original lineage unavailable")?;
                    let pin = &original.receipt_reference.object;
                    if snapshot.transaction.selected.records.contains(pin) {
                        // Active intent settlement cannot use retained-only proof.
                        if self.manifest.records.contains(pin) {
                            return Ok(SharedAdoption::CommittedNotApplied);
                        }
                    } else if settlement::retained_uncertainty_in_store(store, snapshot, &original)?
                        == IntentUncertainty::Unresolved
                        && self.manifest.records.contains(pin)
                    {
                        return Ok(SharedAdoption::CommittedNotApplied);
                    }
                }
                retained.extend(snapshot.transaction.retained.clone());
                // A losing explicit fact or draft needs custody even without
                // a model/native receipt. Retention grants inspection only.
                retained.push(snapshot.transaction.selected.clone());
                retained.retain(|prior| {
                    prior
                        .records
                        .iter()
                        .any(|r| !self.manifest.records.contains(r))
                        || prior.media.iter().any(|r| !self.manifest.media.contains(r))
                        || prior
                            .media_coverage
                            .iter()
                            .any(|c| !self.manifest.media_coverage.contains(c))
                });
                let mut seen = BTreeSet::new();
                let mut unique = Vec::new();
                for manifest in retained {
                    if seen.insert(digest(&serde_json::to_vec(&manifest)?)) {
                        unique.push(manifest);
                    }
                }
                retained = unique;
            }
            let change = SelectionChange {
                operation,
                accepted_base_sha256: self.remote.clone(),
                selected: self.manifest.clone(),
                retained,
            };
            let publication = match expected {
                Some(expected) => store.activate_selected(expected, change, self.objects)?,
                None => store.initialize_selected(&self.scope, change, self.objects)?,
            };
            crate::memory::selected::register_received_locked(
                store,
                &identity,
                &self.owner,
                operation,
                &self.remote,
            )?;
            Ok(SharedAdoption::Applied(Box::new(publication.transaction)))
        })
    }
}
pub(crate) fn same_membership(a: &Manifest, b: &Manifest) -> bool {
    a.format == b.format
        && a.scope == b.scope
        && a.records == b.records
        && a.record_namespaces == b.record_namespaces
        && a.media == b.media
        && a.media_coverage == b.media_coverage
}

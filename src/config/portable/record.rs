//! Portable declaration records and original conditions, not live admission.
use super::selected::Artifact;
use crate::storage::{
    digest, valid_digest, Envelope, Kind, Manifest, Namespace, ObjectRef, Uuid, MAX_ITEMS,
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const NAME: &str = "urn:remarkable-buddies:global-record:v1:portable-configuration";
pub fn key_sha256() -> String {
    digest(NAME.as_bytes())
}
pub fn record_id(group: Uuid) -> Result<Uuid> {
    ensure!(!group.is_nil(), "portable group absent");
    Ok(Uuid::new_v5(&group, NAME.as_bytes()))
}
/// Serializable conditions are operator preconditions only. A current qualified
/// source must recheck them before any mutation; deserialization grants nothing.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub group: Uuid,
    pub mode_epoch: Uuid,
    pub binding_sha256: String,
    pub store_generation: Uuid,
    pub local_config_sha256: String,
    pub selection: SelectionCondition,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SelectionCondition {
    Absent {},
    Selected {
        aggregate_generation: Uuid,
        accepted_base_sha256: String,
        selection_sha256: String,
        revision: Uuid,
    },
}
impl Condition {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.group.is_nil()
                && !self.mode_epoch.is_nil()
                && !self.store_generation.is_nil()
                && valid_digest(&self.binding_sha256)
                && valid_digest(&self.local_config_sha256),
            "invalid portable original condition"
        );
        if let SelectionCondition::Selected {
            aggregate_generation,
            accepted_base_sha256,
            selection_sha256,
            revision,
        } = &self.selection
        {
            ensure!(
                !aggregate_generation.is_nil()
                    && !revision.is_nil()
                    && valid_digest(accepted_base_sha256)
                    && valid_digest(selection_sha256),
                "invalid portable selected condition"
            );
        }
        Ok(())
    }
}
/// No Debug/Serialize: the artifact can include private personalization text.
pub struct Projection {
    head: Envelope,
    artifact: Artifact,
}
impl Projection {
    pub fn artifact(&self) -> &Artifact {
        &self.artifact
    }
    pub fn revision(&self) -> Uuid {
        self.head.revision_id
    }
    pub fn operation(&self) -> Uuid {
        self.head.operation_id
    }
}
fn decode(group: Uuid, reference: &ObjectRef, bytes: &[u8]) -> Result<Envelope> {
    ensure!(
        bytes.len() as u64 == reference.bytes && digest(bytes) == reference.sha256,
        "portable original object differs"
    );
    crate::config::inspection::strict_json(bytes)?;
    let envelope: Envelope = serde_json::from_slice(bytes)?;
    envelope.validate()?;
    ensure!(
        envelope.namespace == Namespace::PortableConfiguration
            && envelope.record_id == record_id(group)?
            && envelope.parents.len() <= 1,
        "portable record identity differs"
    );
    Artifact::from_value(envelope.payload.clone())
        .map_err(|_| anyhow::anyhow!("invalid portable declaration"))?;
    Ok(envelope)
}
/// Read one exact selected closure. Caller supplies Store-verified original
/// bytes; hashes are rechecked and no all-history/default-value fallback exists.
pub fn project(
    scope: &crate::storage::selection::SelectionScope,
    manifest: &Manifest,
    mut load: impl FnMut(&ObjectRef) -> Result<Vec<u8>>,
) -> Result<Projection> {
    scope.validate()?;
    ensure!(
        scope.key_sha256 == key_sha256(),
        "portable scope key differs"
    );
    manifest.validate()?;
    ensure!(
        manifest.scope == crate::storage::Scope::SelectedRecords
            && manifest.records.len() <= MAX_ITEMS
            && manifest.media.is_empty()
            && manifest.media_coverage.is_empty(),
        "portable manifest media unsupported"
    );
    let mut records = BTreeMap::new();
    let mut operations = BTreeSet::new();
    let mut children = BTreeSet::new();
    for reference in &manifest.records {
        ensure!(
            manifest.record_namespaces.get(&reference.sha256)
                == Some(&Namespace::PortableConfiguration),
            "portable manifest namespace differs"
        );
        let record = decode(scope.group, reference, &load(reference)?)?;
        ensure!(
            operations.insert(record.operation_id)
                && records.insert(record.revision_id, record).is_none(),
            "portable original operation or revision repeated"
        );
    }
    let mut roots = 0usize;
    for record in records.values() {
        if let Some(parent) = record.parents.first() {
            ensure!(
                records.contains_key(parent) && children.insert(*parent),
                "portable parent missing or forked"
            );
        } else {
            roots += 1;
        }
    }
    ensure!(roots == 1, "portable chain has no unique genesis");
    let heads: Vec<_> = records
        .keys()
        .filter(|revision| !children.contains(revision))
        .copied()
        .collect();
    ensure!(heads.len() == 1, "portable head is conflicted");
    let mut next = Some(heads[0]);
    let mut seen = BTreeSet::new();
    while let Some(revision) = next {
        ensure!(seen.insert(revision), "portable causal cycle");
        next = records[&revision].parents.first().copied();
    }
    ensure!(seen.len() == records.len(), "portable disconnected history");
    let head = records.remove(&heads[0]).context("portable head absent")?;
    let artifact = Artifact::from_value(head.payload.clone())
        .map_err(|_| anyhow::anyhow!("invalid portable head"))?;
    Ok(Projection { head, artifact })
}
/// Pure preparation only. Current selection/qualified absence, raw local Config
/// and mode must be admitted by the real producer before this envelope is used.
/// Equal declaration artifacts return None; effective fallback equality is not
/// a no-op. Nothing writes local Config or creates a publication receipt here.
pub fn prepare(
    actor: Uuid,
    operation: Uuid,
    original: &Condition,
    previous: Option<&Projection>,
    patch: &Artifact,
) -> Result<Option<Envelope>> {
    original.validate()?;
    ensure!(
        !actor.is_nil() && !operation.is_nil(),
        "invalid portable edit identity"
    );
    ensure!(
        match (&original.selection, previous) {
            (SelectionCondition::Absent {}, None) => true,
            (SelectionCondition::Selected { revision, .. }, Some(previous)) =>
                *revision == previous.revision(),
            _ => false,
        },
        "portable original selection differs"
    );
    if let Some(previous) = previous {
        ensure!(
            previous.head.record_id == record_id(original.group)?,
            "portable prior group differs"
        );
    }
    let empty = Artifact::empty();
    let prior = previous.map(|p| p.artifact()).unwrap_or(&empty);
    let result = prior
        .overlay(patch)
        .map_err(|_| anyhow::anyhow!("invalid portable sparse declaration"))?;
    if result == *prior {
        return Ok(None);
    }
    let record = record_id(original.group)?;
    let name = serde_json::to_vec(&(
        "remarkable-buddies:portable-preference-edit:v1",
        operation,
        original,
        patch.sha256(),
        result.sha256(),
    ))?;
    let envelope = Envelope {
        envelope_version: 1,
        namespace: Namespace::PortableConfiguration,
        domain_schema_version: 1,
        record_id: record,
        revision_id: Uuid::new_v5(&record, &name),
        parents: previous
            .map(|p| BTreeSet::from([p.revision()]))
            .unwrap_or_default(),
        operation_id: operation,
        actor_id: actor,
        kind: Kind::Value,
        payload: result.value().clone(),
        media_descriptors: vec![],
    };
    envelope.validate()?;
    Ok(Some(envelope))
}

#[cfg(test)]
mod tests;

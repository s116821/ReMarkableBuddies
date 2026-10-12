//! Typed local selected membership. Provider slots and saved IDs are not authority.
use super::{Key, Scope};
use crate::storage::{
    digest,
    selection::{AcceptedSelectionTransaction, SelectedSnapshot, SelectionScope},
    valid_digest, Envelope, Kind, Namespace, ObjectRef, Uuid,
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_AGGREGATES: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub group: Uuid,
    pub profile: Uuid,
    pub binding_sha256: String,
}
impl Identity {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.group.is_nil() && !self.profile.is_nil() && valid_digest(&self.binding_sha256),
            "invalid memory catalog identity"
        );
        Ok(())
    }
    fn bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        Ok(format!(
            "urn:remarkable-buddies:subject-memory-catalog:v1:{}",
            serde_json::to_string(self)?
        )
        .into_bytes())
    }
    pub fn record_id(&self) -> Result<Uuid> {
        Ok(Uuid::new_v5(&Uuid::NAMESPACE_URL, &self.bytes()?))
    }
    pub fn scope(&self) -> Result<SelectionScope> {
        Ok(SelectionScope {
            group: self.group,
            key_sha256: digest(&self.bytes()?),
            binding_sha256: self.binding_sha256.clone(),
        })
    }
    pub fn aggregate_scope(&self, owner: &Owner) -> Result<SelectionScope> {
        self.validate()?;
        owner.validate(self.profile)?;
        Ok(SelectionScope {
            group: self.group,
            key_sha256: owner.key_sha256()?,
            binding_sha256: self.binding_sha256.clone(),
        })
    }
}

/// The document recipe is shared with conversation membership, independent of profile.
pub fn document_key(document: Uuid) -> Result<String> {
    ensure!(!document.is_nil(), "invalid aggregate document");
    Ok(digest(
        format!("urn:remarkable-buddies:source-document:v1:{document}").as_bytes(),
    ))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Owner {
    Document { document: Uuid },
    GlobalFact { key: Key },
}
impl Owner {
    fn validate(&self, profile: Uuid) -> Result<()> {
        match self {
            Self::Document { document } => {
                document_key(*document)?;
            }
            Self::GlobalFact { key } => {
                key.validate()?;
                ensure!(
                    key.profile == profile && key.scope == Scope::Global,
                    "foreign global owner"
                );
            }
        }
        Ok(())
    }
    pub fn key_sha256(&self) -> Result<String> {
        match self {
            Self::Document { document } => document_key(*document),
            Self::GlobalFact { key } => {
                key.validate()?;
                ensure!(
                    key.scope == Scope::Global,
                    "global aggregate requires global fact"
                );
                Ok(digest(
                    format!(
                        "urn:remarkable-buddies:global-record:v1:subject-memory:{}",
                        key.record_id()?
                    )
                    .as_bytes(),
                ))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub format: u32,
    pub catalog: Uuid,
    pub key_sha256: String,
    pub owner: Owner,
    pub genesis_operation: Uuid,
    pub genesis_sha256: String,
}
impl Registration {
    pub fn record_id(&self) -> Result<Uuid> {
        ensure!(
            !self.catalog.is_nil() && valid_digest(&self.key_sha256),
            "invalid registration identity"
        );
        Ok(Uuid::new_v5(
            &self.catalog,
            format!("aggregate:{}", self.key_sha256).as_bytes(),
        ))
    }
    pub(crate) fn validate(&self, identity: &Identity) -> Result<()> {
        ensure!(
            self.format == 1
                && self.catalog == identity.record_id()?
                && !self.genesis_operation.is_nil()
                && valid_digest(&self.genesis_sha256),
            "invalid memory registration"
        );
        self.owner.validate(identity.profile)?;
        ensure!(
            self.key_sha256 == self.owner.key_sha256()?,
            "registration aggregate mismatch"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Record {
    CatalogRoot {
        format: u32,
        group: Uuid,
        profile: Uuid,
        binding_sha256: String,
    },
    AggregateRegistration {
        format: u32,
        catalog: Uuid,
        key_sha256: String,
        owner: Owner,
        genesis_operation: Uuid,
        genesis_sha256: String,
    },
}
impl Record {
    pub fn root(identity: &Identity) -> Result<Self> {
        identity.validate()?;
        Ok(Self::CatalogRoot {
            format: 1,
            group: identity.group,
            profile: identity.profile,
            binding_sha256: identity.binding_sha256.clone(),
        })
    }
    pub fn registration(registration: Registration) -> Self {
        Self::AggregateRegistration {
            format: registration.format,
            catalog: registration.catalog,
            key_sha256: registration.key_sha256,
            owner: registration.owner,
            genesis_operation: registration.genesis_operation,
            genesis_sha256: registration.genesis_sha256,
        }
    }
    pub fn envelope(&self, identity: &Identity, actor: Uuid, operation: Uuid) -> Result<Envelope> {
        let record_id = self.identity(identity)?.0;
        let envelope = Envelope {
            envelope_version: 1,
            namespace: Namespace::SubjectMemory,
            domain_schema_version: 1,
            record_id,
            revision_id: Uuid::new_v5(&operation, b"remarkable-buddies:subject-memory:revision:v1"),
            parents: Default::default(),
            operation_id: operation,
            actor_id: actor,
            kind: Kind::Value,
            payload: serde_json::to_value(self)?,
            media_descriptors: vec![],
        };
        envelope.validate()?;
        Ok(envelope)
    }
    fn identity(&self, identity: &Identity) -> Result<(Uuid, Option<Registration>)> {
        identity.validate()?;
        match self {
            Self::CatalogRoot {
                format,
                group,
                profile,
                binding_sha256,
            } => {
                ensure!(
                    *format == 1
                        && *group == identity.group
                        && *profile == identity.profile
                        && binding_sha256 == &identity.binding_sha256,
                    "foreign catalog root"
                );
                Ok((identity.record_id()?, None))
            }
            Self::AggregateRegistration {
                format,
                catalog,
                key_sha256,
                owner,
                genesis_operation,
                genesis_sha256,
            } => {
                let registration = Registration {
                    format: *format,
                    catalog: *catalog,
                    key_sha256: key_sha256.clone(),
                    owner: owner.clone(),
                    genesis_operation: *genesis_operation,
                    genesis_sha256: genesis_sha256.clone(),
                };
                registration.validate(identity)?;
                Ok((registration.record_id()?, Some(registration)))
            }
        }
    }
}

pub(crate) struct Catalog {
    pub registrations: BTreeMap<String, Registration>,
}
impl Catalog {
    /// `history` must be the complete already-validated, cumulatively charged lineage.
    pub fn project(
        identity: &Identity,
        snapshot: &SelectedSnapshot,
        history: &[AcceptedSelectionTransaction],
    ) -> Result<Self> {
        let scope = identity.scope()?;
        ensure!(
            snapshot.token.scope() == &scope && snapshot.transaction.scope == scope,
            "foreign catalog selection"
        );
        ensure!(
            snapshot.selected_records.len() == snapshot.transaction.selected.records.len(),
            "incomplete catalog references"
        );
        ensure!(
            snapshot.transaction.selected.media.is_empty()
                && snapshot.transaction.selected.media_coverage.is_empty(),
            "catalog media unsupported"
        );
        ensure!(
            !history.is_empty()
                && history[0].transaction.operation == snapshot.transaction.operation,
            "catalog lineage absent"
        );
        let current: BTreeMap<&str, &ObjectRef> = snapshot
            .transaction
            .selected
            .records
            .iter()
            .map(|reference| (reference.sha256.as_str(), reference))
            .collect();
        for predecessor in history {
            let predecessor = &predecessor.transaction;
            ensure!(
                predecessor.scope == scope
                    && predecessor.selected.records.iter().all(|reference| current
                        .get(reference.sha256.as_str())
                        .is_some_and(|exact| **exact == *reference)),
                "catalog accepted membership removed or replaced"
            );
        }
        let mut root = None;
        let mut registrations = BTreeMap::new();
        for (envelope, reference) in snapshot
            .selected_records
            .iter()
            .zip(&snapshot.transaction.selected.records)
        {
            envelope.validate()?;
            ensure!(
                envelope.namespace == Namespace::SubjectMemory
                    && envelope.domain_schema_version == 1
                    && envelope.kind == Kind::Value
                    && envelope.parents.is_empty()
                    && envelope.media_descriptors.is_empty(),
                "unsupported catalog envelope"
            );
            ensure!(
                envelope.revision_id
                    == Uuid::new_v5(
                        &envelope.operation_id,
                        b"remarkable-buddies:subject-memory:revision:v1"
                    ),
                "catalog revision identity mismatch"
            );
            let record: Record = serde_json::from_value(envelope.payload.clone())
                .context("unsupported catalog payload")?;
            let (id, registration) = record.identity(identity)?;
            ensure!(
                id == envelope.record_id,
                "catalog logical identity mismatch"
            );
            if let Some(registration) = registration {
                ensure!(
                    registrations
                        .insert(registration.key_sha256.clone(), registration)
                        .is_none(),
                    "duplicate catalog registration"
                );
            } else {
                ensure!(root.replace(reference).is_none(), "duplicate catalog root");
            }
        }
        ensure!(
            root.is_some() && registrations.len() <= MAX_AGGREGATES,
            "missing root or excessive aggregate count"
        );
        Ok(Self { registrations })
    }
}

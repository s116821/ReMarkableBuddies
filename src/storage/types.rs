//! Policy-neutral identities and generic envelopes. Domain payloads remain opaque.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
pub use uuid::Uuid;

pub const FORMAT: u32 = 1;
pub const MAX_RECORD: usize = 1024 * 1024;
pub const MAX_METADATA: usize = 8 * MAX_RECORD;
pub const MAX_ITEMS: usize = 4096;
pub const MAX_MEDIA: u64 = 256 * 1024 * 1024;

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum Namespace {
    Conversation,
    Source,
    ExportAssociation,
    SubjectMemory,
    Handwriting,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Media {
    pub sha256: String,
    pub bytes: u64,
    pub media_type: String,
}
impl Media {
    pub fn validate(&self) -> Result<()> {
        ensure!(valid_digest(&self.sha256), "invalid media digest");
        ensure!(self.bytes <= MAX_MEDIA, "media exceeds supported bound");
        ensure!(
            !self.media_type.is_empty() && self.media_type.len() <= 128,
            "invalid media type"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Value,
    Tombstone,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub envelope_version: u32,
    pub namespace: Namespace,
    pub domain_schema_version: u32,
    pub record_id: Uuid,
    pub revision_id: Uuid,
    pub parents: BTreeSet<Uuid>,
    pub operation_id: Uuid,
    pub actor_id: Uuid,
    pub kind: Kind,
    pub payload: Value,
    pub media_descriptors: Vec<Media>,
}
impl Envelope {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.envelope_version == FORMAT,
            "unsupported envelope version"
        );
        ensure!(
            self.domain_schema_version == 1,
            "unsupported domain schema version"
        );
        ensure!(
            ![
                self.record_id,
                self.revision_id,
                self.operation_id,
                self.actor_id
            ]
            .iter()
            .any(Uuid::is_nil),
            "nil identity"
        );
        ensure!(
            self.parents.len() <= MAX_ITEMS && !self.parents.contains(&self.revision_id),
            "invalid parents"
        );
        ensure!(
            self.media_descriptors.len() <= MAX_ITEMS,
            "too many media descriptors"
        );
        let mut seen = BTreeSet::new();
        for media in &self.media_descriptors {
            media.validate()?;
            ensure!(seen.insert(&media.sha256), "duplicate media descriptor");
        }
        ensure!(
            serde_json::to_vec(self)?.len() <= MAX_RECORD,
            "record exceeds bound"
        );
        if self.kind == Kind::Tombstone {
            ensure!(
                self.payload.is_null() && self.media_descriptors.is_empty(),
                "tombstone must not carry content"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Omission {
    PolicyDisabled,
    SizeDeferred,
    UnavailableAtSource,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "availability", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Coverage {
    Included { sha256: String },
    Omitted { sha256: String, reason: Omission },
}
impl Coverage {
    pub fn hash(&self) -> &str {
        match self {
            Self::Included { sha256 } | Self::Omitted { sha256, .. } => sha256,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ObjectRef {
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    LocalTransaction,
    SelectedRecords,
    SupplementalMedia,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: u32,
    pub transaction_id: Uuid,
    pub scope: Scope,
    pub records: Vec<ObjectRef>,
    pub media: Vec<ObjectRef>,
    pub media_coverage: Vec<Coverage>,
}
impl Manifest {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.format == FORMAT && !self.transaction_id.is_nil(),
            "unsupported manifest"
        );
        ensure!(
            !self.records.is_empty() && self.records.len() + self.media.len() <= MAX_ITEMS,
            "invalid manifest bounds"
        );
        ensure!(
            self.media_coverage.len() <= MAX_ITEMS,
            "coverage exceeds bound"
        );
        let mut seen = BTreeSet::new();
        for item in self.records.iter().chain(&self.media) {
            ensure!(
                valid_digest(&item.sha256) && item.bytes <= MAX_MEDIA,
                "invalid object reference"
            );
            ensure!(seen.insert(&item.sha256), "duplicate required object");
        }
        let coverage: BTreeSet<_> = self.media_coverage.iter().map(Coverage::hash).collect();
        ensure!(
            coverage.len() == self.media_coverage.len() && coverage.iter().all(|h| valid_digest(h)),
            "invalid coverage"
        );
        for item in &self.media_coverage {
            let required = self.media.iter().any(|m| m.sha256 == item.hash());
            ensure!(
                required == matches!(item, Coverage::Included { .. }),
                "coverage contradicts required set"
            );
        }
        ensure!(
            self.media
                .iter()
                .all(|m| coverage.contains(m.sha256.as_str())),
            "missing media coverage"
        );
        Ok(())
    }
}

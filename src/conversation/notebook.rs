//! Notebook archive and processing facts; never native intent or source authority.
use super::*;
use crate::{
    retrieval::SourceFormat,
    storage::valid_digest,
    writer::notebook::{ValidatedNotebook, MAX_CANONICAL_BYTES, MEDIA_TYPE},
};
use serde::{Deserialize, Serialize};

pub const NOTEBOOK_SOURCE_SCHEMA: u32 = 3;
pub const NOTEBOOK_PROCESSING_SCHEMA: u32 = 3;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArchivedNormalizedSource {
    pub id: Uuid,
    pub conversation: Uuid,
    /// Original user turn, also when later refinements reference this source.
    pub turn: Uuid,
    pub document: Uuid,
    pub format: SourceFormat,
    pub source_sha256: String,
    pub extractor_revision: String,
    pub units_sha256: String,
    pub raw_input_sha256: String,
    pub raw_input_bytes: u64,
    pub media: Media,
    pub selection: TextScope,
    pub guidance: Option<String>,
}
impl ArchivedNormalizedSource {
    pub fn from_notebook(
        conversation: Uuid,
        turn: Uuid,
        source: &ValidatedNotebook,
        selection: TextScope,
        guidance: Option<String>,
    ) -> Result<Self> {
        let identity =
            source.source_identity(conversation, turn, &selection, guidance.as_deref())?;
        let document = source.document();
        let value = Self {
            id: identity.archive_id,
            conversation,
            turn,
            document: document.document,
            format: document.format.clone(),
            source_sha256: document.source_sha256.clone(),
            extractor_revision: document.extractor_revision.clone(),
            units_sha256: source.units_sha256().to_owned(),
            raw_input_sha256: source.raw_sha256().to_owned(),
            raw_input_bytes: source.raw_bytes() as u64,
            media: Media {
                sha256: digest(source.canonical_bytes()),
                bytes: source.canonical_bytes().len() as u64,
                media_type: MEDIA_TYPE.to_owned(),
            },
            selection,
            guidance,
        };
        value.validate()?;
        ensure!(
            value.identity_digest()? == identity.source_identity_digest,
            "notebook source recipe mismatch"
        );
        Ok(value)
    }
    pub fn identity_digest(&self) -> Result<String> {
        let scope = match self.selection {
            TextScope::Page { page } => serde_json::json!(["page", page]),
            TextScope::SuppliedNotebook => serde_json::json!(["supplied-notebook"]),
        };
        Ok(digest(&serde_json::to_vec(&serde_json::json!([
            "writer-notebook-source-v1",
            self.conversation,
            self.turn,
            self.document,
            self.format,
            self.source_sha256,
            self.extractor_revision,
            self.units_sha256,
            self.raw_input_sha256,
            self.raw_input_bytes,
            self.media.sha256,
            self.media.bytes,
            self.media.media_type,
            scope,
            self.guidance
        ]))?))
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.conversation.is_nil() && !self.turn.is_nil() && !self.document.is_nil(),
            "notebook archive identity absent"
        );
        ensure!(
            self.id
                == Uuid::new_v5(
                    &Uuid::NAMESPACE_URL,
                    format!(
                        "urn:rmb:writer-notebook-source:v1:{}",
                        self.identity_digest()?
                    )
                    .as_bytes()
                ),
            "notebook archive identity mismatch"
        );
        self.media.validate()?;
        ensure!(
            self.media.media_type == MEDIA_TYPE
                && self.media.bytes > 0
                && self.media.bytes <= MAX_CANONICAL_BYTES as u64
                && self.raw_input_bytes > 0
                && self.raw_input_bytes <= MAX_CANONICAL_BYTES as u64,
            "invalid notebook archive bounds"
        );
        ensure!(
            [
                &self.source_sha256,
                &self.units_sha256,
                &self.raw_input_sha256
            ]
            .into_iter()
            .all(|hash| valid_digest(hash)),
            "invalid notebook archive digest"
        );
        ensure!(
            !self.extractor_revision.trim().is_empty()
                && self.extractor_revision.len() <= 128
                && !self.extractor_revision.chars().any(char::is_control),
            "invalid notebook extractor revision"
        );
        ensure!(
            !matches!(self.selection, TextScope::Page { page: 0 }),
            "invalid notebook scope"
        );
        ensure!(
            self.guidance
                .as_ref()
                .is_none_or(|s| !s.trim().is_empty() && s.len() <= 4096 && !s.contains('\0')),
            "invalid notebook guidance"
        );
        Ok(())
    }
    /// Requires committed reachability and the complete canonical JSON blob.
    /// Original raw provenance is retained; refinement never rereads its path.
    pub fn load(&self, store: &Store) -> Result<ValidatedNotebook> {
        self.validate()?;
        let bytes = store.read_object(&ObjectRef {
            sha256: self.media.sha256.clone(),
            bytes: self.media.bytes,
        })?;
        self.validate_bytes(&bytes)
    }
    pub(crate) fn validate_bytes(&self, bytes: &[u8]) -> Result<ValidatedNotebook> {
        self.validate()?;
        ensure!(
            bytes.len() as u64 == self.media.bytes,
            "notebook archive size mismatch"
        );
        let source = ValidatedNotebook::from_archived_bytes(
            bytes,
            &self.media.sha256,
            &self.raw_input_sha256,
            self.raw_input_bytes,
        )?;
        let doc = source.document();
        ensure!(
            doc.document == self.document
                && doc.format == self.format
                && doc.source_sha256 == self.source_sha256
                && doc.extractor_revision == self.extractor_revision
                && source.units_sha256() == self.units_sha256,
            "notebook archive content mismatch"
        );
        ensure!(
            source
                .source_identity(
                    self.conversation,
                    self.turn,
                    &self.selection,
                    self.guidance.as_deref()
                )?
                .archive_id
                == self.id,
            "notebook archive scope mismatch"
        );
        Ok(source)
    }
}

#[cfg(test)]
mod tests;

mod processing;
pub use processing::*;

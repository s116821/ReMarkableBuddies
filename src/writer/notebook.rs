//! Immutable normalized input and complete host-only notebook coverage.
//! Planning is not Store admission, a fresh stage grant, or native ownership.
use crate::{
    conversation::TextScope,
    retrieval::{Index, NormalizedDocument},
    storage::{digest, Uuid},
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const MAX_CANONICAL_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SELECTED_BYTES: usize = 256 * 1024;
pub const MAX_SELECTED_UNITS: usize = 256;
pub const MAX_MAPS: usize = 8;
pub const MAX_CALLS: usize = 9;
pub const MAX_RANGE_BYTES: usize = 32 * 1024;
pub const MAX_CONTEXT_BYTES: usize = 512;
pub const MEDIA_TYPE: &str =
    "application/vnd.remarkable-buddies.normalized-document+json;version=1";
pub const CHUNKER_REVISION: &str = "notebook-chunker-v1";
mod input;
pub use input::{read_document_file, InputRefusal};
mod payload;
pub use payload::{FrozenNotebookPayloads, NotebookMapOutput, PayloadContext};
pub mod runner;

/// Validated observed buffer, not an atomic concurrent-filesystem snapshot.
/// Includes unselected units; raw presentation is represented by hash and size.
pub struct ValidatedNotebook {
    document: NormalizedDocument,
    canonical: Vec<u8>,
    raw_sha256: String,
    raw_bytes: usize,
    units_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceIdentity {
    pub archive_id: Uuid,
    pub source_identity_digest: String,
}

/// Original unit ordinal and half-open UTF-8 byte range. Empty supplied units
/// retain [unit,0,0] in authoritative coverage, never in optional context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "[usize; 3]")]
pub struct Segment(usize, usize, usize);
impl TryFrom<[usize; 3]> for Segment {
    type Error = &'static str;
    fn try_from([unit, start, end]: [usize; 3]) -> std::result::Result<Self, Self::Error> {
        if start > end || end > 4 * 1024 * 1024 {
            return Err("invalid notebook range");
        }
        Ok(Self(unit, start, end))
    }
}
impl Segment {
    pub fn unit(&self) -> usize {
        self.0
    }
    pub fn range(&self) -> [usize; 2] {
        [self.1, self.2]
    }
    pub fn bytes(&self) -> usize {
        self.2 - self.1
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
    ordinal: usize,
    authoritative: Vec<Segment>,
    before_context: Vec<Segment>,
    after_context: Vec<Segment>,
}
impl Chunk {
    pub fn ordinal(&self) -> usize {
        self.ordinal
    }
    pub fn authoritative(&self) -> &[Segment] {
        &self.authoritative
    }
    pub fn before_context(&self) -> &[Segment] {
        &self.before_context
    }
    pub fn after_context(&self) -> &[Segment] {
        &self.after_context
    }
    /// Exact ordered recipe from the central notebook identities contract.
    pub fn identity_recipe(&self) -> Value {
        json!([
            "map",
            self.ordinal,
            self.authoritative,
            self.before_context,
            self.after_context
        ])
    }
}

/// Coverage only: encoded payload reservation and fresh durable stage admission
/// must additionally succeed before any caller can invoke a model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Coverage {
    selected_bytes: usize,
    selected_units: usize,
    chunks: Vec<Chunk>,
}
impl Coverage {
    pub fn selected_bytes(&self) -> usize {
        self.selected_bytes
    }
    pub fn selected_units(&self) -> usize {
        self.selected_units
    }
    pub fn chunks(&self) -> &[Chunk] {
        &self.chunks
    }
    pub fn required_calls(&self) -> usize {
        self.chunks.len() + 1 // All ordered maps, then one reducer.
    }
}

impl ValidatedNotebook {
    /// Canonical bytes already reached through a committed archive. Retain the
    /// archive's original raw-input facts rather than inventing new provenance
    /// from the reserialized document during refinement.
    pub(crate) fn from_archived_bytes(
        canonical: &[u8],
        canonical_sha256: &str,
        raw_sha256: &str,
        raw_bytes: u64,
    ) -> Result<Self> {
        ensure!(
            raw_bytes > 0
                && raw_bytes <= MAX_CANONICAL_BYTES as u64
                && crate::storage::valid_digest(raw_sha256),
            "invalid archived raw provenance"
        );
        let mut source = Self::from_bytes(canonical, canonical_sha256)?;
        ensure!(
            source.canonical_bytes() == canonical,
            "noncanonical notebook archive"
        );
        source.raw_sha256 = raw_sha256.to_owned();
        source.raw_bytes = raw_bytes as usize;
        Ok(source)
    }
    /// The file-admission layer must supply the exact bounded observed bytes.
    /// This function performs no I/O, Store mutation or credential/provider work.
    pub fn from_bytes(raw: &[u8], expected_sha256: &str) -> Result<Self> {
        ensure!(raw.len() <= MAX_CANONICAL_BYTES, "notebook input too large");
        ensure!(
            expected_sha256.len() == 64
                && expected_sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid expected notebook digest"
        );
        ensure!(
            digest(raw) == expected_sha256,
            "notebook input digest mismatch"
        );
        let document: NormalizedDocument = serde_json::from_slice(raw)
            .map_err(|_| anyhow::anyhow!("invalid normalized notebook JSON"))?;
        // Reuse complete source validation; relevance ranking never selects
        // which supplied content survives the archive or notebook plan.
        Index::build(document.clone())?;
        let canonical = serde_json::to_vec(&document)?;
        ensure!(
            canonical.len() <= MAX_CANONICAL_BYTES,
            "notebook archive too large"
        );
        let units_sha256 = digest(&serde_json::to_vec(&document.units)?);
        Ok(Self {
            document,
            canonical,
            raw_sha256: expected_sha256.to_owned(),
            raw_bytes: raw.len(),
            units_sha256,
        })
    }
    pub fn document(&self) -> &NormalizedDocument {
        &self.document
    }
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical
    }
    pub fn raw_sha256(&self) -> &str {
        &self.raw_sha256
    }
    pub fn raw_bytes(&self) -> usize {
        self.raw_bytes
    }
    pub fn units_sha256(&self) -> &str {
        &self.units_sha256
    }
    pub fn text(&self, segment: &Segment) -> Result<&str> {
        self.document
            .units
            .get(segment.0)
            .and_then(|unit| unit.text.get(segment.1..segment.2))
            .ok_or_else(|| anyhow::anyhow!("invalid notebook source range"))
    }
    pub fn source_identity(
        &self,
        conversation: Uuid,
        original_user_turn: Uuid,
        selection: &TextScope,
        guidance: Option<&str>,
    ) -> Result<SourceIdentity> {
        ensure!(
            !conversation.is_nil() && !original_user_turn.is_nil(),
            "notebook source identity absent"
        );
        ensure!(
            guidance.is_none_or(|s| !s.trim().is_empty() && s.len() <= 4096 && !s.contains('\0')),
            "invalid explicit notebook guidance"
        );
        self.selected(selection)?;
        let scope = match selection {
            TextScope::Page { page } => json!(["page", page]),
            TextScope::SuppliedNotebook => json!(["supplied-notebook"]),
        };
        let doc = &self.document;
        let recipe = json!([
            "writer-notebook-source-v1",
            conversation,
            original_user_turn,
            doc.document,
            doc.format,
            doc.source_sha256,
            doc.extractor_revision,
            self.units_sha256,
            self.raw_sha256,
            self.raw_bytes,
            digest(&self.canonical),
            self.canonical.len(),
            MEDIA_TYPE,
            scope,
            guidance
        ]);
        let hash = digest(&serde_json::to_vec(&recipe)?);
        Ok(SourceIdentity {
            archive_id: Uuid::new_v5(
                &Uuid::NAMESPACE_URL,
                format!("urn:rmb:writer-notebook-source:v1:{hash}").as_bytes(),
            ),
            source_identity_digest: hash,
        })
    }
    fn selected(&self, selection: &TextScope) -> Result<Vec<usize>> {
        let selected: Vec<_> = self
            .document
            .units
            .iter()
            .enumerate()
            .filter(|(_, unit)| match selection {
                TextScope::Page { page } => unit.page == Some(*page),
                TextScope::SuppliedNotebook => true,
            })
            .map(|(i, _)| i)
            .collect();
        ensure!(
            !selected.is_empty() && selected.len() <= MAX_SELECTED_UNITS,
            "notebook selected unit bound"
        );
        let bytes: usize = selected
            .iter()
            .map(|&i| self.document.units[i].text.len())
            .sum();
        ensure!(
            bytes <= MAX_SELECTED_BYTES,
            "notebook selected text too large"
        );
        ensure!(
            selected
                .iter()
                .any(|&i| !self.document.units[i].text.trim().is_empty()),
            "empty notebook selection"
        );
        Ok(selected)
    }
    pub fn coverage(&self, selection: &TextScope, max_model_calls: usize) -> Result<Coverage> {
        ensure!(
            (1..=MAX_CALLS).contains(&max_model_calls),
            "invalid notebook call allowance"
        );
        let selected = self.selected(selection)?;
        let selected_bytes = selected
            .iter()
            .map(|&i| self.document.units[i].text.len())
            .sum();
        let mut chunks = Vec::new();
        let mut ranges = Vec::new();
        let mut bytes = 0;
        for &unit in &selected {
            let text = &self.document.units[unit].text;
            if text.is_empty() {
                ranges.push(Segment(unit, 0, 0));
                continue;
            }
            let mut start = 0;
            while start < text.len() {
                let remaining = text.len() - start;
                // Prefer keeping a remaining whole unit/range intact over
                // filling the tail of a previous map with an arbitrary split.
                if bytes > 0 && remaining > MAX_RANGE_BYTES - bytes {
                    push_chunk(&mut chunks, &mut ranges)?;
                    bytes = 0;
                }
                let mut end = (start + MAX_RANGE_BYTES - bytes).min(text.len());
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                if end < text.len() {
                    if let Some(offset) = text[start..end].rfind("\n\n") {
                        end = start + offset + 2;
                    }
                }
                ensure!(end > start, "notebook range cannot advance");
                ranges.push(Segment(unit, start, end));
                bytes += end - start;
                start = end;
            }
        }
        if !ranges.is_empty() {
            push_chunk(&mut chunks, &mut ranges)?;
        }
        ensure!(
            chunks.len() < max_model_calls,
            "notebook plan exceeds call allowance"
        );
        for chunk in &mut chunks {
            let first = chunk.authoritative.first().expect("nonempty chunk");
            let last = chunk.authoritative.last().expect("nonempty chunk");
            chunk.before_context = self.context(&selected, first, true);
            chunk.after_context = self.context(&selected, last, false);
        }
        Ok(Coverage {
            selected_bytes,
            selected_units: selected.len(),
            chunks,
        })
    }
    fn context(&self, selected: &[usize], edge: &Segment, before: bool) -> Vec<Segment> {
        let mut position = selected
            .iter()
            .position(|&i| i == edge.0)
            .expect("selected edge");
        let mut offset = if before { edge.1 } else { edge.2 };
        let mut remaining = MAX_CONTEXT_BYTES;
        let mut result = Vec::new();
        loop {
            let unit = selected[position];
            let text = &self.document.units[unit].text;
            let (mut start, mut end) = if before {
                (offset.saturating_sub(remaining), offset)
            } else {
                (offset, (offset + remaining).min(text.len()))
            };
            while !text.is_char_boundary(start) {
                start += 1;
            }
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            if start < end {
                remaining -= end - start;
                result.push(Segment(unit, start, end));
            }
            // A boundary inside this unit may leave less than one UTF-8 scalar
            // of budget; context stays adjacent instead of skipping that scalar.
            if remaining == 0 || (before && start > 0) || (!before && end < text.len()) {
                break;
            }
            if before {
                if position == 0 {
                    break;
                }
                position -= 1;
                offset = self.document.units[selected[position]].text.len();
            } else {
                if position + 1 == selected.len() {
                    break;
                }
                position += 1;
                offset = 0;
            }
        }
        if before {
            result.reverse();
        }
        result
    }
}

fn push_chunk(chunks: &mut Vec<Chunk>, ranges: &mut Vec<Segment>) -> Result<()> {
    ensure!(chunks.len() < MAX_MAPS, "notebook plan exceeds map bound");
    chunks.push(Chunk {
        ordinal: chunks.len(),
        authoritative: std::mem::take(ranges),
        before_context: Vec::new(),
        after_context: Vec::new(),
    });
    Ok(())
}

#[cfg(test)]
mod tests;

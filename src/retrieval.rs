//! Derived normalized-source retrieval; no extraction or native identity authority.
use crate::storage::{digest, Uuid};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const MAX_TEXT: usize = 4 * 1024 * 1024;
const CHUNK_BYTES: usize = 2048;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceFormat {
    Pdf,
    Epub,
    Notebook,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUnit {
    pub page: Option<u32>,
    pub section: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NormalizedDocument {
    pub document: Uuid,
    pub format: SourceFormat,
    /// Extracting adapter's digest of original bytes; not an extraction proof.
    pub source_sha256: String,
    pub extractor_revision: String,
    pub units: Vec<SourceUnit>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexKey {
    pub document: Uuid,
    pub format: SourceFormat,
    pub source_sha256: String,
    pub extractor_revision: String,
    pub normalized_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Passage {
    pub unit: usize,
    pub page: Option<u32>,
    pub section: Option<String>,
    /// Half-open UTF8 byte offsets into the original normalized source unit.
    pub range: [usize; 2],
    pub text: String,
    pub direct_match: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Retrieved {
    pub source: IndexKey,
    pub passages: Vec<Passage>,
    /// A match or requested neighbor was omitted by the count/encoded-byte bound.
    pub budget_limited: bool,
}

#[derive(Clone)]
struct Chunk {
    unit: usize,
    range: [usize; 2],
    terms: BTreeSet<String>,
}

pub struct Index {
    key: IndexKey,
    units: Vec<SourceUnit>,
    chunks: Vec<Chunk>,
}

/// Receiving code can substitute a retriever without coupling it to an LLM.
pub trait Provider {
    fn query(
        &self,
        expected: &IndexKey,
        question: &str,
        max_passages: usize,
        max_bytes: usize,
        neighbors: bool,
    ) -> Result<Retrieved>;
}

fn terms(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

impl Index {
    pub fn build(document: NormalizedDocument) -> Result<Self> {
        ensure!(
            !document.document.is_nil(),
            "source document identity absent"
        );
        ensure!(
            document.source_sha256.len() == 64
                && document
                    .source_sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid source digest"
        );
        ensure!(
            !document.extractor_revision.trim().is_empty()
                && document.extractor_revision.len() <= 128
                && !document.extractor_revision.chars().any(char::is_control),
            "invalid extractor revision"
        );
        ensure!(
            !document.units.is_empty() && document.units.len() <= 4096,
            "invalid source unit count"
        );
        let mut total = 0usize;
        let mut locations = BTreeSet::new();
        for unit in &document.units {
            ensure!(unit.page != Some(0), "invalid source page");
            ensure!(
                unit.section.as_ref().is_none_or(|section| {
                    !section.trim().is_empty()
                        && section.len() <= 512
                        && !section.chars().any(char::is_control)
                }),
                "invalid source section"
            );
            ensure!(
                unit.page.is_some() || unit.section.is_some(),
                "source location absent"
            );
            ensure!(
                locations.insert((unit.page, unit.section.clone())),
                "duplicate source location"
            );
            ensure!(unit.text.len() <= 512 * 1024, "source unit exceeds bound");
            total = total
                .checked_add(unit.text.len())
                .ok_or_else(|| anyhow::anyhow!("source text exceeds bound"))?;
            ensure!(total <= MAX_TEXT, "source text exceeds bound");
            ensure!(
                !unit
                    .text
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')),
                "unsupported source control character"
            );
        }
        let key = IndexKey {
            document: document.document,
            format: document.format,
            source_sha256: document.source_sha256,
            extractor_revision: document.extractor_revision,
            normalized_sha256: digest(&serde_json::to_vec(&document.units)?),
        };
        let mut chunks = Vec::new();
        for (unit, value) in document.units.iter().enumerate() {
            let mut start = 0;
            while start < value.text.len() {
                let mut end = (start + CHUNK_BYTES).min(value.text.len());
                while !value.text.is_char_boundary(end) {
                    end -= 1;
                }
                if end < value.text.len() {
                    if let Some((offset, character)) = value.text[start..end]
                        .char_indices()
                        .rev()
                        .find(|(offset, c)| *offset >= CHUNK_BYTES / 2 && c.is_whitespace())
                    {
                        end = start + offset + character.len_utf8();
                    }
                }
                chunks.push(Chunk {
                    unit,
                    range: [start, end],
                    terms: terms(&value.text[start..end]),
                });
                ensure!(chunks.len() <= 8192, "source chunk count exceeds bound");
                start = end;
            }
        }
        Ok(Self {
            key,
            units: document.units,
            chunks,
        })
    }

    pub fn key(&self) -> &IndexKey {
        &self.key
    }

    /// Searches the whole immutable index each time; no previous-result filter.
    pub fn query(
        &self,
        expected: &IndexKey,
        question: &str,
        max_passages: usize,
        max_bytes: usize,
        neighbors: bool,
    ) -> Result<Retrieved> {
        ensure!(expected == &self.key, "source index identity changed");
        ensure!(
            !question.trim().is_empty() && question.len() <= 2048 && !question.contains('\0'),
            "invalid retrieval question"
        );
        ensure!(
            (1..=16).contains(&max_passages) && (512..=16_384).contains(&max_bytes),
            "invalid retrieval budget"
        );
        let words = terms(question);
        ensure!(
            !words.is_empty() && words.len() <= 64,
            "invalid retrieval terms"
        );
        let mut ranked: Vec<_> = self
            .chunks
            .iter()
            .enumerate()
            .filter_map(|(ordinal, chunk)| {
                let score = chunk.terms.intersection(&words).count();
                (score > 0).then_some((ordinal, score))
            })
            .collect();
        ranked.sort_by_key(|&(ordinal, score)| (std::cmp::Reverse(score), ordinal));
        let direct: BTreeSet<_> = ranked.iter().map(|&(ordinal, _)| ordinal).collect();
        let mut candidates: Vec<_> = ranked.iter().map(|&(ordinal, _)| ordinal).collect();
        if neighbors {
            for &(ordinal, _) in &ranked {
                for neighbor in [ordinal.checked_sub(1), ordinal.checked_add(1)]
                    .into_iter()
                    .flatten()
                {
                    if self
                        .chunks
                        .get(neighbor)
                        .is_some_and(|chunk| chunk.unit == self.chunks[ordinal].unit)
                    {
                        candidates.push(neighbor);
                    }
                }
            }
        }
        let mut result = Retrieved {
            source: self.key.clone(),
            passages: Vec::new(),
            budget_limited: false,
        };
        ensure!(
            serde_json::to_vec(&result)?.len() <= max_bytes,
            "retrieval metadata exceeds budget"
        );
        let mut seen = BTreeSet::new();
        for ordinal in candidates {
            if !seen.insert(ordinal) {
                continue;
            }
            if result.passages.len() == max_passages {
                result.budget_limited = true;
                continue;
            }
            let chunk = &self.chunks[ordinal];
            let unit = &self.units[chunk.unit];
            result.passages.push(Passage {
                unit: chunk.unit,
                page: unit.page,
                section: unit.section.clone(),
                range: chunk.range,
                text: unit.text[chunk.range[0]..chunk.range[1]].to_owned(),
                direct_match: direct.contains(&ordinal),
            });
            if serde_json::to_vec(&result)?.len() > max_bytes {
                result.passages.pop();
                result.budget_limited = true;
            }
        }
        ensure!(
            direct.is_empty() || result.passages.iter().any(|passage| passage.direct_match),
            "retrieval matches cannot fit budget"
        );
        ensure!(
            serde_json::to_vec(&result)?.len() <= max_bytes,
            "retrieval result exceeds budget"
        );
        Ok(result)
    }
}

impl Provider for Index {
    fn query(
        &self,
        expected: &IndexKey,
        question: &str,
        max_passages: usize,
        max_bytes: usize,
        neighbors: bool,
    ) -> Result<Retrieved> {
        Index::query(self, expected, question, max_passages, max_bytes, neighbors)
    }
}

/// Derived in-process cache only. Installing different normalized material replaces it.
#[derive(Default)]
pub struct Cache {
    indexes: BTreeMap<Uuid, Index>,
}

impl Cache {
    /// Returns true when material changed; validation completes before replacement.
    pub fn install(&mut self, document: NormalizedDocument) -> Result<bool> {
        let index = Index::build(document)?;
        if self
            .indexes
            .get(&index.key.document)
            .is_some_and(|old| old.key == index.key)
        {
            return Ok(false);
        }
        ensure!(
            self.indexes.contains_key(&index.key.document) || self.indexes.len() < 16,
            "retrieval cache document bound exceeded"
        );
        self.indexes.insert(index.key.document, index);
        Ok(true)
    }

    pub fn get(&self, document: Uuid) -> Option<&Index> {
        self.indexes.get(&document)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> NormalizedDocument {
        NormalizedDocument {
            document: Uuid::new_v4(),
            format: SourceFormat::Pdf,
            source_sha256: digest(b"original document bytes"),
            extractor_revision: "fixture-extractor-v1".into(),
            units: vec![
                SourceUnit {
                    page: Some(1),
                    section: Some("Capacitors".into()),
                    text: "Capacitance stores charge. Capacitors use dielectric material.\r\n"
                        .into(),
                },
                SourceUnit {
                    page: Some(2),
                    section: Some("Inductors".into()),
                    text: "Inductance stores magnetic energy.\n線圈 stores energy.".into(),
                },
            ],
        }
    }

    #[test]
    fn actual_queries_search_new_pages_and_preserve_exact_provenance() {
        let input = document();
        let index = Index::build(input.clone()).unwrap();
        let provider: &dyn Provider = &index;
        let first = provider
            .query(index.key(), "capacitors dielectric", 8, 8192, true)
            .unwrap();
        assert_eq!(first.passages.len(), 1);
        assert_eq!(first.passages[0].page, Some(1));
        assert_eq!(first.passages[0].text, input.units[0].text);
        let followup = provider
            .query(index.key(), "線圈 magnetic", 8, 8192, true)
            .unwrap();
        assert_eq!(followup.passages.len(), 1);
        assert_eq!(followup.passages[0].page, Some(2));
        assert_eq!(followup.source.source_sha256, input.source_sha256);
        assert_eq!(
            followup.source.normalized_sha256,
            digest(&serde_json::to_vec(&input.units).unwrap())
        );
        let empty = provider
            .query(index.key(), "unrelatedterm", 8, 8192, true)
            .unwrap();
        assert!(empty.passages.is_empty());
        assert!(!empty.budget_limited);
        let mut foreign = index.key().clone();
        foreign.document = Uuid::new_v4();
        assert!(provider
            .query(&foreign, "magnetic", 8, 8192, false)
            .is_err());
    }

    #[test]
    fn cache_replacement_invalidates_old_source_extractor_and_normalized_content() {
        let mut input = document();
        let id = input.document;
        let mut cache = Cache::default();
        assert!(cache.install(input.clone()).unwrap());
        let original = cache.get(id).unwrap().key().clone();
        assert!(!cache.install(input.clone()).unwrap());
        for change in 0..3 {
            match change {
                0 => input.source_sha256 = digest(b"changed original bytes"),
                1 => input.extractor_revision = "fixture-v2".into(),
                _ => input.units[0].text = "Replacement text about resistance.".into(),
            }
            let before = cache.get(id).unwrap().key().clone();
            assert!(cache.install(input.clone()).unwrap());
            let current = cache.get(id).unwrap();
            assert_ne!(current.key(), &before);
            assert!(current.query(&original, "charge", 8, 8192, false).is_err());
        }
        let current = cache.get(id).unwrap();
        assert!(current
            .query(current.key(), "dielectric", 8, 8192, false)
            .unwrap()
            .passages
            .is_empty());
        let mut invalid = input;
        invalid.units[0].page = Some(0);
        let key = current.key().clone();
        assert!(cache.install(invalid).is_err());
        assert_eq!(cache.get(id).unwrap().key(), &key);
    }

    #[test]
    fn utf8_chunks_neighbors_ranges_and_deduplication_are_exact() {
        let mut input = document();
        input.units[0].text = format!("{} target {}", "a ".repeat(1024), "中".repeat(1500));
        let index = Index::build(input.clone()).unwrap();
        let result = index
            .query(index.key(), "target", 16, 16_384, true)
            .unwrap();
        let rebuilt: String = index
            .chunks
            .iter()
            .filter(|chunk| chunk.unit == 0)
            .map(|chunk| &input.units[0].text[chunk.range[0]..chunk.range[1]])
            .collect();
        assert_eq!(rebuilt, input.units[0].text);
        assert!(result.passages.len() >= 2);
        let mut seen = BTreeSet::new();
        for passage in &result.passages {
            assert_eq!(passage.unit, 0); // Never leaks a different page as neighbor.
            assert!(seen.insert((passage.unit, passage.range)));
            assert_eq!(
                passage.text,
                input.units[passage.unit].text[passage.range[0]..passage.range[1]]
            );
            assert!(passage.text.len() <= CHUNK_BYTES);
        }
        assert!(result.passages.iter().any(|p| p.direct_match));
        let all = index
            .query(index.key(), "target 中 a", 16, 16_384, true)
            .unwrap();
        let unique: BTreeSet<_> = all.passages.iter().map(|p| (p.unit, p.range)).collect();
        assert_eq!(unique.len(), all.passages.len());
    }

    #[test]
    fn full_encoded_budget_refuses_unrepresentable_matches_and_marks_omissions() {
        let mut input = document();
        input.units[0].text = format!("target {}", "\"\\\t".repeat(600));
        input.units[1].text = "target short text".into();
        let index = Index::build(input).unwrap();
        let result = index.query(index.key(), "target", 1, 1024, true).unwrap();
        assert_eq!(result.passages.len(), 1);
        assert_eq!(result.passages[0].page, Some(2));
        assert!(result.budget_limited);
        assert!(serde_json::to_vec(&result).unwrap().len() <= 1024);
        let result = index.query(index.key(), "target", 1, 8192, true).unwrap();
        assert!(result.budget_limited);
        assert_eq!(result.passages.len(), 1);
        let mut only = document();
        only.units.truncate(1);
        only.units[0].text = format!("target {}", "\"\\\t".repeat(600));
        let only = Index::build(only).unwrap();
        assert!(only.query(only.key(), "target", 8, 512, true).is_err());
    }

    #[test]
    fn invalid_material_and_query_bounds_refuse_without_echoing_content() {
        let base = document();
        for invalid in 0..7 {
            let mut input = base.clone();
            match invalid {
                0 => input.document = Uuid::nil(),
                1 => input.source_sha256 = "secret-invalid-digest".into(),
                2 => input.extractor_revision.clear(),
                3 => input.units[0].section = Some("\0secret".into()),
                4 => {
                    input.units[0].page = None;
                    input.units[0].section = None;
                }
                5 => input.units.push(input.units[0].clone()),
                _ => input.units[0].text = "secret\0text".into(),
            }
            let error = Index::build(input).err().unwrap().to_string();
            assert!(!error.contains("secret"));
        }
        let index = Index::build(base).unwrap();
        for query in ["", "   ", "!!!", "x\0y"] {
            assert!(index.query(index.key(), query, 8, 8192, true).is_err());
        }
        assert!(index.query(index.key(), "charge", 0, 8192, true).is_err());
        assert!(index.query(index.key(), "charge", 17, 8192, true).is_err());
        assert!(index.query(index.key(), "charge", 8, 16_385, true).is_err());
    }

    #[test]
    fn long_material_and_cache_capacity_are_bounded_without_replacing_valid_entries() {
        let mut input = document();
        input.units[0].text = "x".repeat(512 * 1024 + 1);
        assert!(Index::build(input).is_err());
        let mut input = document();
        input.units = (0..9)
            .map(|page| SourceUnit {
                page: Some(page + 1),
                section: None,
                text: "x".repeat(512 * 1024),
            })
            .collect();
        assert!(Index::build(input).is_err());
        let mut cache = Cache::default();
        let mut first = None;
        for _ in 0..16 {
            let input = document();
            first.get_or_insert(input.document);
            cache.install(input).unwrap();
        }
        assert!(cache.install(document()).is_err());
        assert!(cache.get(first.unwrap()).is_some());
    }
}

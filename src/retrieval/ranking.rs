//! Derived lexical statistics only; no persisted identity or confidence score.
use super::{tokens, Chunk};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn frequencies(text: &str) -> Result<(BTreeMap<String, usize>, usize)> {
    let mut counts = BTreeMap::<String, usize>::new();
    let mut length = 0usize;
    for token in tokens(text) {
        let count = counts.entry(token).or_default();
        *count = count
            .checked_add(1)
            .context("source token count exceeds bound")?;
        length = length
            .checked_add(1)
            .context("source token count exceeds bound")?;
    }
    Ok((counts, length))
}

pub(super) struct Corpus {
    frequencies: BTreeMap<String, usize>,
    searchable: usize,
    tokens: usize,
}

impl Corpus {
    pub(super) fn build(chunks: &[Chunk]) -> Result<Self> {
        let mut value = Self {
            frequencies: BTreeMap::new(),
            searchable: 0,
            tokens: 0,
        };
        for chunk in chunks.iter().filter(|chunk| chunk.tokens > 0) {
            value.searchable = value
                .searchable
                .checked_add(1)
                .context("source corpus count exceeds bound")?;
            value.tokens = value
                .tokens
                .checked_add(chunk.tokens)
                .context("source corpus count exceeds bound")?;
            for term in chunk.terms.keys() {
                let count = value.frequencies.entry(term.clone()).or_default();
                *count = count
                    .checked_add(1)
                    .context("source corpus count exceeds bound")?;
            }
        }
        Ok(value)
    }

    pub(super) fn score(&self, chunk: &Chunk, question: &BTreeSet<String>) -> f64 {
        if self.searchable == 0 || chunk.tokens == 0 {
            return 0.0;
        }
        let n = self.searchable as f64;
        let average = self.tokens as f64 / n;
        let length = chunk.tokens as f64;
        // Positive finite denominators follow from admitted nonempty corpus counts.
        question
            .iter()
            .filter_map(|term| {
                let frequency = *chunk.terms.get(term)? as f64;
                let documents = *self.frequencies.get(term)? as f64;
                let idf = (1.0 + (n - documents + 0.5) / (documents + 0.5)).ln();
                Some(idf * frequency * 2.2 / (frequency + 1.2 * (0.25 + 0.75 * length / average)))
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_length_repetition_has_diminishing_positive_returns() {
        let chunks: Vec<_> = (1..=3)
            .map(|count| {
                let text = format!(
                    "{}{}",
                    "charge ".repeat(count),
                    "filler ".repeat(100 - count)
                );
                let (terms, tokens) = frequencies(&text).unwrap();
                Chunk {
                    unit: 0,
                    range: [0, text.len()],
                    terms,
                    tokens,
                }
            })
            .collect();
        let corpus = Corpus::build(&chunks).unwrap();
        let question = BTreeSet::from(["charge".to_owned()]);
        let scores: Vec<_> = chunks.iter().map(|c| corpus.score(c, &question)).collect();
        assert!(scores.iter().all(|x| x.is_finite() && *x > 0.0));
        assert!(scores[0] < scores[1] && scores[1] < scores[2]);
        assert!(scores[2] - scores[1] < scores[1] - scores[0]);
    }
}

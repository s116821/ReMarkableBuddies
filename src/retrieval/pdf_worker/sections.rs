//! Read EPUB locations from the same normalized bytes the parent hashes.
use super::{epub, CHUNK_BYTES};
use anyhow::{ensure, Result};
use serde::{
    de::{Error, IgnoredAny, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use std::{collections::BTreeSet, fmt, io};

struct Chunks<F>(F);

impl<F: FnMut(&mut [u8]) -> Result<usize>> io::Read for Chunks<F> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        (self.0)(buffer).map_err(io::Error::other)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Location {
    section: String,
    page: Option<u32>,
    #[serde(rename = "text")]
    _text: IgnoredAny,
}

struct Spine(usize);

impl<'de> Visitor<'de> for Spine {
    type Value = Vec<String>;

    fn expecting(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "{} ordered EPUB sections", self.0)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut sections = Vec::with_capacity(self.0);
        while let Some(unit) = seq.next_element::<Location>()? {
            if sections.len() == self.0 || unit.page.is_some() {
                return Err(A::Error::custom("Invalid EPUB unit count or page number"));
            }
            epub::validate_archive_path(&unit.section).map_err(A::Error::custom)?;
            sections.push(unit.section);
        }
        if sections.len() != self.0 || sections.iter().collect::<BTreeSet<_>>().len() != self.0 {
            return Err(A::Error::custom("Missing or duplicate EPUB section"));
        }
        Ok(sections)
    }
}

pub(super) fn from_units(
    count: u32,
    read: impl FnMut(&mut [u8]) -> Result<usize>,
) -> Result<Vec<String>> {
    let count = usize::try_from(count)?;
    ensure!(
        (1..=epub::MAX_SECTIONS).contains(&count),
        "Invalid EPUB section count"
    );
    let input = io::BufReader::with_capacity(CHUNK_BYTES, Chunks(read));
    let mut json = serde_json::Deserializer::from_reader(input);
    let sections = json.deserialize_seq(Spine(count))?;
    json.end()?;
    Ok(sections)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn section_order_comes_from_each_hashed_unit_even_between_chunks() {
        let bytes = br#"[{"page":null,"section":"b.xhtml","text":"a.xhtml"},{"page":null,"section":"a.xhtml","text":"b.xhtml"}]"#;
        let parse = |mut input: &[u8], count| {
            from_units(count, |buffer| {
                let length = buffer.len().min(3);
                Ok(input.read(&mut buffer[..length])?)
            })
        };
        assert_eq!(parse(bytes, 2).unwrap(), ["b.xhtml", "a.xhtml"]);
        assert!(parse(bytes, 1).is_err());
        assert!(parse(&bytes[..bytes.len() - 1], 2).is_err());
        assert!(parse(
            br#"[{"page":1,"section":"a.xhtml","text":"wrong location"}]"#,
            1
        )
        .is_err());
        assert!(parse(
            br#"[{"page":null,"section":"a","text":""},{"page":null,"section":"a","text":""}]"#,
            2
        )
        .is_err());
    }
}

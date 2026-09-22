//! Bounded, read-only RMv6 text diagnostics. This does not authorize native edits.
//!
//! Format and CRDT ordering adapted from rmscene 0.8.0 (Rick Lupton, MIT):
//! https://github.com/ricklupton/rmscene; license: docs/licenses/rmscene-MIT.txt.
//! Unknown text fields fail; opaque root/layout bytes are retained, not interpreted.
use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

const HEADER: &[u8] = b"reMarkable .lines file, version=6          ";
const MAX_FILE: usize = 16 * 1024 * 1024;
const MAX_ITEMS: usize = 100_000;
type Id = (u8, u64);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Character {
    pub value: char,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Paragraph {
    pub style: u8,
    pub characters: Vec<Character>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NativeText {
    pub paragraphs: Vec<Paragraph>,
    /// Includes position/width and unrecognized trailing fields, byte-for-byte.
    pub root_layout: Vec<u8>,
    /// Every non-text block, including opaque metadata, in original order.
    /// Only validated PageInfo text/line counters are normalized for comparison.
    /// Native bytes are never rewritten by this module.
    pub scene_records: Vec<Vec<u8>>,
}

impl NativeText {
    pub fn text(&self) -> String {
        self.paragraphs
            .iter()
            .map(|p| p.characters.iter().map(|c| c.value).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(n <= self.0.len(), "Truncated native text field");
        let (value, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(value)
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn var(&mut self) -> Result<u64> {
        let mut value = 0;
        for shift in (0..70).step_by(7) {
            let byte = self.byte()?;
            ensure!(shift < 63 || byte <= 1, "Native integer overflow");
            value |= u64::from(byte & 127) << shift;
            if byte & 128 == 0 {
                return Ok(value);
            }
        }
        bail!("Native integer overflow")
    }
    fn id(&mut self) -> Result<Id> {
        Ok((self.byte()?, self.var()?))
    }
    fn tag(&mut self, index: u64, kind: u64) -> Result<()> {
        ensure!(
            self.var()? == (index << 4 | kind),
            "Unsupported native text tag"
        );
        Ok(())
    }
    fn tagged_id(&mut self, index: u64) -> Result<Id> {
        self.tag(index, 15)?;
        self.id()
    }
    fn sub(&mut self, index: u64) -> Result<Reader<'a>> {
        self.tag(index, 12)?;
        let n = self.u32()? as usize;
        Ok(Reader(self.take(n)?))
    }
    fn done(&self) -> Result<()> {
        ensure!(self.0.is_empty(), "Unknown native text fields");
        Ok(())
    }
    fn count(&mut self) -> Result<usize> {
        let n = usize::try_from(self.var()?)?;
        ensure!(n <= MAX_ITEMS, "Native text item limit exceeded");
        Ok(n)
    }
}

#[derive(Clone, Debug)]
enum Value {
    Char(char),
    Format(u32),
    Deleted,
}
#[derive(Clone, Debug)]
struct Item {
    id: Id,
    left: Id,
    right: Id,
    value: Value,
}

fn items(reader: &mut Reader<'_>) -> Result<Vec<Item>> {
    let count = reader.count()?;
    let mut result = Vec::new();
    for _ in 0..count {
        let mut item = reader.sub(0)?;
        let id = item.tagged_id(2)?;
        let left = item.tagged_id(3)?;
        let right = item.tagged_id(4)?;
        item.tag(5, 4)?;
        let deleted = item.u32()? as usize;
        let mut text = String::new();
        let mut format = None;
        if !item.0.is_empty() {
            let mut string = item.sub(6)?;
            let n = usize::try_from(string.var()?)?;
            ensure!(string.byte()? == 1, "Unsupported native string encoding");
            text = std::str::from_utf8(string.take(n)?)?.to_owned();
            if !string.0.is_empty() {
                string.tag(2, 4)?;
                format = Some(string.u32()?);
            }
            string.done()?;
        }
        item.done()?;
        if let Some(code) = format {
            ensure!(
                text.is_empty() && deleted == 0 && (1..=4).contains(&code),
                "Unsupported native formatting"
            );
            result.push(Item {
                id,
                left,
                right,
                value: Value::Format(code),
            });
        } else {
            ensure!(
                deleted == 0 || text.is_empty(),
                "Conflicting native deletion"
            );
            let length = if deleted > 0 {
                deleted
            } else {
                text.chars().count()
            };
            ensure!(
                result.len().saturating_add(length) <= MAX_ITEMS,
                "Native text expansion limit exceeded"
            );
            let values: Vec<_> = if deleted > 0 {
                vec![Value::Deleted; length]
            } else {
                text.chars().map(Value::Char).collect()
            };
            for (offset, value) in values.into_iter().enumerate() {
                let current = (
                    id.0,
                    id.1.checked_add(offset as u64)
                        .context("Native ID overflow")?,
                );
                let previous = if offset == 0 {
                    left
                } else {
                    (id.0, current.1 - 1)
                };
                let next = if offset + 1 == length {
                    right
                } else {
                    (
                        id.0,
                        current.1.checked_add(1).context("Native ID overflow")?,
                    )
                };
                result.push(Item {
                    id: current,
                    left: previous,
                    right: next,
                    value,
                });
            }
        }
        ensure!(result.len() <= MAX_ITEMS, "Native text item limit exceeded");
    }
    reader.done()?;
    Ok(result)
}

/// Kahn ordering with the same concurrent-author tie break as rmscene 0.8.
fn order(items: &[Item]) -> Result<Vec<usize>> {
    let mut ids = BTreeMap::new();
    for (i, item) in items.iter().enumerate() {
        ensure!(
            item.id != (0, 0) && ids.insert(item.id, i + 1).is_none(),
            "Duplicate/reserved native text ID"
        );
    }
    let end = items.len() + 1;
    let mut edges = vec![Vec::new(); end + 1];
    let mut degree = vec![0; end + 1];
    for (i, item) in items.iter().enumerate() {
        let node = i + 1;
        let left = if item.left == (0, 0) {
            0
        } else {
            *ids.get(&item.left)
                .context("Unresolved native left text anchor")?
        };
        let right = if item.right == (0, 0) {
            end
        } else {
            *ids.get(&item.right)
                .context("Unresolved native right text anchor")?
        };
        edges[left].push(node);
        degree[node] += 1;
        edges[node].push(right);
        degree[right] += 1;
    }
    let key = |node: usize| {
        if node == 0 {
            (0, 0, 0, node)
        } else if node == end {
            (2, 0, 0, node)
        } else {
            let id = items[node - 1].id;
            (1, 255 - id.0, id.1, node)
        }
    };
    let mut ready: BTreeSet<_> = (0..=end)
        .filter(|&node| degree[node] == 0)
        .map(key)
        .collect();
    let mut result = Vec::new();
    while let Some((_, _, _, node)) = ready.pop_first() {
        if node == end {
            break;
        }
        if node != 0 {
            result.push(node - 1);
        }
        for &next in &edges[node] {
            degree[next] -= 1;
            if degree[next] == 0 {
                ready.insert(key(next));
            }
        }
    }
    ensure!(result.len() == items.len(), "Cyclic native text ordering");
    Ok(result)
}

fn root(mut reader: Reader<'_>) -> Result<NativeText> {
    ensure!(
        reader.tagged_id(1)? == (0, 0),
        "Unsupported native text root"
    );
    let mut body = reader.sub(2)?;
    let mut outer_items = body.sub(1)?;
    let sequence = items(&mut outer_items.sub(1)?)?;
    outer_items.done()?;
    let mut outer_styles = body.sub(2)?;
    let mut style_data = outer_styles.sub(1)?;
    let mut styles = BTreeMap::new();
    for _ in 0..style_data.count()? {
        let id = style_data.id()?;
        style_data.tagged_id(1)?;
        let mut style = style_data.sub(2)?;
        ensure!(style.byte()? == 17, "Unsupported native paragraph encoding");
        let code = style.byte()?;
        ensure!(
            code <= 7 && styles.insert(id, code).is_none(),
            "Unsupported/duplicate native paragraph style"
        );
        style.done()?;
    }
    style_data.done()?;
    outer_styles.done()?;
    body.done()?;
    // Preserve layout and unknown trailing root data exactly. Editing policy must
    // independently establish compatibility; successful text extraction is not it.
    let layout = reader.0.to_vec();
    let mut position = reader.sub(3)?;
    for _ in 0..2 {
        ensure!(
            f64::from_le_bytes(position.take(8)?.try_into()?).is_finite(),
            "Invalid text position"
        );
    }
    position.done()?;
    reader.tag(4, 4)?;
    let width = f32::from_bits(reader.u32()?);
    ensure!(width.is_finite() && width > 0.0, "Invalid text width");
    let mut paragraphs = Vec::new();
    let mut paragraph = Paragraph {
        style: *styles.get(&(0, 0)).unwrap_or(&1),
        characters: Vec::new(),
    };
    let (mut bold, mut italic) = (false, false);
    for index in order(&sequence)? {
        let item = &sequence[index];
        match item.value {
            Value::Deleted => {}
            Value::Format(code) => match code {
                1 => bold = true,
                2 => bold = false,
                3 => italic = true,
                4 => italic = false,
                _ => unreachable!(),
            },
            Value::Char('\n') => {
                paragraphs.push(paragraph);
                paragraph = Paragraph {
                    style: *styles.get(&item.id).unwrap_or(&1),
                    characters: Vec::new(),
                };
            }
            Value::Char(value) => paragraph.characters.push(Character {
                value,
                bold,
                italic,
            }),
        }
    }
    paragraphs.push(paragraph);
    Ok(NativeText {
        paragraphs,
        root_layout: layout,
        scene_records: Vec::new(),
    })
}

pub fn read(bytes: &[u8]) -> Result<NativeText> {
    ensure!(
        bytes.len() <= MAX_FILE,
        "Native file exceeds diagnostic limit"
    );
    let mut reader = Reader(bytes);
    ensure!(
        reader.take(HEADER.len())? == HEADER,
        "Unsupported native file version"
    );
    let mut text = None;
    let mut scene_records = Vec::new();
    let mut page_counts = None;
    let mut count = 0;
    while !reader.0.is_empty() {
        count += 1;
        ensure!(count <= MAX_ITEMS, "Native block limit exceeded");
        let n = reader.u32()? as usize;
        ensure!(reader.byte()? == 0, "Unsupported native block header");
        let minimum = reader.byte()?;
        let version = reader.byte()?;
        let kind = reader.byte()?;
        ensure!(minimum <= version, "Invalid native block version");
        let data = reader.take(n)?;
        if kind == 7 {
            ensure!(
                minimum == 1 && version == 1 && text.is_none(),
                "Unsupported/multiple native text roots"
            );
            text = Some(root(Reader(data))?);
        } else {
            let mut record = vec![kind, minimum, version];
            record.extend_from_slice(data);
            if kind == 10 {
                ensure!(
                    minimum == 0 && version == 1 && page_counts.is_none(),
                    "Unsupported/multiple PageInfo blocks"
                );
                let mut info = Reader(data);
                let mut values = Vec::new();
                for index in 1..=5 {
                    info.tag(index, 4)?;
                    values.push(info.u32()?);
                }
                info.done()?;
                page_counts = Some((values[2], values[3]));
                // PageInfo's known text/line counters change with the text.
                // Validate them below before excluding those fields from
                // preservation equality. All other bytes remain exact.
                record[14..18].fill(0);
                record[19..23].fill(0);
            }
            scene_records.push(record);
        }
    }
    let mut text = text.context("No native text root")?;
    if let Some((characters, lines)) = page_counts {
        ensure!(
            characters as usize == text.text().chars().count() + 1
                && lines as usize == text.paragraphs.len(),
            "PageInfo counters disagree with complete native text"
        );
    }
    text.scene_records = scene_records;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    const APPLIED: &[u8] = include_bytes!("../../tests/fixtures/native-history/applied.rm");
    #[test]
    fn native_short_delete_and_restore_preserve_prior_text_styles_and_ink() {
        let applied = read(APPLIED).unwrap();
        let removed = read(include_bytes!(
            "../../tests/fixtures/native-history/removed.rm"
        ))
        .unwrap();
        let restored = read(include_bytes!(
            "../../tests/fixtures/native-history/restored.rm"
        ))
        .unwrap();
        assert_eq!(applied.paragraphs.len(), 20);
        assert_eq!(removed.paragraphs.len(), 16);
        assert_eq!(applied.paragraphs[..15], removed.paragraphs[..15]);
        assert!(removed.paragraphs[15].characters.is_empty());
        assert_eq!(
            applied.text(),
            format!("{}Q: REM15?\n\nA: a+b=c; x^2 +/- 1.\n---\n", removed.text())
        );
        assert_eq!(applied.paragraphs, restored.paragraphs);
        assert_eq!(applied.root_layout, removed.root_layout);
        assert_eq!(applied.root_layout, restored.root_layout);
        assert_eq!(applied.scene_records, removed.scene_records);
        assert_eq!(applied.scene_records, restored.scene_records);
    }
    #[test]
    fn native_long_range_preserves_all_previous_content_and_operators() {
        let applied = read(include_bytes!(
            "../../tests/fixtures/native-history/long-applied.rm"
        ))
        .unwrap();
        let removed = read(include_bytes!(
            "../../tests/fixtures/native-history/long-removed.rm"
        ))
        .unwrap();
        let restored = read(include_bytes!(
            "../../tests/fixtures/native-history/long-restored.rm"
        ))
        .unwrap();
        let qa = include_str!("../../tests/fixtures/native-history/long-qa.txt");
        assert_eq!(qa.len(), 328);
        assert_eq!(applied.text(), format!("{}{qa}", removed.text()));
        assert_eq!(applied.paragraphs[..19], removed.paragraphs[..19]);
        assert_eq!(applied, restored);
        assert_eq!(applied.scene_records, removed.scene_records);
    }
    #[test]
    fn malformed_files_and_integer_overflow_fail_without_partial_output() {
        for end in 0..HEADER.len() + 8 {
            assert!(read(&APPLIED[..end]).is_err());
        }
        assert!(read(&APPLIED[..APPLIED.len() - 1]).is_err());
        let mut invalid = APPLIED.to_vec();
        invalid[HEADER.len()..HEADER.len() + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read(&invalid).is_err());
        assert!(Reader(&[255; 11]).var().is_err());
        assert!(Reader(&[0x80, 0x01]).var().is_ok());
    }
    #[test]
    fn cyclic_and_duplicate_crdt_ids_are_rejected() {
        let a = Item {
            id: (1, 1),
            left: (1, 2),
            right: (0, 0),
            value: Value::Char('a'),
        };
        let b = Item {
            id: (1, 2),
            left: (1, 1),
            right: (0, 0),
            value: Value::Char('b'),
        };
        assert!(order(&[a.clone(), b]).is_err());
        assert!(order(&[a.clone(), a]).is_err());
    }

    #[test]
    fn dangling_nonzero_anchors_are_not_guessed_as_boundaries() {
        for (left, right) in [((9, 9), (0, 0)), ((0, 0), (9, 9))] {
            assert!(order(&[Item {
                id: (1, 1),
                left,
                right,
                value: Value::Char('a')
            }])
            .is_err());
        }
    }

    #[test]
    fn opaque_blocks_are_preserved_and_bad_metadata_counts_are_rejected() {
        let original = read(APPLIED).unwrap();
        let mut extended = APPLIED.to_vec();
        extended.extend_from_slice(&[1, 0, 0, 0, 0, 1, 1, 222, 42]);
        let extra = read(&extended).unwrap();
        assert_ne!(original.scene_records, extra.scene_records);
        assert_eq!(extra.scene_records.last().unwrap(), &[222, 1, 1, 42]);
        let mut altered = APPLIED.to_vec();
        let mut offset = HEADER.len();
        while offset < altered.len() {
            let size = u32::from_le_bytes(altered[offset..offset + 4].try_into().unwrap()) as usize;
            if altered[offset + 7] == 10 {
                altered[offset + 8 + 11] ^= 1;
                assert!(read(&altered).is_err());
                return;
            }
            offset += 8 + size;
        }
        panic!("fixture requires PageInfo");
    }
}

//! Closed declared-map semantics on the existing maintained typed parser.
//! Never invokes an upstream eager CMap constructor or interprets a font program.
use super::{absent, decode_stream, dictionary, number, resolve};
use anyhow::{bail, ensure, Context, Result};
use lopdf::{content::Content, Dictionary, Document, Object, ObjectId, Stream, StringFormat};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

const MAP_BYTES: usize = 64 * 1024;
const TOTAL_BYTES: usize = 1024 * 1024;
const MAP_ENTRIES: usize = 4096;
const TOTAL_ENTRIES: usize = 65536;
const TARGET_BYTES: usize = 256 * 1024;
const METADATA: usize = 65536;

fn charge(used: &mut usize, add: usize, limit: usize, error: &'static str) -> Result<()> {
    ensure!(add <= limit.saturating_sub(*used), error);
    *used += add;
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Width {
    One,
    Two,
}
impl Width {
    fn bytes(self) -> usize {
        match self {
            Self::One => 1,
            Self::Two => 2,
        }
    }
    fn code(self, bytes: &[u8]) -> Result<u16> {
        ensure!(
            bytes.len() == self.bytes(),
            "Unsupported PDF Unicode code width"
        );
        Ok(match self {
            Self::One => u16::from(bytes[0]),
            Self::Two => u16::from_be_bytes([bytes[0], bytes[1]]),
        })
    }
}
pub(super) struct Map {
    entries: BTreeMap<u16, String>,
    width: Width,
}
impl Map {
    pub(super) fn emit(
        &self,
        bytes: &[u8],
        mut emit: impl FnMut(char) -> Result<()>,
    ) -> Result<()> {
        ensure!(
            bytes.len().is_multiple_of(self.width.bytes()),
            "Truncated PDF Unicode code"
        );
        for bytes in bytes.chunks_exact(self.width.bytes()) {
            for ch in self.text(self.width.code(bytes)?)?.chars() {
                emit(ch)?;
            }
        }
        Ok(())
    }
    pub(super) fn text(&self, code: u16) -> Result<&str> {
        self.entries
            .get(&code)
            .map(String::as_str)
            .context("Unmapped PDF Unicode code")
    }
}

#[derive(Default)]
pub(super) struct Cache {
    fonts: BTreeMap<ObjectId, Arc<Map>>,
    maps: BTreeMap<ObjectId, Arc<Map>>,
    decoded: usize,
    operations: usize,
    entries: usize,
    target: usize,
    metadata: usize,
}

#[derive(PartialEq, Eq)]
struct SystemInfo {
    registry: Vec<u8>,
    ordering: Vec<u8>,
    supplement: i64,
}
fn reference(value: &Object) -> Result<ObjectId> {
    value
        .as_reference()
        .map_err(|_| anyhow::anyhow!("PDF Unicode resource must be indirect"))
}
fn name(value: &Object, bound: usize) -> Result<&[u8]> {
    let value = value
        .as_name()
        .map_err(|_| anyhow::anyhow!("Invalid PDF Unicode name"))?;
    ensure!(
        !value.is_empty() && value.len() <= bound,
        "PDF Unicode name bound"
    );
    Ok(value)
}
fn integer(value: &Object) -> Result<i64> {
    value
        .as_i64()
        .map_err(|_| anyhow::anyhow!("Invalid PDF Unicode integer"))
}
fn string(value: &Object, bound: usize) -> Result<&[u8]> {
    let value = value
        .as_str()
        .map_err(|_| anyhow::anyhow!("Invalid PDF Unicode metadata string"))?;
    ensure!(
        !value.is_empty() && value.len() <= bound,
        "PDF Unicode metadata string bound"
    );
    Ok(value)
}
fn hex(value: &Object) -> Result<&[u8]> {
    match value {
        Object::String(bytes, StringFormat::Hexadecimal) => Ok(bytes),
        _ => bail!("PDF Unicode mapping requires hexadecimal strings"),
    }
}
fn code(value: &Object, width: Width) -> Result<u16> {
    width.code(hex(value)?)
}
fn target(bytes: &[u8]) -> Result<String> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= 16 && bytes.len().is_multiple_of(2),
        "PDF Unicode target byte bound"
    );
    let mut out = String::new();
    for value in char::decode_utf16(
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_be_bytes(*b)),
    ) {
        let ch = value.map_err(|_| anyhow::anyhow!("Invalid PDF Unicode target"))?;
        ensure!(
            ch != '\u{fffd}' && (!ch.is_control() || matches!(ch, '\t' | '\n' | '\r')),
            "Unsupported PDF Unicode target scalar"
        );
        out.push(ch);
    }
    Ok(out)
}

impl Cache {
    fn keys(&mut self, dict: &Dictionary, allowed: &[&[u8]]) -> Result<()> {
        charge(
            &mut self.metadata,
            dict.len(),
            METADATA,
            "PDF Unicode metadata work bound",
        )?;
        ensure!(
            dict.iter()
                .all(|(key, _)| allowed.contains(&key.as_slice())),
            "Unsupported PDF Unicode resource field"
        );
        Ok(())
    }
    fn array<'a>(&mut self, doc: &'a Document, value: &'a Object) -> Result<&'a Vec<Object>> {
        let values = resolve(doc, value)?
            .as_array()
            .map_err(|_| anyhow::anyhow!("Invalid PDF Unicode metadata array"))?;
        charge(
            &mut self.metadata,
            values.len(),
            METADATA,
            "PDF Unicode metadata work bound",
        )?;
        Ok(values)
    }
    fn info(&mut self, doc: &Document, value: &Object) -> Result<SystemInfo> {
        let dict = dictionary(doc, value)?;
        self.keys(dict, &[b"Registry", b"Ordering", b"Supplement"])?;
        let registry = string(
            resolve(
                doc,
                dict.get(b"Registry")
                    .context("Missing PDF character registry")?,
            )?,
            128,
        )?
        .to_vec();
        let ordering = string(
            resolve(
                doc,
                dict.get(b"Ordering")
                    .context("Missing PDF character ordering")?,
            )?,
            128,
        )?
        .to_vec();
        let supplement = integer(resolve(
            doc,
            dict.get(b"Supplement")
                .context("Missing PDF character supplement")?,
        )?)?;
        ensure!(supplement >= 0, "Invalid PDF character supplement");
        Ok(SystemInfo {
            registry,
            ordering,
            supplement,
        })
    }
    // CMap programs contain literal metadata, not PDF resource lookups. No
    // Document is available here, so nested references cannot acquire semantics.
    fn direct_info(&mut self, value: &Object) -> Result<SystemInfo> {
        let dict = value
            .as_dict()
            .map_err(|_| anyhow::anyhow!("CMap body requires a direct system info dictionary"))?;
        self.keys(dict, &[b"Registry", b"Ordering", b"Supplement"])?;
        let registry = string(
            dict.get(b"Registry")
                .context("Missing PDF character registry")?,
            128,
        )?
        .to_vec();
        let ordering = string(
            dict.get(b"Ordering")
                .context("Missing PDF character ordering")?,
            128,
        )?
        .to_vec();
        let supplement = integer(
            dict.get(b"Supplement")
                .context("Missing PDF character supplement")?,
        )?;
        ensure!(supplement >= 0, "Invalid PDF character supplement");
        Ok(SystemInfo {
            registry,
            ordering,
            supplement,
        })
    }
    fn opaque(&mut self, doc: &Document, value: &Object) -> Result<()> {
        reference(value)?;
        let stream = resolve(doc, value)?
            .as_stream()
            .map_err(|_| anyhow::anyhow!("Invalid opaque PDF font stream"))?;
        absent(&stream.dict, &[b"F", b"FFilter", b"FDecodeParms"])
    }
    fn descriptor(&mut self, doc: &Document, value: &Object, base: &[u8]) -> Result<()> {
        let dict = dictionary(doc, value)?;
        self.keys(
            dict,
            &[
                b"Type",
                b"FontName",
                b"FontFamily",
                b"FontStretch",
                b"FontWeight",
                b"Flags",
                b"FontBBox",
                b"ItalicAngle",
                b"Ascent",
                b"Descent",
                b"Leading",
                b"CapHeight",
                b"XHeight",
                b"StemV",
                b"StemH",
                b"AvgWidth",
                b"MaxWidth",
                b"MissingWidth",
                b"FontFile2",
                b"CharSet",
                b"CIDSet",
            ],
        )?;
        ensure!(
            dict.has_type(b"FontDescriptor")
                && name(
                    resolve(doc, dict.get(b"FontName").context("Missing PDF font name")?)?,
                    128
                )? == base,
            "Invalid PDF Unicode font descriptor identity"
        );
        for (key, value) in dict {
            match key.as_slice() {
                b"Type" | b"FontName" => {}
                b"FontFamily" | b"CharSet" => {
                    let value = resolve(doc, value)?
                        .as_str()
                        .map_err(|_| anyhow::anyhow!("Invalid PDF font descriptor string"))?;
                    ensure!(value.len() <= 512, "PDF font descriptor string bound");
                }
                b"FontStretch" => {
                    name(resolve(doc, value)?, 512)?;
                }
                b"Flags" => {
                    ensure!(
                        integer(resolve(doc, value)?)? >= 0,
                        "Invalid PDF font flags"
                    );
                }
                b"FontBBox" => {
                    let array = self.array(doc, value)?;
                    ensure!(array.len() == 4, "Invalid PDF font bounding box");
                    for item in array {
                        number(resolve(doc, item)?)?;
                    }
                }
                b"FontFile2" | b"CIDSet" => self.opaque(doc, value)?,
                _ => {
                    number(resolve(doc, value)?)?;
                }
            }
        }
        Ok(())
    }
    fn widths(&mut self, doc: &Document, value: &Object) -> Result<()> {
        let array = self.array(doc, value)?;
        ensure!(array.len() <= 8192, "PDF font width item bound");
        let mut items = array.len();
        let mut at = 0;
        while at < array.len() {
            let start = integer(resolve(doc, &array[at])?)?;
            ensure!((0..=65535).contains(&start), "Invalid PDF width CID");
            at += 1;
            let next = resolve(doc, array.get(at).context("Incomplete PDF width metadata")?)?;
            if let Object::Array(widths) = next {
                ensure!(
                    widths.len() <= 8192usize.saturating_sub(items),
                    "PDF font width item bound"
                );
                items += widths.len();
                charge(
                    &mut self.metadata,
                    widths.len(),
                    METADATA,
                    "PDF Unicode metadata work bound",
                )?;
                ensure!(
                    !widths.is_empty() && widths.len() <= 65536 - start as usize,
                    "Invalid PDF width array range"
                );
                for width in widths {
                    number(resolve(doc, width)?)?;
                }
                at += 1;
            } else {
                let end = integer(next)?;
                ensure!((start..=65535).contains(&end), "Invalid PDF width range");
                at += 1;
                number(resolve(
                    doc,
                    array.get(at).context("Incomplete PDF width range")?,
                )?)?;
                at += 1;
            }
        }
        Ok(())
    }
    pub(super) fn font(
        &mut self,
        doc: &Document,
        object: &Object,
        font: &Dictionary,
    ) -> Result<Arc<Map>> {
        let id = reference(object)?;
        if let Some(map) = self.fonts.get(&id) {
            ensure!(
                map.width == Width::Two,
                "PDF Unicode cached font width mismatch"
            );
            return Ok(map.clone());
        }
        ensure!(self.fonts.len() < 64, "PDF Unicode font count bound");
        self.keys(
            font,
            &[
                b"Type",
                b"Subtype",
                b"BaseFont",
                b"Encoding",
                b"DescendantFonts",
                b"ToUnicode",
                b"Name",
            ],
        )?;
        ensure!(
            font.has_type(b"Font")
                && font.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Type0")
                && font.get(b"Encoding").and_then(Object::as_name).ok() == Some(b"Identity-H"),
            "Unsupported PDF Unicode font profile"
        );
        let base = name(
            resolve(
                doc,
                font.get(b"BaseFont")
                    .context("Missing PDF Unicode base font")?,
            )?,
            128,
        )?;
        if let Ok(value) = font.get(b"Name") {
            name(resolve(doc, value)?, 128)?;
        }
        let descendants = self.array(
            doc,
            font.get(b"DescendantFonts")
                .context("Missing PDF descendant font")?,
        )?;
        ensure!(
            descendants.len() == 1,
            "Unsupported PDF descendant font count"
        );
        reference(&descendants[0])?;
        let descendant = dictionary(doc, &descendants[0])?;
        self.keys(
            descendant,
            &[
                b"Type",
                b"Subtype",
                b"BaseFont",
                b"CIDSystemInfo",
                b"FontDescriptor",
                b"DW",
                b"W",
                b"CIDToGIDMap",
            ],
        )?;
        ensure!(
            descendant.has_type(b"Font")
                && descendant.get(b"Subtype").and_then(Object::as_name).ok()
                    == Some(b"CIDFontType2")
                && name(
                    resolve(
                        doc,
                        descendant
                            .get(b"BaseFont")
                            .context("Missing PDF descendant base font")?
                    )?,
                    128
                )? == base,
            "Unsupported PDF descendant font identity"
        );
        self.info(
            doc,
            descendant
                .get(b"CIDSystemInfo")
                .context("Missing PDF descendant system info")?,
        )?;
        self.descriptor(
            doc,
            descendant
                .get(b"FontDescriptor")
                .context("Missing PDF font descriptor")?,
            base,
        )?;
        if let Ok(value) = descendant.get(b"DW") {
            number(resolve(doc, value)?)?;
        }
        if let Ok(value) = descendant.get(b"W") {
            self.widths(doc, value)?;
        }
        if let Ok(value) = descendant.get(b"CIDToGIDMap") {
            if let Object::Name(value) = resolve(doc, value)? {
                ensure!(value == b"Identity", "Unsupported PDF glyph map name");
            } else {
                self.opaque(doc, value)?;
            }
        }
        let map = self.map(
            doc,
            font.get(b"ToUnicode").context("Missing PDF Unicode map")?,
            Width::Two,
        )?;
        self.fonts.insert(id, map.clone());
        Ok(map)
    }
    pub(super) fn simple_font(
        &mut self,
        doc: &Document,
        object: &Object,
        font: &Dictionary,
    ) -> Result<Arc<Map>> {
        let id = reference(object)?;
        if let Some(map) = self.fonts.get(&id) {
            ensure!(
                map.width == Width::One,
                "PDF Unicode cached font width mismatch"
            );
            return Ok(map.clone());
        }
        ensure!(self.fonts.len() < 64, "PDF Unicode font count bound");
        self.keys(
            font,
            &[
                b"Type",
                b"Subtype",
                b"BaseFont",
                b"Encoding",
                b"FirstChar",
                b"LastChar",
                b"Widths",
                b"FontDescriptor",
                b"ToUnicode",
                b"Name",
            ],
        )?;
        ensure!(
            font.has_type(b"Font")
                && font.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"TrueType")
                && name(
                    resolve(
                        doc,
                        font.get(b"Encoding")
                            .context("Missing PDF simple encoding")?
                    )?,
                    128
                )? == b"WinAnsiEncoding",
            "Unsupported PDF single-byte font profile"
        );
        let base = name(
            resolve(
                doc,
                font.get(b"BaseFont")
                    .context("Missing PDF Unicode base font")?,
            )?,
            128,
        )?;
        if let Ok(value) = font.get(b"Name") {
            name(resolve(doc, value)?, 128)?;
        }
        let first = integer(resolve(
            doc,
            font.get(b"FirstChar")
                .context("Missing PDF first character")?,
        )?)?;
        let last = integer(resolve(
            doc,
            font.get(b"LastChar")
                .context("Missing PDF last character")?,
        )?)?;
        ensure!(
            (0..=255).contains(&first) && (first..=255).contains(&last),
            "Invalid PDF font character range"
        );
        let widths = self.array(
            doc,
            font.get(b"Widths").context("Missing PDF simple widths")?,
        )?;
        ensure!(
            widths.len() == (last - first + 1) as usize,
            "Invalid PDF simple width count"
        );
        for value in widths {
            number(resolve(doc, value)?)?;
        }
        let descriptor = font
            .get(b"FontDescriptor")
            .context("Missing PDF font descriptor")?;
        absent(dictionary(doc, descriptor)?, &[b"CIDSet"])?;
        self.descriptor(doc, descriptor, base)?;
        let map = self.map(
            doc,
            font.get(b"ToUnicode").context("Missing PDF Unicode map")?,
            Width::One,
        )?;
        // This is font-local admission, including when map() returns a cache hit.
        ensure!(
            map.entries
                .keys()
                .all(|code| (first as u16..=last as u16).contains(code)),
            "PDF Unicode map outside font character range"
        );
        self.fonts.insert(id, map.clone());
        Ok(map)
    }
    fn map(&mut self, doc: &Document, object: &Object, width: Width) -> Result<Arc<Map>> {
        let id = reference(object)?;
        if let Some(map) = self.maps.get(&id) {
            ensure!(map.width == width, "PDF Unicode cached map width mismatch");
            return Ok(map.clone());
        }
        ensure!(self.maps.len() < 64, "PDF Unicode map count bound");
        let stream = resolve(doc, object)?
            .as_stream()
            .map_err(|_| anyhow::anyhow!("Invalid PDF Unicode map stream"))?;
        self.keys(
            &stream.dict,
            &[
                b"Length",
                b"Filter",
                b"Type",
                b"CMapName",
                b"CIDSystemInfo",
                b"WMode",
            ],
        )?;
        if let Ok(value) = stream.dict.get(b"Type") {
            ensure!(
                name(resolve(doc, value)?, 128)? == b"CMap",
                "Invalid PDF CMap stream type"
            );
        }
        if let Ok(value) = stream.dict.get(b"WMode") {
            ensure!(
                integer(resolve(doc, value)?)? == 0,
                "Unsupported PDF CMap writing mode"
            );
        }
        let room = MAP_BYTES.min(TOTAL_BYTES.saturating_sub(self.decoded));
        ensure!(room > 0, "PDF cumulative CMap byte bound");
        let bytes = decode_stream(stream, room)?;
        charge(
            &mut self.decoded,
            bytes.len(),
            TOTAL_BYTES,
            "PDF cumulative CMap byte bound",
        )?;
        let parsed = Content::decode_strict(&bytes)
            .map_err(|_| anyhow::anyhow!("Malformed PDF CMap syntax"))?;
        ensure!(parsed.operations.len() <= 4096, "PDF CMap operation bound");
        charge(
            &mut self.operations,
            parsed.operations.len(),
            16384,
            "PDF cumulative CMap operation bound",
        )?;
        let map = Arc::new(self.project(doc, stream, parsed, width)?);
        self.maps.insert(id, map.clone());
        Ok(map)
    }
    fn project(
        &mut self,
        doc: &Document,
        stream: &Stream,
        parsed: Content<Vec<lopdf::content::Operation>>,
        width: Width,
    ) -> Result<Map> {
        let ops = parsed.operations;
        let mut at = 0;
        let args = take(&ops, &mut at, "findresource", 2)?;
        ensure!(
            name(&args[0], 128)? == b"CIDInit" && name(&args[1], 128)? == b"ProcSet",
            "Unsupported PDF CMap wrapper"
        );
        take(&ops, &mut at, "begin", 0)?;
        let args = take(&ops, &mut at, "dict", 1)?;
        ensure!(
            (1..=64).contains(&integer(&args[0])?),
            "PDF CMap dictionary bound"
        );
        take(&ops, &mut at, "begin", 0)?;
        take(&ops, &mut at, "begincmap", 0)?;
        let mut seen = BTreeSet::new();
        let mut cmap_name = None;
        let mut info = None;
        let mut cmap_type = false;
        while ops.get(at).is_some_and(|op| op.operator == "def") {
            let args = take(&ops, &mut at, "def", 2)?;
            let key = name(&args[0], 128)?;
            ensure!(
                seen.insert(key.to_vec()),
                "Duplicate PDF CMap metadata definition"
            );
            match key {
                b"CMapType" => {
                    ensure!(integer(&args[1])? == 2, "Unsupported PDF CMap type");
                    cmap_type = true;
                }
                b"CMapName" => cmap_name = Some(name(&args[1], 128)?.to_vec()),
                b"CIDSystemInfo" => info = Some(self.direct_info(&args[1])?),
                b"WMode" => {
                    ensure!(integer(&args[1])? == 0, "Unsupported PDF CMap writing mode");
                }
                b"CMapVersion" => {
                    number(&args[1])?;
                }
                _ => bail!("Unsupported PDF CMap metadata"),
            }
        }
        let cmap_name = cmap_name.context("Missing PDF CMap name")?;
        let info = info.context("Missing PDF CMap system info")?;
        ensure!(cmap_type, "Missing PDF CMap type");
        if let Ok(value) = stream.dict.get(b"CMapName") {
            ensure!(
                name(resolve(doc, value)?, 128)? == cmap_name,
                "PDF CMap name mismatch"
            );
        }
        if let Ok(value) = stream.dict.get(b"CIDSystemInfo") {
            ensure!(
                self.info(doc, value)? == info,
                "PDF CMap system info mismatch"
            );
        }
        let count = section_count(&ops, &mut at, "begincodespacerange", 16)?;
        let args = take(&ops, &mut at, "endcodespacerange", 2 * count)?;
        let mut spaces = Vec::new();
        for pair in args.as_chunks::<2>().0 {
            let (lo, hi) = (code(&pair[0], width)?, code(&pair[1], width)?);
            ensure!(lo <= hi, "Invalid PDF codespace range");
            spaces.push((lo, hi));
        }
        spaces.sort_unstable();
        ensure!(
            spaces.windows(2).all(|p| p[0].1 < p[1].0),
            "Overlapping PDF codespaces"
        );
        let mut builder = Builder {
            spaces,
            reserved: 0,
            entries: BTreeMap::new(),
        };
        while ops.get(at).is_some_and(|op| op.operator != "endcmap") {
            match ops[at].operator.as_str() {
                "beginbfchar" => {
                    let count = section_count(&ops, &mut at, "beginbfchar", 100)?;
                    let args = take(&ops, &mut at, "endbfchar", 2 * count)?;
                    builder.reserve(self, count)?;
                    for pair in args.as_chunks::<2>().0 {
                        builder.insert(self, code(&pair[0], width)?, target(hex(&pair[1])?)?)?;
                    }
                }
                "beginbfrange" => {
                    let count = section_count(&ops, &mut at, "beginbfrange", 100)?;
                    let args = take(&ops, &mut at, "endbfrange", 3 * count)?;
                    for triple in args.as_chunks::<3>().0 {
                        let (lo, hi) = (code(&triple[0], width)?, code(&triple[1], width)?);
                        ensure!(lo <= hi, "Invalid PDF Unicode source range");
                        let length = usize::from(hi) - usize::from(lo) + 1;
                        // This happens before a single range element is expanded.
                        builder.reserve(self, length)?;
                        match &triple[2] {
                            Object::Array(values) => {
                                ensure!(
                                    values.len() == length,
                                    "PDF Unicode range target count mismatch"
                                );
                                for (offset, value) in values.iter().enumerate() {
                                    builder.insert(
                                        self,
                                        lo + offset as u16,
                                        target(hex(value)?)?,
                                    )?;
                                }
                            }
                            value => {
                                let bytes = hex(value)?;
                                ensure!(
                                    bytes.len() == 2,
                                    "Unsupported PDF Unicode range target width"
                                );
                                let start = u32::from(u16::from_be_bytes([bytes[0], bytes[1]]));
                                let end = start + length as u32 - 1;
                                ensure!(
                                    end <= 65535 && (end < 0xd800 || start > 0xdfff),
                                    "Invalid PDF Unicode target range"
                                );
                                for offset in 0..length {
                                    let bytes = ((start + offset as u32) as u16).to_be_bytes();
                                    builder.insert(self, lo + offset as u16, target(&bytes)?)?;
                                }
                            }
                        }
                    }
                }
                _ => bail!("Unsupported PDF CMap operation"),
            }
        }
        take(&ops, &mut at, "endcmap", 0)?;
        take(&ops, &mut at, "CMapName", 0)?;
        take(&ops, &mut at, "currentdict", 0)?;
        let args = take(&ops, &mut at, "defineresource", 1)?;
        ensure!(
            name(&args[0], 128)? == b"CMap",
            "Unsupported PDF CMap final resource"
        );
        take(&ops, &mut at, "pop", 0)?;
        take(&ops, &mut at, "end", 0)?;
        take(&ops, &mut at, "end", 0)?;
        ensure!(
            at == ops.len() && !builder.entries.is_empty(),
            "Incomplete PDF CMap profile"
        );
        Ok(Map {
            entries: builder.entries,
            width,
        })
    }
}

fn take<'a>(
    ops: &'a [lopdf::content::Operation],
    at: &mut usize,
    expected: &str,
    count: usize,
) -> Result<&'a [Object]> {
    let op = ops.get(*at).context("Incomplete PDF CMap wrapper")?;
    ensure!(
        op.operator == expected && op.operands.len() == count,
        "Unsupported PDF CMap operation shape"
    );
    *at += 1;
    Ok(&op.operands)
}
fn section_count(
    ops: &[lopdf::content::Operation],
    at: &mut usize,
    expected: &str,
    max: usize,
) -> Result<usize> {
    let args = take(ops, at, expected, 1)?;
    let count = integer(&args[0])?;
    ensure!(
        count > 0 && count <= max as i64,
        "PDF CMap section count bound"
    );
    Ok(count as usize)
}
struct Builder {
    spaces: Vec<(u16, u16)>,
    reserved: usize,
    entries: BTreeMap<u16, String>,
}
impl Builder {
    fn reserve(&mut self, cache: &mut Cache, count: usize) -> Result<()> {
        ensure!(
            count <= MAP_ENTRIES.saturating_sub(self.reserved)
                && count <= TOTAL_ENTRIES.saturating_sub(cache.entries),
            "PDF Unicode mapping entry bound"
        );
        self.reserved += count;
        cache.entries += count;
        Ok(())
    }
    fn insert(&mut self, cache: &mut Cache, code: u16, text: String) -> Result<()> {
        ensure!(
            self.spaces
                .iter()
                .any(|&(lo, hi)| (lo..=hi).contains(&code)),
            "PDF Unicode code outside codespace"
        );
        ensure!(
            !self.entries.contains_key(&code),
            "Duplicate PDF Unicode mapping"
        );
        charge(
            &mut cache.target,
            text.len(),
            TARGET_BYTES,
            "PDF Unicode target text bound",
        )?;
        self.entries.insert(code, text);
        Ok(())
    }
}

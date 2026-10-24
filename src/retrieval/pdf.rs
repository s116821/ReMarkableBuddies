//! Offline supported PDF text layer; not rendering, OCR or native authority.
mod unicode;
use super::{Index, NormalizedDocument, SourceFormat, SourceUnit};
use crate::storage::{digest, Uuid};
use anyhow::{bail, ensure, Context, Result};
use lopdf::{
    content::Content, Dictionary, Document, Encoding, LoadOptions, Object, ObjectId, Stream,
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
};

const INPUT: usize = 16 * 1024 * 1024;
const PAGE_CONTENT: usize = 1024 * 1024;
const TOTAL_CONTENT: usize = 8 * 1024 * 1024;
const PAGE_TEXT: usize = 512 * 1024;
const TOTAL_TEXT: usize = 4 * 1024 * 1024;
pub const EXTRACTOR_REVISION: &str = "pdf-explicit-unicode-r2-lopdf-0.46.0";

#[derive(Debug, Serialize)]
pub struct PdfText {
    pub document: NormalizedDocument,
    /// All these page units remain present. No image decoding or OCR occurred.
    pub textless_pages: Vec<u32>,
}

// All errors deliberately exclude source strings, operator names and paths.
fn resolve<'a>(doc: &'a Document, mut object: &'a Object) -> Result<&'a Object> {
    let mut seen = BTreeSet::new();
    while let Object::Reference(id) = object {
        ensure!(
            seen.len() < 32 && seen.insert(*id),
            "PDF reference limit or cycle"
        );
        object = doc.objects.get(id).context("Missing PDF object")?;
    }
    Ok(object)
}
fn dictionary<'a>(doc: &'a Document, object: &'a Object) -> Result<&'a Dictionary> {
    resolve(doc, object)?
        .as_dict()
        .map_err(|_| anyhow::anyhow!("Invalid PDF dictionary"))
}
fn number(value: &Object) -> Result<f64> {
    let value = match value {
        Object::Integer(value) => *value as f64,
        Object::Real(value) => f64::from(*value),
        _ => bail!("Invalid PDF numeric operand"),
    };
    ensure!(value.is_finite(), "Invalid PDF numeric operand");
    Ok(value)
}
fn numbers(operands: &[Object], count: usize) -> Result<()> {
    ensure!(operands.len() == count, "Invalid PDF operand count");
    for operand in operands {
        number(operand)?;
    }
    Ok(())
}
fn absent(dict: &Dictionary, keys: &[&[u8]]) -> Result<()> {
    ensure!(
        keys.iter().all(|key| !dict.has(key)),
        "Unsupported PDF resource"
    );
    Ok(())
}

struct Page<'a> {
    dict: &'a Dictionary,
    resources: Option<&'a Dictionary>,
}
struct Pages<'a> {
    doc: &'a Document,
    seen: BTreeSet<ObjectId>,
    pages: Vec<Page<'a>>,
}
impl<'a> Pages<'a> {
    fn visit(
        &mut self,
        id: ObjectId,
        parent: Option<ObjectId>,
        inherited: Option<&'a Dictionary>,
        depth: usize,
    ) -> Result<usize> {
        ensure!(
            depth <= 32 && self.seen.len() < 1024 && self.seen.insert(id),
            "PDF page tree bound or cycle"
        );
        let dict = self
            .doc
            .objects
            .get(&id)
            .context("Missing PDF page node")?
            .as_dict()
            .map_err(|_| anyhow::anyhow!("Invalid PDF page node"))?;
        match parent {
            Some(parent) => ensure!(
                dict.get(b"Parent").and_then(Object::as_reference).ok() == Some(parent),
                "Invalid PDF page parent"
            ),
            None => ensure!(!dict.has(b"Parent"), "Invalid PDF root parent"),
        }
        let resources = if dict.has(b"Resources") {
            Some(dictionary(self.doc, dict.get(b"Resources").unwrap())?)
        } else {
            inherited
        };
        match dict.get(b"Type").and_then(Object::as_name).ok() {
            Some(b"Page") => {
                absent(dict, &[b"Kids", b"Count", b"Annots"])?;
                ensure!(self.pages.len() < 256, "PDF page count bound");
                self.pages.push(Page { dict, resources });
                Ok(1)
            }
            Some(b"Pages") => {
                let kids = resolve(self.doc, dict.get(b"Kids").context("Missing PDF Kids")?)?
                    .as_array()
                    .map_err(|_| anyhow::anyhow!("Invalid PDF Kids"))?;
                ensure!(!kids.is_empty() && kids.len() <= 1024, "PDF Kids bound");
                let mut count = 0;
                for kid in kids {
                    let child = kid
                        .as_reference()
                        .map_err(|_| anyhow::anyhow!("Invalid PDF child reference"))?;
                    count += self.visit(child, Some(id), resources, depth + 1)?;
                }
                ensure!(
                    dict.get(b"Count").and_then(Object::as_i64).ok() == Some(count as i64),
                    "Inconsistent PDF page count"
                );
                Ok(count)
            }
            _ => bail!("Unsupported PDF page node"),
        }
    }
}

#[derive(Clone, Copy)]
enum Simple {
    Standard,
    WinAnsi,
    MacRoman,
}
impl Simple {
    fn name(self) -> &'static [u8] {
        match self {
            Self::Standard => b"StandardEncoding",
            Self::WinAnsi => b"WinAnsiEncoding",
            Self::MacRoman => b"MacRomanEncoding",
        }
    }
}
enum Font {
    Simple(Simple),
    Unicode(std::sync::Arc<unicode::Map>),
}
struct Resources {
    fonts: BTreeMap<Vec<u8>, Font>,
    images: BTreeSet<Vec<u8>>,
}
fn resources(
    doc: &Document,
    source: Option<&Dictionary>,
    unicode: &mut unicode::Cache,
) -> Result<Resources> {
    let mut admitted = Resources {
        fonts: BTreeMap::new(),
        images: BTreeSet::new(),
    };
    let Some(source) = source else {
        return Ok(admitted);
    };
    absent(source, &[b"ExtGState", b"Properties"])?;
    if source.has(b"Font") {
        let fonts = dictionary(doc, source.get(b"Font").unwrap())?;
        ensure!(fonts.len() <= 64, "PDF font resource bound");
        for (alias, object) in fonts {
            ensure!(alias.len() <= 128, "PDF font alias bound");
            let font = dictionary(doc, object)?;
            if font.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Type0") {
                let map = unicode.font(doc, object, font)?;
                admitted.fonts.insert(alias.clone(), Font::Unicode(map));
                continue;
            }
            absent(
                font,
                &[
                    b"ToUnicode",
                    b"FontDescriptor",
                    b"DescendantFonts",
                    b"CharProcs",
                ],
            )?;
            ensure!(
                font.has_type(b"Font")
                    && font.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Type1"),
                "Unsupported PDF font type"
            );
            let base = font
                .get(b"BaseFont")
                .and_then(Object::as_name)
                .map_err(|_| anyhow::anyhow!("Missing PDF base font"))?;
            ensure!(
                [
                    b"Courier".as_slice(),
                    b"Courier-Bold",
                    b"Courier-Oblique",
                    b"Courier-BoldOblique",
                    b"Helvetica",
                    b"Helvetica-Bold",
                    b"Helvetica-Oblique",
                    b"Helvetica-BoldOblique",
                    b"Times-Roman",
                    b"Times-Bold",
                    b"Times-Italic",
                    b"Times-BoldItalic"
                ]
                .contains(&base),
                "Unsupported PDF base font"
            );
            let encoding = match font.get(b"Encoding").and_then(Object::as_name).ok() {
                Some(b"StandardEncoding") => Simple::Standard,
                Some(b"WinAnsiEncoding") => Simple::WinAnsi,
                Some(b"MacRomanEncoding") => Simple::MacRoman,
                _ => bail!("Unsupported PDF font encoding"),
            };
            admitted.fonts.insert(alias.clone(), Font::Simple(encoding));
        }
    }
    if source.has(b"XObject") {
        let objects = dictionary(doc, source.get(b"XObject").unwrap())?;
        ensure!(objects.len() <= 64, "PDF XObject resource bound");
        for (alias, object) in objects {
            ensure!(alias.len() <= 128, "PDF XObject alias bound");
            let stream = resolve(doc, object)?
                .as_stream()
                .map_err(|_| anyhow::anyhow!("Invalid PDF XObject"))?;
            ensure!(
                stream.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Image"),
                "Unsupported PDF XObject"
            );
            absent(&stream.dict, &[b"F", b"FFilter", b"FDecodeParms"])?;
            admitted.images.insert(alias.clone());
        }
    }
    Ok(admitted)
}

fn decode_stream(stream: &Stream, allowance: usize) -> Result<Vec<u8>> {
    absent(
        &stream.dict,
        &[b"F", b"FFilter", b"FDecodeParms", b"DecodeParms"],
    )?;
    if stream.dict.has(b"Filter") {
        ensure!(
            stream.dict.get(b"Filter").and_then(Object::as_name).ok() == Some(b"FlateDecode"),
            "Unsupported PDF content filter"
        );
    }
    stream
        .get_plain_content_with_limit(allowance)
        .map_err(|_| anyhow::anyhow!("PDF stream decode failed or exhausted"))
}
fn content(doc: &Document, page: &Dictionary, total: &mut usize) -> Result<Vec<u8>> {
    let Some(object) = page.get(b"Contents").ok() else {
        return Ok(Vec::new());
    };
    let object = resolve(doc, object)?;
    let sources: Vec<&Object> = match object {
        Object::Null => Vec::new(),
        Object::Stream(_) => vec![object],
        Object::Array(array) => {
            ensure!(array.len() <= 64, "PDF content stream count bound");
            ensure!(
                array
                    .iter()
                    .all(|value| matches!(value, Object::Reference(_))),
                "Invalid PDF content array"
            );
            array.iter().collect()
        }
        _ => bail!("Invalid PDF page contents"),
    };
    let mut out = Vec::new();
    for object in sources {
        let separator = usize::from(!out.is_empty());
        let allowance = PAGE_CONTENT
            .saturating_sub(out.len() + separator)
            .min(TOTAL_CONTENT.saturating_sub(*total + separator));
        ensure!(allowance > 0, "PDF content byte bound");
        let stream = resolve(doc, object)?
            .as_stream()
            .map_err(|_| anyhow::anyhow!("Invalid PDF content stream"))?;
        let decoded = decode_stream(stream, allowance)?;
        *total += decoded.len() + separator;
        if separator != 0 {
            out.push(b'\n');
        }
        out.extend_from_slice(&decoded);
    }
    Ok(out)
}

struct Text<'a> {
    out: String,
    total: &'a mut usize,
}
impl Text<'_> {
    fn push(&mut self, ch: char) -> Result<()> {
        ensure!(
            !ch.is_control() || matches!(ch, '\t' | '\n' | '\r'),
            "Unsupported PDF text control"
        );
        let ch = if ch.is_whitespace() { ' ' } else { ch };
        if ch == ' ' && (self.out.is_empty() || self.out.ends_with([' ', '\n'])) {
            return Ok(());
        }
        self.reserve(ch.len_utf8())?;
        self.out.push(ch);
        Ok(())
    }
    fn reserve(&mut self, bytes: usize) -> Result<()> {
        ensure!(
            self.out.len() + bytes <= PAGE_TEXT && *self.total + bytes <= TOTAL_TEXT,
            "PDF text byte bound"
        );
        *self.total += bytes;
        Ok(())
    }
    fn boundary(&mut self, newline: bool) -> Result<()> {
        if self.out.is_empty() || self.out.ends_with('\n') {
            return Ok(());
        }
        if newline {
            if self.out.ends_with(' ') {
                self.out.pop();
                *self.total -= 1;
            }
            self.reserve(1)?;
            self.out.push('\n');
        } else {
            self.push(' ')?;
        }
        Ok(())
    }
    fn shown(&mut self, bytes: &[u8], font: Option<&Font>) -> Result<()> {
        let font = match font.context("PDF shown text has no selected font")? {
            Font::Simple(font) => *font,
            Font::Unicode(map) => {
                ensure!(bytes.len().is_multiple_of(2), "Truncated PDF Unicode code");
                for code in bytes.as_chunks::<2>().0 {
                    for ch in map.text(u16::from_be_bytes([code[0], code[1]]))?.chars() {
                        self.push(ch)?;
                    }
                }
                return Ok(());
            }
        };
        // Only trusted literals enter this getter. Input dictionaries never do.
        let mut trusted = Dictionary::new();
        trusted.set("Type", Object::Name(b"Font".to_vec()));
        trusted.set("Encoding", Object::Name(font.name().to_vec()));
        let empty = Document::new();
        let Encoding::OneByteEncoding(table) = trusted
            .get_font_encoding(&empty)
            .map_err(|_| anyhow::anyhow!("PDF encoding initialization failed"))?
        else {
            bail!("Unsupported PDF encoding variant");
        };
        for byte in bytes {
            let glyph = table[usize::from(*byte)].context("Unmapped PDF shown glyph")?;
            let ch = char::from_u32(u32::from(glyph.utf16_code_unit()))
                .context("Invalid PDF shown glyph")?;
            self.push(ch)?;
        }
        Ok(())
    }
}

fn extract(
    bytes: &[u8],
    resources: &Resources,
    total_text: &mut usize,
    total_ops: &mut usize,
) -> Result<String> {
    let parsed =
        Content::decode_strict(bytes).map_err(|_| anyhow::anyhow!("Malformed PDF page content"))?;
    ensure!(
        parsed.operations.len() <= 32768 && *total_ops + parsed.operations.len() <= 131072,
        "PDF operation eligibility bound"
    );
    *total_ops += parsed.operations.len();
    let mut text = Text {
        out: String::new(),
        total: total_text,
    };
    let mut inside = false;
    let mut shown = false;
    let mut font = None;
    let mut graphics = 0usize;
    for operation in parsed.operations {
        let args = operation.operands.as_slice();
        let op = operation.operator.as_str();
        if matches!(
            op,
            "m" | "l"
                | "v"
                | "y"
                | "re"
                | "c"
                | "h"
                | "S"
                | "s"
                | "f"
                | "F"
                | "f*"
                | "B"
                | "B*"
                | "b"
                | "b*"
                | "n"
                | "W"
                | "W*"
        ) {
            ensure!(!inside, "PDF path operation inside text object");
        }
        match op {
            "BT" => {
                numbers(args, 0)?;
                ensure!(!inside, "Nested PDF text object");
                inside = true;
                shown = false;
                font = None;
            }
            "ET" => {
                numbers(args, 0)?;
                ensure!(inside, "Unbalanced PDF text object");
                inside = false;
                text.boundary(true)?;
            }
            "Tf" => {
                ensure!(inside && args.len() == 2, "Invalid PDF font selection");
                let alias = args[0]
                    .as_name()
                    .map_err(|_| anyhow::anyhow!("Invalid PDF font alias"))?;
                ensure!(number(&args[1])? > 0.0, "Invalid PDF font size");
                font = Some(
                    resources
                        .fonts
                        .get(alias)
                        .context("Missing PDF font resource")?,
                );
            }
            "Tj" | "'" | "\"" => {
                ensure!(inside, "PDF text outside text object");
                let offset = if op == "\"" { 2 } else { 0 };
                ensure!(args.len() == offset + 1, "Invalid PDF shown-text operands");
                if offset != 0 {
                    number(&args[0])?;
                    number(&args[1])?;
                }
                if op != "Tj" {
                    text.boundary(true)?;
                }
                let bytes = args[offset]
                    .as_str()
                    .map_err(|_| anyhow::anyhow!("Invalid PDF shown string"))?;
                text.shown(bytes, font)?;
                shown = true;
            }
            "TJ" => {
                ensure!(inside && args.len() == 1, "Invalid PDF text array");
                let array = args[0]
                    .as_array()
                    .map_err(|_| anyhow::anyhow!("Invalid PDF text array"))?;
                for item in array {
                    match item {
                        Object::String(bytes, _) => text.shown(bytes, font)?,
                        Object::Integer(_) | Object::Real(_) => {
                            if number(item)? <= -120.0 {
                                text.boundary(false)?;
                            }
                        }
                        _ => bail!("Unsupported PDF text array item"),
                    }
                }
                shown = true;
            }
            "T*" => {
                ensure!(inside, "PDF text state outside text object");
                numbers(args, 0)?;
                text.boundary(true)?;
            }
            "Tm" | "Td" | "TD" => {
                ensure!(inside && !shown, "Unsupported PDF text repositioning");
                numbers(args, if op == "Tm" { 6 } else { 2 })?;
            }
            "Tc" | "Tw" | "Tz" | "TL" | "Ts" | "Tr" => {
                ensure!(inside, "PDF text state outside text object");
                numbers(args, 1)?;
                if op == "Tr" {
                    ensure!(
                        number(&args[0])? == 0.0,
                        "Unsupported PDF text rendering mode"
                    );
                }
                if op == "Tz" {
                    ensure!(number(&args[0])? > 0.0, "Invalid PDF text scaling");
                }
            }
            "q" => {
                ensure!(!inside && graphics < 32, "PDF graphics state bound");
                numbers(args, 0)?;
                graphics += 1;
            }
            "Q" => {
                ensure!(!inside && graphics > 0, "Unbalanced PDF graphics state");
                numbers(args, 0)?;
                graphics -= 1;
            }
            "cm" => {
                ensure!(!inside, "Unsupported PDF transform inside text");
                numbers(args, 6)?;
            }
            "Do" => {
                ensure!(!inside && args.len() == 1, "Invalid PDF image operation");
                let alias = args[0]
                    .as_name()
                    .map_err(|_| anyhow::anyhow!("Invalid PDF image alias"))?;
                ensure!(
                    resources.images.contains(alias),
                    "Missing PDF image resource"
                );
            }
            // Closed nontext numeric/vector profile. No text-bearing extension is ignored.
            "w" | "J" | "j" | "M" | "G" | "g" => {
                numbers(args, 1)?;
            }
            "m" | "l" => {
                numbers(args, 2)?;
            }
            "RG" | "rg" => {
                numbers(args, 3)?;
            }
            "K" | "k" | "v" | "y" | "re" => {
                numbers(args, 4)?;
            }
            "c" => {
                numbers(args, 6)?;
            }
            "h" | "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "n" | "W" | "W*" => {
                numbers(args, 0)?;
            }
            _ => bail!("Unsupported PDF content operation"),
        }
    }
    ensure!(!inside && graphics == 0, "Unbalanced PDF page state");
    while text.out.ends_with([' ', '\n']) {
        text.out.pop();
        *text.total -= 1;
    }
    Ok(text.out)
}

/// Caller-supplied bytes only. Library allocations are not a global RAM/CPU bound.
pub fn normalize(document: Uuid, mut input: impl Read) -> Result<PdfText> {
    ensure!(!document.is_nil(), "Missing PDF document identity");
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 32768];
    let mut eof = false;
    for _ in 0..4096 {
        let allowance = (INPUT + 1 - bytes.len()).min(buffer.len());
        ensure!(allowance > 0, "PDF original byte bound");
        let read = input
            .read(&mut buffer[..allowance])
            .map_err(|_| anyhow::anyhow!("PDF acquisition failed"))?;
        if read == 0 {
            eof = true;
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        ensure!(bytes.len() <= INPUT, "PDF original byte bound");
    }
    ensure!(eof, "PDF acquisition call bound");
    let doc = Document::load_mem_with_options(
        &bytes,
        LoadOptions {
            strict: true,
            max_decompressed_size: Some(PAGE_CONTENT),
            ..Default::default()
        },
    )
    .map_err(|_| anyhow::anyhow!("PDF load failed or exhausted"))?;
    ensure!(
        !doc.trailer.has(b"Encrypt") && !doc.was_encrypted(),
        "Encrypted PDF unsupported"
    );
    ensure!(doc.objects.len() <= 16384, "PDF object eligibility bound");
    let catalog = dictionary(
        &doc,
        doc.trailer.get(b"Root").context("Missing PDF catalog")?,
    )?;
    ensure!(catalog.has_type(b"Catalog"), "Invalid PDF catalog type");
    absent(catalog, &[b"AcroForm", b"OCProperties"])?;
    let root = catalog
        .get(b"Pages")
        .and_then(Object::as_reference)
        .map_err(|_| anyhow::anyhow!("Missing PDF page tree"))?;
    let mut ordered = Pages {
        doc: &doc,
        seen: BTreeSet::new(),
        pages: Vec::new(),
    };
    ordered.visit(root, None, None, 1)?;
    ensure!(!ordered.pages.is_empty(), "PDF has no pages");
    let (mut total_content, mut total_text, mut total_ops) = (0, 0, 0);
    let mut units = Vec::new();
    let mut textless_pages = Vec::new();
    let mut unicode = unicode::Cache::default();
    for (offset, page) in ordered.pages.iter().enumerate() {
        let resources = resources(&doc, page.resources, &mut unicode)?;
        let content = content(&doc, page.dict, &mut total_content)?;
        let text = extract(&content, &resources, &mut total_text, &mut total_ops)?;
        let page = (offset + 1) as u32;
        if text.is_empty() {
            textless_pages.push(page);
        }
        units.push(SourceUnit {
            page: Some(page),
            section: None,
            text,
        });
    }
    ensure!(
        textless_pages.len() != units.len(),
        "PDF has no supported text"
    );
    let document = NormalizedDocument {
        document,
        format: SourceFormat::Pdf,
        source_sha256: digest(&bytes),
        extractor_revision: EXTRACTOR_REVISION.into(),
        units,
    };
    Index::build(document.clone()).context("Invalid normalized PDF output")?;
    Ok(PdfText {
        document,
        textless_pages,
    })
}

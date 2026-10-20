//! Bounded offline EPUB XML text, never rendered visibility or native authority.
use super::{Index, NormalizedDocument, SourceFormat, SourceUnit};
use crate::storage::{digest, Uuid};
use anyhow::{bail, ensure, Context, Result};
use quick_xml::{events::Event, name::ResolveResult, reader::NsReader};
use serde::Serialize;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    io::{self, Cursor, Read, Seek, SeekFrom},
    rc::Rc,
};
use zip::{CompressionMethod, ZipArchive};

const ARCHIVE_BYTES: usize = 16 * 1024 * 1024;
const XML_TOTAL: usize = 8 * 1024 * 1024;
const UNIT_BYTES: usize = 512 * 1024;
const TEXT_BYTES: usize = 4 * 1024 * 1024;
const OCF: &[u8] = b"urn:oasis:names:tc:opendocument:xmlns:container";
const OPF: &[u8] = b"http://www.idpf.org/2007/opf";
const XHTML: &[u8] = b"http://www.w3.org/1999/xhtml";

#[derive(Debug, Serialize)]
pub struct EpubText {
    pub document: NormalizedDocument,
    /// These ordered units remain present with empty text; no OCR is implied.
    pub textless_sections: Vec<String>,
}

struct Budget {
    read: u64,
    maximum: u64,
    calls: usize,
}
struct ArchiveInput<'a> {
    cursor: Cursor<&'a [u8]>,
    budget: Rc<RefCell<Budget>>,
}
impl ArchiveInput<'_> {
    fn admit(&self) -> io::Result<()> {
        let mut budget = self.budget.borrow_mut();
        if budget.calls >= 32768 {
            return Err(io::Error::other("EPUB archive I/O budget exhausted"));
        }
        budget.calls += 1;
        Ok(())
    }
}
impl Read for ArchiveInput<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.admit()?;
        let remaining = {
            let b = self.budget.borrow();
            b.maximum.saturating_sub(b.read)
        };
        if remaining == 0 {
            return Err(io::Error::other("EPUB archive read budget exhausted"));
        }
        let count = buf.len().min(remaining as usize);
        let read = self.cursor.read(&mut buf[..count])?;
        self.budget.borrow_mut().read += read as u64;
        Ok(read)
    }
}
impl Seek for ArchiveInput<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.admit()?;
        self.cursor.seek(position)
    }
}

fn archive_path(path: &str) -> Result<String> {
    ensure!(
        !path.is_empty()
            && path.len() <= 512
            && !path.chars().any(char::is_control)
            && !path.contains(['\\', ':', '?', '#'])
            && !path.starts_with('/'),
        "Unsupported EPUB resource path"
    );
    ensure!(
        path.split('/')
            .all(|p| !p.is_empty() && p != "." && p != ".."),
        "Noncanonical EPUB resource path"
    );
    Ok(path.to_owned())
}

fn reference(base: &str, href: &str) -> Result<String> {
    ensure!(
        !href.is_empty() && href.len() <= 512 && !href.contains(['?', '#', '\\', ':']),
        "Unsupported EPUB href"
    );
    let bytes = href.as_bytes();
    for (i, value) in bytes.iter().enumerate() {
        if *value == b'%' {
            ensure!(
                bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                    && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit),
                "Invalid EPUB href encoding"
            );
        }
    }
    let decoded = percent_encoding::percent_decode_str(href)
        .decode_utf8()
        .map_err(|_| anyhow::anyhow!("Invalid EPUB href encoding"))?;
    ensure!(
        !decoded.starts_with('/') && !decoded.contains(['\\', ':', '?', '#']),
        "External EPUB resource refused"
    );
    let mut parts: Vec<&str> = base
        .rsplit_once('/')
        .map_or_else(Vec::new, |(parent, _)| parent.split('/').collect());
    for part in decoded.split('/') {
        match part {
            "." => {}
            ".." => {
                ensure!(parts.pop().is_some(), "EPUB href escapes archive");
            }
            "" => bail!("Invalid EPUB href"),
            part => parts.push(part),
        }
    }
    archive_path(&parts.join("/"))
}

#[derive(PartialEq)]
struct Node {
    namespace: Vec<u8>,
    name: Vec<u8>,
}
impl Node {
    fn is(&self, namespace: &[u8], name: &[u8]) -> bool {
        self.namespace == namespace && self.name == name
    }
}
enum Xml<'a> {
    Start(BTreeMap<String, String>),
    Text(&'a str),
    End,
}

fn xml_character(character: char) -> bool {
    (!character.is_control() || matches!(character, '\n' | '\r' | '\t'))
        && !matches!(character, '\u{fffe}' | '\u{ffff}')
}

fn xml(bytes: &[u8], mut visit: impl FnMut(&[Node], Xml<'_>) -> Result<()>) -> Result<()> {
    let input = std::str::from_utf8(bytes).context("EPUB XML must be UTF8")?;
    ensure!(
        input.chars().all(xml_character),
        "Unsupported EPUB XML character"
    );
    let mut reader = NsReader::from_str(input);
    reader.config_mut().expand_empty_elements = true;
    reader.config_mut().check_comments = true;
    let mut stack = Vec::new();
    let mut roots = 0;
    let mut declaration_seen = false;
    for _ in 0..100000 {
        let (resolved, event) = reader
            .read_resolved_event()
            .map_err(|_| anyhow::anyhow!("Malformed EPUB XML"))?;
        let namespace = match resolved {
            ResolveResult::Bound(namespace) => namespace.as_ref().as_bytes().to_vec(),
            ResolveResult::Unbound => Vec::new(),
            ResolveResult::Unknown(_) => bail!("Unknown EPUB XML namespace"),
        };
        match event {
            Event::Start(element) => {
                ensure!(stack.len() < 64, "EPUB XML nesting exceeds bound");
                ensure!(
                    element.name().as_ref().len() <= 512,
                    "EPUB XML name exceeds bound"
                );
                if stack.is_empty() {
                    roots += 1;
                    ensure!(roots == 1, "Multiple EPUB XML roots");
                }
                stack.push(Node {
                    namespace,
                    name: element.local_name().as_ref().as_bytes().to_vec(),
                });
                let mut attributes = BTreeMap::new();
                for (count, attribute) in element.attributes().enumerate() {
                    ensure!(count < 32, "EPUB XML attribute count exceeds bound");
                    let attribute =
                        attribute.map_err(|_| anyhow::anyhow!("Malformed EPUB XML attribute"))?;
                    ensure!(
                        attribute.key.as_ref().len() <= 512 && attribute.value.len() <= 4096,
                        "EPUB XML attribute exceeds bound"
                    );
                    let key = attribute.key.as_ref();
                    let value = attribute
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|_| anyhow::anyhow!("Unsupported EPUB XML entity"))?;
                    ensure!(
                        value.chars().all(xml_character),
                        "Unsupported EPUB XML attribute character"
                    );
                    if !key.contains(':') {
                        attributes.insert(key.to_owned(), value.into_owned());
                    }
                }
                visit(&stack, Xml::Start(attributes))?;
            }
            Event::End(element) => {
                let node = Node {
                    namespace,
                    name: element.local_name().as_ref().as_bytes().to_vec(),
                };
                ensure!(stack.last() == Some(&node), "Unbalanced EPUB XML");
                visit(&stack, Xml::End)?;
                stack.pop();
            }
            Event::Text(value) => {
                let text = value.as_ref();
                ensure!(
                    !stack.is_empty() || text.trim().is_empty(),
                    "EPUB text outside root"
                );
                visit(&stack, Xml::Text(text))?;
            }
            Event::CData(value) => {
                ensure!(!stack.is_empty(), "EPUB CDATA outside root");
                visit(&stack, Xml::Text(value.as_ref()))?;
            }
            Event::GeneralRef(value) => {
                ensure!(!stack.is_empty(), "EPUB entity outside root");
                if let Some(character) = value
                    .resolve_char_ref()
                    .map_err(|_| anyhow::anyhow!("Invalid EPUB numeric entity"))?
                {
                    ensure!(
                        xml_character(character),
                        "Unsupported EPUB XML entity character"
                    );
                    let mut buf = [0; 4];
                    visit(&stack, Xml::Text(character.encode_utf8(&mut buf)))?;
                } else {
                    let text = match value.as_ref() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => bail!("Unsupported EPUB XML entity"),
                    };
                    visit(&stack, Xml::Text(text))?;
                }
            }
            Event::Decl(declaration) => {
                ensure!(
                    roots == 0 && !declaration_seen,
                    "Misplaced EPUB XML declaration"
                );
                declaration_seen = true;
                ensure!(
                    declaration
                        .version()
                        .map_err(|_| anyhow::anyhow!("Invalid EPUB XML version"))?
                        .as_ref()
                        == "1.0",
                    "EPUB XML must be version1.0"
                );
                if let Some(encoding) = declaration.encoding() {
                    ensure!(
                        encoding
                            .map_err(|_| anyhow::anyhow!("Invalid EPUB encoding"))?
                            .eq_ignore_ascii_case("UTF-8"),
                        "EPUB XML must be UTF8"
                    );
                }
            }
            Event::DocType(_) => bail!("EPUB DTD is unsupported"),
            Event::Eof => {
                ensure!(roots == 1 && stack.is_empty(), "Incomplete EPUB XML");
                return Ok(());
            }
            Event::Comment(_) | Event::PI(_) => {}
            _ => bail!("Unsupported EPUB XML event"),
        }
    }
    bail!("EPUB XML event count exceeds bound")
}

fn attribute<'a>(attributes: &'a BTreeMap<String, String>, key: &str) -> Result<&'a str> {
    attributes
        .get(key)
        .map(String::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Missing EPUB structural attribute"))
}

fn package_path(bytes: &[u8]) -> Result<String> {
    let mut roots = Vec::new();
    let mut rootfiles = false;
    xml(bytes, |path, event| {
        if let Xml::Start(attributes) = event {
            if path.len() == 1 {
                ensure!(
                    path[0].is(OCF, b"container") && attribute(&attributes, "version")? == "1.0",
                    "Unsupported EPUB container"
                );
            } else if path.len() == 2 && path[1].is(OCF, b"rootfiles") {
                ensure!(!rootfiles, "Duplicate EPUB rootfiles");
                rootfiles = true;
            } else if path.len() == 3
                && path[1].is(OCF, b"rootfiles")
                && path[2].is(OCF, b"rootfile")
            {
                ensure!(roots.len() < 64, "EPUB rendition count exceeds bound");
                roots.push((
                    archive_path(attribute(&attributes, "full-path")?)?,
                    attribute(&attributes, "media-type")?.to_owned(),
                ));
            }
        }
        Ok(())
    })?;
    let (path, kind) = roots
        .into_iter()
        .next()
        .context("No EPUB package rendition")?;
    ensure!(
        kind == "application/oebps-package+xml",
        "Unsupported EPUB package rendition"
    );
    Ok(path)
}

fn spine(bytes: &[u8], package: &str) -> Result<Vec<String>> {
    let mut manifest = BTreeMap::new();
    let mut order = Vec::new();
    let (mut seen_manifest, mut seen_spine, mut linear) = (false, false, false);
    xml(bytes, |path, event| {
        if let Xml::Start(attributes) = event {
            if path.len() == 1 {
                ensure!(
                    path[0].is(OPF, b"package")
                        && matches!(attribute(&attributes, "version")?, "2.0" | "3.0"),
                    "Unsupported EPUB package"
                );
            } else if path.len() == 2 && path[1].is(OPF, b"manifest") {
                ensure!(!seen_manifest && !seen_spine, "Ambiguous EPUB manifest");
                seen_manifest = true;
            } else if path.len() == 2 && path[1].is(OPF, b"spine") {
                ensure!(seen_manifest && !seen_spine, "Ambiguous EPUB spine");
                seen_spine = true;
            } else if path.len() == 3 && path[1].is(OPF, b"manifest") && path[2].is(OPF, b"item") {
                ensure!(manifest.len() < 4096, "EPUB manifest exceeds bound");
                let id = attribute(&attributes, "id")?;
                ensure!(id.len() <= 256, "EPUB manifest ID exceeds bound");
                ensure!(
                    manifest
                        .insert(
                            id.to_owned(),
                            (
                                attribute(&attributes, "href")?.to_owned(),
                                attribute(&attributes, "media-type")?.to_owned()
                            )
                        )
                        .is_none(),
                    "Duplicate EPUB manifest ID"
                );
            } else if path.len() == 3 && path[1].is(OPF, b"spine") && path[2].is(OPF, b"itemref") {
                ensure!(order.len() < 512, "EPUB spine exceeds bound");
                let value = attributes.get("linear").map_or("yes", String::as_str);
                ensure!(
                    matches!(value, "yes" | "no"),
                    "Invalid EPUB linear attribute"
                );
                linear |= value == "yes";
                order.push(attribute(&attributes, "idref")?.to_owned());
            }
        }
        Ok(())
    })?;
    ensure!(
        seen_spine && !order.is_empty() && linear,
        "Incomplete EPUB spine"
    );
    let mut used = BTreeSet::new();
    order
        .into_iter()
        .map(|id| {
            let (href, kind) = manifest.get(&id).context("Unresolved EPUB spine ID")?;
            ensure!(
                kind == "application/xhtml+xml",
                "Unsupported EPUB spine media type"
            );
            let path = reference(package, href)?;
            ensure!(used.insert(path.clone()), "Duplicate EPUB spine resource");
            Ok(path)
        })
        .collect()
}

fn body_text(bytes: &[u8]) -> Result<String> {
    let mut text = String::new();
    let (mut body, mut seen_body, mut suppressed) = (false, false, None);
    xml(bytes, |path, event| {
        match event {
            Xml::Start(_) => {
                let node = path.last().context("Missing EPUB XML root")?;
                if path.len() == 1 {
                    ensure!(node.is(XHTML, b"html"), "Unsupported EPUB content root");
                }
                if path.len() == 2 && node.is(XHTML, b"body") {
                    ensure!(!seen_body, "Duplicate EPUB body");
                    seen_body = true;
                    body = true;
                }
                if body
                    && suppressed.is_none()
                    && matches!(node.name.as_slice(), b"script" | b"style" | b"head")
                {
                    suppressed = Some(path.len());
                }
                if body && suppressed.is_none() && block(&node.name) {
                    boundary(&mut text)?;
                }
            }
            Xml::End => {
                if suppressed == Some(path.len()) {
                    suppressed = None;
                }
                if body && suppressed.is_none() && path.last().is_some_and(|n| block(&n.name)) {
                    boundary(&mut text)?;
                }
                if path.len() == 2 && path[1].is(XHTML, b"body") {
                    body = false;
                }
            }
            Xml::Text(value) if body && suppressed.is_none() => {
                for character in value.chars() {
                    if character.is_whitespace() {
                        if !text.is_empty() && !text.ends_with(char::is_whitespace) {
                            append(&mut text, ' ')?;
                        }
                    } else {
                        append(&mut text, character)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    })?;
    ensure!(seen_body, "EPUB content body missing");
    Ok(text.trim().to_owned())
}
fn block(name: &[u8]) -> bool {
    matches!(
        name,
        b"article"
            | b"aside"
            | b"main"
            | b"nav"
            | b"header"
            | b"footer"
            | b"address"
            | b"hgroup"
            | b"hr"
            | b"details"
            | b"summary"
            | b"dialog"
            | b"fieldset"
            | b"legend"
            | b"form"
            | b"menu"
            | b"center"
            | b"dir"
            | b"caption"
            | b"thead"
            | b"tbody"
            | b"tfoot"
            | b"p"
            | b"div"
            | b"h1"
            | b"h2"
            | b"h3"
            | b"h4"
            | b"h5"
            | b"h6"
            | b"pre"
            | b"li"
            | b"br"
            | b"table"
            | b"tr"
            | b"td"
            | b"th"
            | b"blockquote"
            | b"section"
            | b"ul"
            | b"ol"
            | b"dl"
            | b"dt"
            | b"dd"
            | b"figure"
            | b"figcaption"
    )
}
fn append(text: &mut String, character: char) -> Result<()> {
    ensure!(
        text.len() + character.len_utf8() <= UNIT_BYTES,
        "EPUB unit text exceeds bound"
    );
    text.push(character);
    Ok(())
}
fn boundary(text: &mut String) -> Result<()> {
    while text.ends_with(' ') {
        text.pop();
    }
    if !text.is_empty() && !text.ends_with('\n') {
        append(text, '\n')?;
    }
    Ok(())
}

type Archive<'a> = ZipArchive<ArchiveInput<'a>>;
fn resource(
    archive: &mut Archive<'_>,
    name: &str,
    cap: usize,
    total: &mut usize,
) -> Result<Vec<u8>> {
    let file = archive
        .by_name(name)
        .context("EPUB declared resource missing or unsupported")?;
    ensure!(file.size() <= cap as u64, "EPUB XML resource exceeds bound");
    let mut bytes = Vec::new();
    file.take(cap as u64 + 1)
        .read_to_end(&mut bytes)
        .context("EPUB resource decompression failed")?;
    ensure!(
        bytes.len() <= cap && *total + bytes.len() <= XML_TOTAL,
        "EPUB decoded XML exceeds bound"
    );
    *total += bytes.len();
    Ok(bytes)
}

/// Source is supplied by the caller; byte/I/O budgets are not a hard blocking-I/O deadline.
pub fn normalize(document: Uuid, source: impl Read) -> Result<EpubText> {
    ensure!(!document.is_nil(), "Missing EPUB document identity");
    let mut bytes = Vec::new();
    source
        .take(ARCHIVE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .context("EPUB acquisition failed")?;
    ensure!(bytes.len() <= ARCHIVE_BYTES, "EPUB archive exceeds bound");
    let budget = Rc::new(RefCell::new(Budget {
        read: 0,
        maximum: 2 * 1024 * 1024,
        calls: 0,
    }));
    let input = ArchiveInput {
        cursor: Cursor::new(bytes.as_slice()),
        budget: budget.clone(),
    };
    let mut archive = ZipArchive::new(input).context("Unsupported EPUB ZIP metadata")?;
    ensure!(
        archive.offset() == 0 && archive.len() <= 4096,
        "Unsupported EPUB archive layout"
    );
    archive
        .validate_header_consistency()
        .context("Inconsistent EPUB ZIP headers")?;
    let mut names = BTreeSet::new();
    for i in 0..archive.len() {
        let file = archive.by_index_raw(i)?;
        let name = file.name()?.into_owned();
        ensure!(
            file.name_raw() == name.as_bytes(),
            "EPUB filenames must be UTF8"
        );
        archive_path(name.strip_suffix('/').unwrap_or(&name))?;
        ensure!(
            names.insert(name.clone())
                && !file.encrypted()
                && file
                    .unix_mode()
                    .is_none_or(|mode| mode & 0o170000 != 0o120000),
            "Duplicate, encrypted or linked EPUB resource"
        );
        ensure!(
            matches!(
                file.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            ),
            "Unsupported EPUB compression"
        );
        ensure!(
            name != "META-INF/encryption.xml",
            "EPUB encryption metadata is unsupported"
        );
    }
    {
        let file = archive
            .by_name("mimetype")
            .context("EPUB mimetype missing")?;
        ensure!(
            file.header_start() == 0
                && file.compression() == CompressionMethod::Stored
                && file.size() == 20,
            "Invalid EPUB mimetype entry"
        );
    }
    budget.borrow_mut().maximum = 64 * 1024 * 1024;
    let mut total = 0;
    ensure!(
        resource(&mut archive, "mimetype", 20, &mut total)? == b"application/epub+zip",
        "Invalid EPUB mimetype"
    );
    let package = package_path(&resource(
        &mut archive,
        "META-INF/container.xml",
        65536,
        &mut total,
    )?)?;
    let sections = spine(
        &resource(&mut archive, &package, 65536, &mut total)?,
        &package,
    )?;
    let (mut units, mut textless_sections, mut text_bytes) = (Vec::new(), Vec::new(), 0);
    for section in sections {
        let text = body_text(&resource(&mut archive, &section, 1024 * 1024, &mut total)?)?;
        text_bytes += text.len();
        ensure!(
            text_bytes <= TEXT_BYTES,
            "EPUB normalized text exceeds bound"
        );
        if text.is_empty() {
            textless_sections.push(section.clone());
        }
        units.push(SourceUnit {
            page: None,
            section: Some(section),
            text,
        });
    }
    ensure!(text_bytes > 0, "EPUB has no usable body text");
    let document = NormalizedDocument {
        document,
        format: SourceFormat::Epub,
        source_sha256: digest(&bytes),
        extractor_revision: "epub-xml-text-v1".to_owned(),
        units,
    };
    Index::build(document.clone())?;
    Ok(EpubText {
        document,
        textless_sections,
    })
}

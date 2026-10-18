//! Original synthetic EPUB fixtures; no copyrighted books, network or tablet.
use remarkable_reader_buddy::{retrieval::{epub, Index, Provider, SourceFormat}, storage::Uuid};
use std::io::{Cursor, Read, Write};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

const CONTAINER: &str = r#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="book/package.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
fn xhtml(body: &str) -> String {
    format!(r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Not body evidence</title></head><body>{body}</body></html>"#)
}
fn package(items: &str, order: &str) -> String {
    format!(r#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata/><manifest>{items}</manifest><spine>{order}</spine></package>"#)
}
fn book(container: &str, opf: &str, content: &[(&str, String)]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("mimetype", SimpleFileOptions::default().compression_method(CompressionMethod::Stored)).unwrap();
    writer.write_all(b"application/epub+zip").unwrap();
    for (name, bytes) in [("META-INF/container.xml", container), ("book/package.opf", opf)] {
        writer.start_file(name, SimpleFileOptions::default().compression_method(CompressionMethod::Deflated)).unwrap();
        writer.write_all(bytes.as_bytes()).unwrap();
    }
    for (name, bytes) in content {
        writer.start_file(*name, SimpleFileOptions::default().compression_method(CompressionMethod::Deflated)).unwrap();
        writer.write_all(bytes.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
fn ordinary(body: &str) -> Vec<u8> {
    book(CONTAINER, &package(r#"<item id="a" href="chapter.xhtml" media-type="application/xhtml+xml"/>"#, r#"<itemref idref="a"/>"#), &[("book/chapter.xhtml", xhtml(body))])
}
fn normalize(bytes: &[u8]) -> anyhow::Result<epub::EpubText> {
    epub::normalize(Uuid::from_u128(1), bytes)
}

#[test]
fn actual_spine_order_auxiliary_textless_sections_and_index_provenance() {
    let opf = package(r#"<item id="b" href="two.xhtml" media-type="application/xhtml+xml"/><item id="cover" href="cover.xhtml" media-type="application/xhtml+xml"/><item id="a" href="one.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="a"/><itemref idref="cover" linear="no"/><itemref idref="b" linear="no"/>"#);
    let bytes = book(CONTAINER, &opf, &[("book/two.xhtml", xhtml("<p>Dana checks units.</p>")),
        ("book/cover.xhtml", xhtml("<img src='cover.png'/>")),
        ("book/one.xhtml", xhtml("<p>Mass 2 kg; acceleration 3 m/s². F = 6 N.</p>"))]);
    let result = normalize(&bytes).unwrap();
    assert_eq!(result.document.format, SourceFormat::Epub);
    assert_eq!(result.document.units.iter().map(|u| u.section.as_deref().unwrap()).collect::<Vec<_>>(), ["book/one.xhtml", "book/cover.xhtml", "book/two.xhtml"]);
    assert!(result.document.units.iter().all(|u| u.page.is_none()));
    assert_eq!(result.textless_sections, ["book/cover.xhtml"]);
    let index = Index::build(result.document).unwrap();
    let found = index.query(index.key(), "Dana units", 2, 4096, false).unwrap();
    assert_eq!(found.passages[0].section.as_deref(), Some("book/two.xhtml"));
    assert_eq!(found.passages[0].text, "Dana checks units.");
}

#[test]
fn text_entities_unicode_cdata_inline_and_blocks_are_normalized_without_script() {
    let result = normalize(&ordinary("<p>acceler<span>ation</span> = 3 m/s² &amp; &#x3b1;.</p><script>hidden()</script><style>secret</style><p><![CDATA[F < 7 N]]></p>")).unwrap();
    assert_eq!(result.document.units[0].text, "acceleration = 3 m/s² & α.\nF < 7 N");
    assert!(result.textless_sections.is_empty());
}

#[test]
fn original_input_unchanged_and_changed_content_changes_source_and_index_key() {
    let bytes = ordinary("<p>alpha</p>");
    let saved = bytes.clone();
    let a = normalize(&bytes).unwrap().document;
    assert_eq!(bytes, saved);
    let b = normalize(&ordinary("<p>beta</p>")).unwrap().document;
    assert_ne!(a.source_sha256, b.source_sha256);
    assert_ne!(Index::build(a).unwrap().key(), Index::build(b).unwrap().key());
}

#[test]
fn malformed_missing_unresolved_and_unsupported_inputs_never_return_partial_success() {
    let cases = [
        ordinary("<p>not closed"),
        ordinary("<p>&custom;</p>"),
        ordinary("<img src='only-image.png'/>"),
        book(CONTAINER, &package("", r#"<itemref idref="missing"/>"#), &[]),
        book(CONTAINER, &package(r#"<item id="a" href="missing.xhtml" media-type="application/xhtml+xml"/>"#, r#"<itemref idref="a"/>"#), &[]),
        book(CONTAINER, &package(r#"<item id="a" href="a.svg" media-type="image/svg+xml"/>"#, r#"<itemref idref="a"/>"#), &[("book/a.svg", "<svg/>".into())]),
        book(CONTAINER, &package(r#"<item id="a" href="https://example.invalid/a.xhtml" media-type="application/xhtml+xml"/>"#, r#"<itemref idref="a"/>"#), &[]),
        book(CONTAINER, &package(r#"<item id="a" href="../../a.xhtml" media-type="application/xhtml+xml"/>"#, r#"<itemref idref="a"/>"#), &[]),
        book(CONTAINER, &package(r#"<item id="a" href="a.xhtml" media-type="application/xhtml+xml"/>"#, r#"<itemref idref="a"/><itemref idref="a"/>"#), &[("book/a.xhtml", xhtml("good"))]),
        book(CONTAINER, &package(r#"<item id="a" href="a.xhtml" media-type="application/xhtml+xml"/>"#, r#"<itemref idref="a" linear="no"/>"#), &[("book/a.xhtml", xhtml("good"))]),
    ];
    for (i, bytes) in cases.iter().enumerate() { assert!(normalize(bytes).is_err(), "case{i}"); }
    assert!(normalize(b"not a ZIP archive").is_err());
}

#[test]
fn xml_dtd_encoding_depth_and_decoded_text_limits_refuse() {
    for body in [format!("{}text{}", "<div>".repeat(65), "</div>".repeat(65)),
        format!("<p>{}</p>", "a".repeat(512 * 1024 + 1))] {
        assert!(normalize(&ordinary(&body)).is_err());
    }
    let opf = package(r#"<item id="a" href="a.xhtml" media-type="application/xhtml+xml"/>"#, r#"<itemref idref="a"/>"#);
    for content in [format!("<!DOCTYPE html>{}", xhtml("good")),
        format!("<?xml version='1.0' encoding='ISO-8859-1'?>{}", xhtml("good")),
        xhtml(&format!("<p>{}</p>", " ".repeat(1024 * 1024)))] {
        assert!(normalize(&book(CONTAINER, &opf, &[("book/a.xhtml", content)])).is_err());
    }
}

#[test]
fn percent_encoded_paths_default_rendition_and_namespace_prefixes_work() {
    let container = CONTAINER.replace("</rootfiles>", r#"<rootfile full-path="other.opf" media-type="application/oebps-package+xml"/></rootfiles>"#);
    let opf = package(r#"<item id="a" href="chapter%20one.xhtml" media-type="application/xhtml+xml"/>"#, r#"<itemref idref="a"/>"#);
    let result = normalize(&book(&container, &opf, &[("book/chapter one.xhtml", "<h:html xmlns:h='http://www.w3.org/1999/xhtml'><h:body><h:p>ready</h:p></h:body></h:html>".into())])).unwrap();
    assert_eq!(result.document.units[0].section.as_deref(), Some("book/chapter one.xhtml"));
}

#[test]
fn acquisition_uses_one_sentinel_and_does_not_read_unbounded_input() {
    struct Endless { read: usize }
    impl Read for Endless {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> { output.fill(0); self.read += output.len(); Ok(output.len()) }
    }
    let mut source = Endless { read: 0 };
    assert!(epub::normalize(Uuid::from_u128(1), &mut source).is_err());
    assert_eq!(source.read, 16 * 1024 * 1024 + 1);
}

//! Authored EPUB archive inputs shared by normalization and receiving tests.
use std::io::{Cursor, Write};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

pub const CONTAINER: &str = r#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="book/package.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
pub fn xhtml(body: &str) -> String {
    format!(
        r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Not body evidence</title></head><body>{body}</body></html>"#
    )
}
pub fn package(items: &str, order: &str) -> String {
    format!(
        r#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata/><manifest>{items}</manifest><spine>{order}</spine></package>"#
    )
}
pub fn book(container: &str, opf: &str, content: &[(&str, String)]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "mimetype",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(b"application/epub+zip").unwrap();
    for (name, bytes) in [
        ("META-INF/container.xml", container),
        ("book/package.opf", opf),
    ] {
        writer
            .start_file(
                name,
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(bytes.as_bytes()).unwrap();
    }
    for (name, bytes) in content {
        writer
            .start_file(
                *name,
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(bytes.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

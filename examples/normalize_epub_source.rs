//! Development-only offline EPUB text ingestion; no Reader, Store or provider.
use anyhow::{ensure, Context, Result};
use remarkable_reader_buddy::{retrieval::epub, storage::Uuid};
use std::{fs::File, io::{self, Write}};

fn main() -> Result<()> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    ensure!(arguments.len() == 2, "Usage: normalize_epub_source DOCUMENT_UUID EPUB_FILE");
    let document = Uuid::parse_str(arguments[0].to_str().context("Invalid document UUID")?)?;
    let source = File::open(&arguments[1]).context("Cannot open EPUB source")?;
    ensure!(source.metadata()?.is_file(), "EPUB source must be a regular file");
    let normalized = epub::normalize(document, source)?;
    let output = serde_json::to_vec(&normalized)?;
    ensure!(output.len() <= 32 * 1024 * 1024, "EPUB output exceeds bound");
    let mut stdout = io::stdout().lock();
    stdout.write_all(&output)?;
    stdout.write_all(b"\n")?;
    Ok(())
}

//! Development-only PDF text ingestion; no Reader, Store, provider or device.
use anyhow::{ensure, Context, Result};
use remarkable_reader_buddy::{retrieval::pdf, storage::Uuid};
use std::{
    fs::File,
    io::{self, Write},
};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    ensure!(
        args.len() == 2,
        "Usage: normalize_pdf_source DOCUMENT_UUID PDF_FILE"
    );
    let document = Uuid::parse_str(args[0].to_str().context("Invalid document UUID")?)?;
    let source = File::open(&args[1]).context("Cannot open PDF source")?;
    ensure!(
        source.metadata()?.is_file(),
        "PDF source must be a regular file"
    );
    let normalized = pdf::normalize(document, source)?;
    let output = serde_json::to_vec(&normalized)?;
    ensure!(output.len() <= 32 * 1024 * 1024, "PDF output exceeds bound");
    let mut stdout = io::stdout().lock();
    stdout.write_all(&output)?;
    stdout.write_all(b"\n")?;
    Ok(())
}

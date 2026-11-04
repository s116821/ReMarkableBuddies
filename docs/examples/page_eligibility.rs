//! Read-only classifier evidence; no input devices, model or credential access.
//! Native metadata is an observed candidate, not independent visual UI proof.
use anyhow::{ensure, Context, Result};
use remarkable_reader_buddy::{device::native_page, Screenshot, Workflow};
use std::{fs, io::Read, path::Path};

const HEADER: &str = "/var/cache/reader-buddy/header-pattern.png";

fn optional_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() <= 8 * 1024 * 1024,
                "Diagnostic image exceeds8MiB"
            );
            Ok(Some(bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        matches!(args.as_slice(), [mode, _] if mode == "native")
            || matches!(args.as_slice(), [mode, _, _, _] if mode == "offline"),
        "Usage: page_eligibility native NEW_OUTPUT_DIR | offline IMAGE REFERENCE_OR_DASH NEW_OUTPUT_DIR"
    );
    let output = Path::new(args.last().unwrap());
    ensure!(
        !output.exists(),
        "Output directory must be new; preserve earlier evidence"
    );
    let (png, reference, owner) = if args[0] == "native" {
        let proc = Path::new("/proc");
        let root = Path::new("/home/root/.local/share/remarkable/xochitl");
        let settings = Path::new("/home/root/.config/remarkable/xochitl.conf");
        let session = native_page::xochitl_session(proc)?;
        let before = native_page::observed_owner(root, settings, session.clone())?;
        let reference = optional_bytes(Path::new(HEADER))?;
        let mut screenshot = Screenshot::new()?;
        screenshot.take_screenshot()?;
        ensure!(
            session == native_page::xochitl_session(proc)?,
            "Session changed during capture"
        );
        ensure!(
            before == native_page::observed_owner(root, settings, session)?,
            "Page identity changed during capture"
        );
        ensure!(
            reference == optional_bytes(Path::new(HEADER))?,
            "Header reference changed during capture"
        );
        let owner = serde_json::json!({
            "document": before.document, "page": before.page,
            "visit": before.visit, "session": before.session,
            "meaning": "Identity bracketing matched; confirm the visible UI independently"
        });
        (screenshot.get_image_data().to_vec(), reference, owner)
    } else {
        let reference = if args[2] == "-" {
            None
        } else {
            optional_bytes(Path::new(&args[2]))?
        };
        (
            optional_bytes(Path::new(&args[1]))?.context("Input image missing")?,
            reference,
            serde_json::Value::Null,
        )
    };
    let image = image::load_from_memory(&png).context("Decode classifier input")?;
    ensure!(
        (image.width(), image.height()) == (768, 1024),
        "Expected normalized classifier input"
    );
    // Match production's absent/undecodable-cache fallback while retaining bytes
    // and the failure category instead of hiding why no reference was available.
    let decoded = reference
        .as_ref()
        .map(|bytes| image::load_from_memory(bytes));
    let saved = decoded.as_ref().and_then(|result| result.as_ref().ok());
    let classification = Workflow::classify_answer_page(&image, saved);
    let report = serde_json::json!({
        "version": env!("READER_BUDDY_VERSION"), "mode": args[0],
        "classification": format!("{classification:?}"), "owner_observation": owner,
        "input_dimensions": [image.width(), image.height()],
        "reference_present": reference.is_some(),
        "reference_decoded": saved.is_some(),
        "reference_dimensions": saved.map(|image| [image.width(), image.height()]),
        "meaning": "Production classifier result only; no model, navigation, rendering or eligibility threshold change"
    });
    fs::create_dir(output)?;
    fs::write(output.join("input.png"), png)?;
    if let Some(reference) = reference {
        fs::write(output.join("reference.png"), reference)?;
    }
    let report = serde_json::to_string_pretty(&report)?;
    fs::write(output.join("report.json"), &report)?;
    println!("{report}");
    Ok(())
}

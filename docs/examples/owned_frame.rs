//! Pure offline conversion of a bound experimental frame, never native authority.
//! Parent must preserve successful producer exit and closed-frame SHA256 first.
//! Accept outputs only on this example's exit zero and complete receipt. Failed
//! output I/O may leave an incomplete fresh directory for owner inspection.
use anyhow::{Context, Result};
use remarkable_reader_buddy::device::screenshot::{CaptureIdentity, OWNED_RM2_FRAME_BYTES};
use remarkable_reader_buddy::Screenshot;
use sha2::{Digest, Sha256};
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    anyhow::ensure!(
        args.len() == 7,
        "Usage: owned_frame INPUT NEW_OUTPUT_DIR NONCE PID START_TICKS BOOT_ID FRAME_SHA256"
    );
    let text = |i: usize| args[i].to_str().context("Argument must be UTF-8");
    let nonce = text(2)?;
    let identity = CaptureIdentity::new(text(3)?.parse()?, text(4)?.parse()?, text(5)?.into())?;
    let expected_hash = text(6)?;
    anyhow::ensure!(
        expected_hash.len() == 64
            && expected_hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "Invalid selected frame SHA256"
    );
    let mut frame = Vec::new();
    File::open(&args[0])?
        .take(OWNED_RM2_FRAME_BYTES as u64 + 1)
        .read_to_end(&mut frame)?;
    let actual_hash = format!("{:x}", Sha256::digest(&frame));
    anyhow::ensure!(actual_hash == expected_hash, "Closed frame SHA256 mismatch");
    let pngs = Screenshot::encode_owned_rm2_rgb32(&frame, nonce, &identity)?;
    // Complete parsing and both encodings precede any output publication.
    let output = Path::new(&args[1]);
    #[cfg(unix)]
    let mut directory = DirBuilder::new();
    #[cfg(not(unix))]
    let directory = DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory
        .create(output)
        .context("Output directory must be fresh")?;
    for (name, bytes) in [
        ("native.png", &pngs.native),
        ("overview.png", &pngs.overview),
    ] {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(output.join(name))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    println!("nonce={nonce} frame_sha256={actual_hash} native_sha256={:x} overview_sha256={:x} conversion_complete=true native_authority=false",
        Sha256::digest(&pngs.native), Sha256::digest(&pngs.overview));
    Ok(())
}

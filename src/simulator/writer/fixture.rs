//! Optional closed synthetic Store evidence, never an execution/restore grant.
use crate::storage::{digest, files};
use anyhow::{ensure, Result};
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

const MAX_FILES: usize = 8192;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
pub(super) fn destination(root: &Path, output: &Path) -> Result<PathBuf> {
    let joined = std::env::current_dir()?
        .join(root)
        .join(output)
        .join("store-fixture");
    let mut path = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::ParentDir => ensure!(path.pop(), "fixture output escapes root"),
            Component::CurDir => (),
            _ => path.push(part.as_os_str()),
        }
    }
    files::safe_path(&path)?;
    ensure!(!path.exists(), "fixture output already exists");
    Ok(path)
}
fn inventory(
    root: &Path,
    current: &Path,
    entries: &mut Vec<(PathBuf, u64)>,
    total: &mut u64,
    depth: usize,
) -> Result<()> {
    ensure!(depth <= 12, "fixture directory depth exceeded");
    for entry in fs::read_dir(current)? {
        let path = entry?.path();
        let meta = fs::symlink_metadata(&path)?;
        ensure!(!meta.file_type().is_symlink(), "fixture symlink refused");
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            ensure!(
                meta.file_attributes() & 0x400 == 0,
                "fixture reparse point refused"
            );
        }
        if meta.is_dir() {
            inventory(root, &path, entries, total, depth + 1)?;
        } else {
            ensure!(meta.is_file(), "fixture special file refused");
            *total = total
                .checked_add(meta.len())
                .ok_or_else(|| anyhow::anyhow!("fixture byte overflow"))?;
            ensure!(
                *total <= MAX_BYTES && entries.len() < MAX_FILES,
                "fixture size bound exceeded"
            );
            entries.push((path.strip_prefix(root)?.to_path_buf(), meta.len()));
        }
    }
    Ok(())
}
pub(super) fn save(source: &Path, destination: &Path) -> Result<()> {
    files::safe_path(source)?;
    files::safe_path(destination)?;
    ensure!(
        !destination.starts_with(source) && !source.starts_with(destination),
        "fixture paths overlap"
    );
    ensure!(!destination.exists(), "fixture output already exists");
    let mut entries = Vec::new();
    let mut total = 0;
    inventory(source, source, &mut entries, &mut total, 0)?;
    entries.sort();
    fs::create_dir_all(
        destination
            .parent()
            .ok_or_else(|| anyhow::anyhow!("fixture parent absent"))?,
    )?;
    fs::create_dir(destination)?;
    let data = destination.join("data");
    fs::create_dir(&data)?;
    let mut manifest = Vec::new();
    for (relative, size) in entries {
        let original = source.join(&relative);
        files::safe_path(&original)?;
        let meta = fs::symlink_metadata(&original)?;
        ensure!(
            meta.is_file() && meta.len() == size,
            "closed fixture changed"
        );
        let mut bytes = Vec::new();
        fs::File::open(&original)?
            .take(size + 1)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() as u64 == size, "closed fixture changed");
        let target = data.join(&relative);
        fs::create_dir_all(
            target
                .parent()
                .ok_or_else(|| anyhow::anyhow!("fixture member parent absent"))?,
        )?;
        files::safe_path(&target)?;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?
            .write_all(&bytes)?;
        ensure!(
            digest(&fs::read(&target)?) == digest(&bytes),
            "fixture copy verification failed"
        );
        manifest.push(serde_json::json!({"path":relative.to_string_lossy().replace('\\',"/"),"bytes":size,"sha256":digest(&bytes)}));
    }
    fs::OpenOptions::new().write(true).create_new(true).open(destination.join("manifest.json"))?.write_all(&serde_json::to_vec_pretty(&serde_json::json!({"format":1,"kind":"closed-scripted-store-fixture","bytes":total,"files":manifest}))?)?;
    Ok(())
}

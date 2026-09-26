//! Small, ownership-checked filesystem primitives. Never follow stored paths.
use super::types::{digest, Uuid};
use anyhow::{ensure, Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path},
};

pub fn safe_path(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "owned path must be absolute");
    ensure!(path.components().count() >= 3, "refusing filesystem root");
    for component in path.components() {
        ensure!(
            !matches!(component, Component::ParentDir | Component::CurDir),
            "relative path component refused"
        );
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => ensure!(!meta.file_type().is_symlink(), "symlink path refused"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => anyhow::bail!("cannot validate owned path"),
        }
    }
    for forbidden in [
        "/dev",
        "/proc",
        "/sys",
        "/etc",
        "/usr",
        "/bin",
        "/sbin",
        "/opt",
        "/boot",
        "/home/root/.local/share/remarkable/xochitl",
    ] {
        ensure!(!path.starts_with(forbidden), "protected path refused");
    }
    Ok(())
}
pub fn directory(path: &Path) -> Result<()> {
    safe_path(path)?;
    fs::create_dir_all(path).context("create owned directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
pub fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path; // Windows host fixtures do not prove directory durability on Linux.
    Ok(())
}
pub fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    safe_path(path)?;
    ensure!(
        fs::symlink_metadata(path)?.is_file(),
        "expected regular file"
    );
    let file = File::open(path).context("open stored file")?;
    ensure!(file.metadata()?.len() <= limit, "stored file exceeds bound");
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "stored file grew past bound");
    Ok(bytes)
}
pub fn json<T: DeserializeOwned>(path: &Path, limit: u64) -> Result<T> {
    serde_json::from_slice(&read(path, limit)?)
        .map_err(|_| anyhow::anyhow!("invalid or unsupported stored JSON"))
}
pub fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    safe_path(path)?;
    let parent = path.parent().context("missing owned parent")?;
    let tmp = parent.join(format!(".stage-{}", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path).context("atomic publication failed")?;
    sync_dir(parent)
}
pub fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    atomic(path, &serde_json::to_vec(value)?)
}
pub fn create_identity<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    safe_path(path)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // Never replace another initializer's identity. An interrupted first write
    // fails closed on reopening instead of adopting or resetting populated data.
    let mut file = options
        .open(path)
        .context("store identity already created or unavailable")?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    sync_dir(path.parent().context("identity parent missing")?)
}
pub fn object(path: &Path, expected: &str, bytes: &[u8]) -> Result<()> {
    ensure!(digest(bytes) == expected, "object hash mismatch");
    if path.exists() {
        ensure!(
            read(path, bytes.len() as u64)? == bytes,
            "stored object collision"
        );
        return Ok(());
    }
    atomic(path, bytes)
}

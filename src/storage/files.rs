//! Small, ownership-checked filesystem primitives. Never follow stored paths.
use super::types::{digest, valid_digest, ObjectRef, Uuid, MAX_MEDIA};
use anyhow::{ensure, Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
};

pub enum Source {
    Memory(Vec<u8>),
    File { file: File, reference: ObjectRef },
}
impl Source {
    pub fn reference(&self) -> ObjectRef {
        match self {
            Self::Memory(bytes) => ObjectRef {
                sha256: digest(bytes),
                bytes: bytes.len() as u64,
            },
            Self::File { reference, .. } => reference.clone(),
        }
    }
    pub fn len(&self) -> usize {
        match self {
            Self::Memory(b) => b.len(),
            Self::File { reference, .. } => reference.bytes as usize,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn chunk(&mut self, offset: usize, length: usize) -> Result<Vec<u8>> {
        ensure!(
            offset <= self.len() && length <= self.len() - offset,
            "source chunk out of bounds"
        );
        match self {
            Self::Memory(bytes) => Ok(bytes[offset..offset + length].to_vec()),
            Self::File { file, .. } => {
                file.seek(SeekFrom::Start(offset as u64))?;
                let mut bytes = vec![0; length];
                file.read_exact(&mut bytes)?;
                Ok(bytes)
            }
        }
    }
    pub fn open(path: &Path, reference: &ObjectRef) -> Result<Self> {
        safe_path(path)?;
        ensure!(
            fs::symlink_metadata(path)?.is_file(),
            "expected regular object"
        );
        let mut file = File::open(path)?;
        verify_reader(&mut file, reference)?;
        file.seek(SeekFrom::Start(0))?;
        Ok(Self::File {
            file,
            reference: reference.clone(),
        })
    }
}
pub fn verify_reader(mut reader: impl Read, reference: &ObjectRef) -> Result<()> {
    ensure!(
        valid_digest(&reference.sha256) && reference.bytes <= MAX_MEDIA,
        "invalid streamed reference"
    );
    let mut total = 0_u64;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        ensure!(total <= reference.bytes, "stream exceeds declared size");
        hash.update(&buffer[..n]);
    }
    ensure!(
        total == reference.bytes && format!("{:x}", hash.finalize()) == reference.sha256,
        "stream integrity failure"
    );
    Ok(())
}
pub fn copy_verified(path: &Path, mut reader: impl Read, reference: &ObjectRef) -> Result<()> {
    safe_path(path)?;
    ensure!(
        valid_digest(&reference.sha256) && reference.bytes <= MAX_MEDIA,
        "invalid streamed reference"
    );
    if path.exists() {
        let _ = Source::open(path, reference)?;
        return Ok(());
    }
    atomic_with(path, |file| {
        let mut total = 0_u64;
        let mut hash = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            ensure!(total <= reference.bytes, "stream exceeds declared size");
            hash.update(&buffer[..n]);
            file.write_all(&buffer[..n])?;
        }
        ensure!(
            total == reference.bytes && format!("{:x}", hash.finalize()) == reference.sha256,
            "stream integrity failure"
        );
        Ok(())
    })
}

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
            Ok(meta) => {
                ensure!(!meta.file_type().is_symlink(), "symlink path refused");
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    ensure!(
                        meta.file_attributes() & 0x400 == 0,
                        "reparse-point path refused"
                    );
                }
            }
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
    directory_with_sync(path, sync_dir)
}
fn directory_with_sync(path: &Path, mut sync: impl FnMut(&Path) -> Result<()>) -> Result<()> {
    safe_path(path)?;
    let mut missing = Vec::new();
    for ancestor in path.ancestors() {
        if ancestor.exists() {
            break;
        }
        missing.push(ancestor.to_path_buf());
    }
    for next in missing.iter().rev() {
        fs::create_dir(next).context("create owned directory")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(next, fs::Permissions::from_mode(0o700))?;
        }
        sync(next)?;
        sync(next.parent().context("owned directory parent missing")?)?;
    }
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
    atomic_with(path, |file| Ok(file.write_all(bytes)?))
}
struct Temporary {
    path: PathBuf,
    file: Option<File>,
}
impl Drop for Temporary {
    fn drop(&mut self) {
        drop(self.file.take());
        // Only this attempt's create_new file; never remove the destination.
        let _ = fs::remove_file(&self.path);
    }
}
fn atomic_with(path: &Path, write: impl FnOnce(&mut File) -> Result<()>) -> Result<()> {
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
    let file = options.open(&tmp)?;
    let mut temporary = Temporary {
        path: tmp,
        file: Some(file),
    };
    let file = temporary.file.as_mut().unwrap();
    write(file)?;
    file.sync_all()?;
    drop(temporary.file.take());
    fs::rename(&temporary.path, path).context("atomic publication failed")?;
    sync_dir(parent)
}

/// Caller holds the store lease and has validated this directory's ownership.
/// Interrupted staging has no commit authority; retain all other files.
pub(crate) fn cleanup_staging(directory: &Path) -> Result<()> {
    safe_path(directory)?;
    if !directory.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|n| n.strip_prefix(".stage-")) else {
            continue;
        };
        if Uuid::parse_str(id).is_ok() {
            safe_path(&entry.path())?;
            ensure!(entry.file_type()?.is_file(), "invalid staging entry");
            fs::remove_file(entry.path())?;
        }
    }
    sync_dir(directory)
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_writes_clean_only_attempt_staging_and_keep_published_bytes() {
        let root = std::env::temp_dir().join(format!("buddy-stage-cleanup-{}", Uuid::new_v4()));
        directory(&root).unwrap();
        let target = root.join("published");
        atomic(&target, b"prior").unwrap();
        for _ in 0..4 {
            let error = atomic_with(&target, |file| {
                file.write_all(b"partial")?;
                Err(std::io::Error::from(std::io::ErrorKind::StorageFull).into())
            })
            .unwrap_err();
            assert_eq!(
                error.downcast_ref::<std::io::Error>().unwrap().kind(),
                std::io::ErrorKind::StorageFull
            );
            assert_eq!(fs::read(&target).unwrap(), b"prior");
            let reference = ObjectRef {
                sha256: digest(b"complete"),
                bytes: 8,
            };
            assert!(copy_verified(&root.join("object"), &b"short"[..], &reference).is_err());
            assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        }
        let interrupted = root.join(format!(".stage-{}", Uuid::new_v4()));
        fs::write(&interrupted, b"interrupted").unwrap();
        fs::write(root.join(".stage-not-an-owned-uuid"), b"retain").unwrap();
        cleanup_staging(&root).unwrap();
        assert!(!interrupted.exists());
        assert_eq!(fs::read(&target).unwrap(), b"prior");
        assert!(root.join(".stage-not-an-owned-uuid").exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn new_directory_entries_are_synced_in_ancestor_order_and_failure_propagates() {
        let root =
            std::env::temp_dir().join(format!("buddy-directory-durability-{}", Uuid::new_v4()));
        let target = root.join("generations").join("next").join("objects");
        let mut calls = Vec::new();
        directory_with_sync(&target, |p| {
            calls.push(p.to_path_buf());
            Ok(())
        })
        .unwrap();
        for pair in calls.as_chunks::<2>().0 {
            assert_eq!(pair[0].parent(), Some(pair[1].as_path()));
        }
        assert_eq!(calls[calls.len() - 2], target);
        let failed = root.join("failure").join("not-created");
        let mut count = 0;
        assert!(directory_with_sync(&failed, |_| {
            count += 1;
            if count == 2 {
                anyhow::bail!("injected parent-sync failure");
            }
            Ok(())
        })
        .is_err());
        assert!(!failed.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}

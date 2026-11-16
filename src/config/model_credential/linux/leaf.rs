//! Checked Linux leaf I/O shared by the two fixed credential writers.
use super::{metadata_hash, owner, parent, private_mode, Checked, Phase, Status};
use crate::storage::{files, Uuid};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub(crate) struct Leaf {
    pub bytes: Vec<u8>,
    pub metadata_sha256: String,
    pub mode: u32,
}

pub(crate) fn parent_identity(path: &Path) -> Checked<String> {
    Ok(metadata_hash(&parent(path)?, false))
}

pub(crate) fn observe(
    target: &Path,
    config_meta: &fs::Metadata,
    limit: usize,
) -> Checked<Option<Leaf>> {
    let meta = match fs::symlink_metadata(target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Status::Unavailable),
        Ok(meta) => meta,
    };
    let mode = meta.mode() & 0o7777;
    if !meta.is_file()
        || meta.nlink() != 1
        || !owner(&meta)
        || !private_mode(mode)
        || (meta.dev() == config_meta.dev() && meta.ino() == config_meta.ino())
    {
        return Err(Status::Unsupported);
    }
    if meta.len() > limit as u64 {
        return Err(Status::Unavailable);
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(target)
        .map_err(|_| Status::Unavailable)?;
    let identity = metadata_hash(&meta, true);
    if metadata_hash(&file.metadata().map_err(|_| Status::Unavailable)?, true) != identity {
        return Err(Status::Unavailable);
    }
    let mut bytes = Vec::new();
    (&file)
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| Status::Unavailable)?;
    if bytes.len() != meta.len() as usize
        || metadata_hash(&file.metadata().map_err(|_| Status::Unavailable)?, true) != identity
        || metadata_hash(
            &fs::symlink_metadata(target).map_err(|_| Status::Unavailable)?,
            true,
        ) != identity
    {
        return Err(Status::Unavailable);
    }
    Ok(Some(Leaf {
        bytes,
        metadata_sha256: identity,
        mode,
    }))
}

pub(crate) enum Mutation<'a> {
    Create(&'a [u8], u32),
    Replace(&'a [u8], u32),
    Remove,
}

pub(crate) fn publish(
    target: &Path,
    mutation: Mutation<'_>,
    revalidate: impl FnOnce() -> Checked<()>,
    mut hook: impl FnMut(Phase) -> Checked<()>,
    attempted: &mut bool,
) -> Checked<()> {
    let root = target.parent().ok_or(Status::Unavailable)?;
    let stage = match mutation {
        Mutation::Create(bytes, mode) | Mutation::Replace(bytes, mode) => {
            Some(Stage::create(root, bytes, mode)?)
        }
        Mutation::Remove => None,
    };
    hook(Phase::Staged)?;
    revalidate()?;
    hook(Phase::BeforeSyscall)?;
    if let Some(stage) = &stage {
        stage.check_sealed()?;
    }
    *attempted = true;
    match (mutation, stage.as_ref()) {
        (Mutation::Create(..), Some(stage)) => {
            fs::hard_link(&stage.path, target).map_err(|_| Status::Unavailable)?;
            stage.cleanup()?;
        }
        (Mutation::Replace(..), Some(stage)) => {
            fs::rename(&stage.path, target).map_err(|_| Status::Unavailable)?;
        }
        (Mutation::Remove, None) => {
            fs::remove_file(target).map_err(|_| Status::Unavailable)?;
        }
        _ => return Err(Status::InvalidInput),
    }
    hook(Phase::AfterSyscall)?;
    hook(Phase::BeforeSync)?;
    files::sync_dir(root).map_err(|_| Status::Unavailable)?;
    hook(Phase::BeforeReadback)
}

struct Stage {
    path: PathBuf,
    parent_identity: String,
    identity: (u64, u64),
    sealed_metadata: String,
}
impl Stage {
    fn create(root: &Path, bytes: &[u8], mode: u32) -> Checked<Self> {
        if !private_mode(mode) {
            return Err(Status::Unsupported);
        }
        let path = root.join(format!(".credential-stage-{}", Uuid::new_v4()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|_| Status::Unavailable)?;
        let meta = file.metadata().map_err(|_| Status::Unavailable)?;
        let mut stage = Self {
            path,
            parent_identity: metadata_hash(&parent(root)?, false),
            identity: (meta.dev(), meta.ino()),
            sealed_metadata: String::new(),
        };
        let mut file = file;
        file.write_all(bytes).map_err(|_| Status::Unavailable)?;
        file.set_permissions(fs::Permissions::from_mode(mode))
            .map_err(|_| Status::Unavailable)?;
        file.sync_all().map_err(|_| Status::Unavailable)?;
        let meta = file.metadata().map_err(|_| Status::Unavailable)?;
        if !meta.is_file() || !owner(&meta) || meta.nlink() != 1 || meta.mode() & 0o7777 != mode {
            return Err(Status::Unsupported);
        }
        stage.sealed_metadata = metadata_hash(&meta, true);
        Ok(stage)
    }
    fn check_sealed(&self) -> Checked<()> {
        let root = self.path.parent().ok_or(Status::Unavailable)?;
        if metadata_hash(&parent(root)?, false) != self.parent_identity {
            return Err(Status::Unavailable);
        }
        let meta = fs::symlink_metadata(&self.path).map_err(|_| Status::Unavailable)?;
        if !meta.is_file() || metadata_hash(&meta, true) != self.sealed_metadata {
            return Err(Status::Unsupported);
        }
        Ok(())
    }
    fn cleanup(&self) -> Checked<()> {
        let root = self.path.parent().ok_or(Status::Unavailable)?;
        if metadata_hash(&parent(root)?, false) != self.parent_identity {
            return Err(Status::Unavailable);
        }
        match fs::symlink_metadata(&self.path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Ok(m) if m.is_file() && (m.dev(), m.ino()) == self.identity => {
                fs::remove_file(&self.path).map_err(|_| Status::Unavailable)
            }
            _ => Err(Status::Unavailable),
        }
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

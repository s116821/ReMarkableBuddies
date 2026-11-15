use super::{Action, State};
use crate::{
    config::{inspection, Config},
    storage::{StorePaths, MAX_RECORD},
};
use anyhow::{ensure, Context, Result};
use nix::libc;
use std::{
    ffi::{CStr, CString},
    fs::{self, File, Metadata, OpenOptions, Permissions},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

#[cfg(test)]
mod tests;

const DIRECTORY: libc::c_int = libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_RDONLY;
const ENTRY: libc::c_int = libc::O_NOFOLLOW | libc::O_PATH;

pub(super) fn run(action: Action, config: &Path) -> State {
    // Production anchors the complete path from / and only admits root.
    if unsafe { libc::geteuid() } != 0 {
        return State::Blocked;
    }
    Tree::open(Path::new("/"), Path::new("/home/root"), config)
        .and_then(|tree| attempt(&tree, action, &Config::default()))
        .unwrap_or(State::Blocked)
}

fn open_at(parent: &File, name: &CStr, flags: libc::c_int, mode: libc::mode_t) -> Result<File> {
    // All names are single, NUL-terminated components. Returned descriptors
    // are owned by File and never inherited.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC,
            mode,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn optional<T>(result: Result<T>) -> Result<Option<T>> {
    match result {
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|e| e.kind() == io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        other => other.map(Some),
    }
}

fn trusted(meta: &Metadata, uid: u32) -> bool {
    meta.uid() == uid && meta.mode() & 0o022 == 0
}

fn same_inode(left: &Metadata, right: &Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn same_version(left: &Metadata, right: &Metadata) -> bool {
    same_inode(left, right)
        && left.len() == right.len()
        && left.uid() == right.uid()
        && left.mode() == right.mode()
        && left.nlink() == right.nlink()
        && (left.mtime(), left.mtime_nsec()) == (right.mtime(), right.mtime_nsec())
        && (left.ctime(), left.ctime_nsec()) == (right.ctime(), right.ctime_nsec())
}

fn descriptor_path(file: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

struct Tree<'a> {
    base: &'a Path,
    home: &'a Path,
    config: &'a Path,
    uid: u32,
    base_dir: File,
}

impl<'a> Tree<'a> {
    fn open(base: &'a Path, home: &'a Path, config: &'a Path) -> Result<Self> {
        let uid = unsafe { libc::geteuid() };
        let base_dir = Self::open_base(base, uid)?;
        let tree = Self {
            base,
            home,
            config,
            uid,
            base_dir,
        };
        ensure!(
            config == home.join(".config/remarkable-buddies/config.json")
                && tree.walk(home, 0)?.is_some(),
            "configuration home refused"
        );
        Ok(tree)
    }

    fn open_base(path: &Path, uid: u32) -> Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)?;
        ensure!(trusted(&file.metadata()?, uid), "untrusted directory");
        Ok(file)
    }

    // Return a pinned chain including the base. Creation is used only for the
    // two known config directories, never for the home or Store roots.
    fn walk(&self, path: &Path, create_last: usize) -> Result<Option<Vec<File>>> {
        let relative = path.strip_prefix(self.base)?;
        let mut parts = relative.components();
        let count = parts.clone().count();
        let mut chain = vec![self.base_dir.try_clone()?];
        let mut prefix = self.base.to_path_buf();
        for index in 0..count {
            let Some(Component::Normal(part)) = parts.next() else {
                anyhow::bail!("untrusted directory component");
            };
            let name = CString::new(part.as_bytes())?;
            let parent = chain.last().expect("chain includes base");
            let child = match optional(open_at(parent, &name, DIRECTORY, 0))? {
                Some(child) => child,
                None if index >= count.saturating_sub(create_last) => {
                    self.current(&prefix, &chain)?;
                    let rc = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
                    if rc != 0 {
                        let error = io::Error::last_os_error();
                        if error.kind() != io::ErrorKind::AlreadyExists {
                            return Err(error.into());
                        }
                    }
                    open_at(parent, &name, DIRECTORY, 0)?
                }
                None => return Ok(None),
            };
            ensure!(trusted(&child.metadata()?, self.uid), "untrusted directory");
            chain.push(child);
            prefix.push(part);
        }
        Ok(Some(chain))
    }

    fn parent(&self) -> &Path {
        self.config.parent().expect("fixed config has a parent")
    }

    fn current(&self, path: &Path, chain: &[File]) -> Result<()> {
        let base = Self::open_base(self.base, self.uid)?;
        ensure!(
            same_inode(&base.metadata()?, &self.base_dir.metadata()?),
            "directory anchor changed"
        );
        let fresh = self.walk(path, 0)?.context("directory disappeared")?;
        ensure!(fresh.len() == chain.len(), "directory chain changed");
        for (old, current) in chain.iter().zip(fresh) {
            ensure!(
                same_inode(&old.metadata()?, &current.metadata()?),
                "directory chain changed"
            );
        }
        Ok(())
    }

    fn check_empty_roots(&self, paths: &StorePaths) -> Result<()> {
        for path in [&paths.data, &paths.cache, &paths.credentials] {
            ensure!(
                path.starts_with(self.home) && path != self.home,
                "untrusted Store root"
            );
            if let Some(chain) = self.walk(path, 0)? {
                let dir = chain.last().expect("chain includes root");
                ensure!(
                    fs::read_dir(descriptor_path(dir))?
                        .next()
                        .transpose()?
                        .is_none(),
                    "nonempty Store root"
                );
                self.current(path, &chain)?;
            }
        }
        Ok(())
    }

    fn has_saved_config(&self) -> Result<bool> {
        let Some(chain) = self.walk(self.parent(), 0)? else {
            return Ok(false);
        };
        let parent = chain.last().expect("config parent");
        let present = self.read_saved(parent)?.is_some();
        self.current(self.parent(), &chain)?;
        Ok(present)
    }

    fn read_saved(&self, parent: &File) -> Result<Option<(Metadata, Vec<u8>)>> {
        let Some(pinned) = optional(open_at(parent, c"config.json", ENTRY, 0))? else {
            return Ok(None);
        };
        let original = pinned.metadata()?;
        ensure!(
            original.is_file()
                && trusted(&original, self.uid)
                && original.nlink() == 1
                && original.len() <= MAX_RECORD as u64,
            "untrusted saved configuration"
        );
        // Reopen the already pinned regular inode for reading. A special file
        // or a symlink substituted at the final pathname is never opened for I/O.
        let file = File::open(descriptor_path(&pinned))?;
        ensure!(
            same_version(&original, &file.metadata()?),
            "configuration changed"
        );
        let mut bytes = Vec::new();
        file.take(MAX_RECORD as u64 + 1).read_to_end(&mut bytes)?;
        let current = open_at(parent, c"config.json", ENTRY, 0)?.metadata()?;
        ensure!(
            bytes.len() as u64 == original.len() && same_version(&original, &current),
            "configuration changed"
        );
        inspection::document(&bytes).map_err(|_| anyhow::anyhow!("invalid saved configuration"))?;
        Ok(Some((current, bytes)))
    }
}

struct Stage<'a> {
    parent: &'a File,
    name: CString,
    file: File,
    published: bool,
}

impl<'a> Stage<'a> {
    fn new(parent: &'a File) -> Result<Self> {
        let name = CString::new(format!(".stage-initial-{}", Uuid::new_v4()))?;
        let file = open_at(
            parent,
            &name,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
            0o600,
        )?;
        Ok(Self {
            parent,
            name,
            file,
            published: false,
        })
    }

    fn owns_name(&self) -> Result<bool> {
        let Some(entry) = optional(open_at(self.parent, &self.name, ENTRY, 0))? else {
            return Ok(false);
        };
        Ok(same_inode(&self.file.metadata()?, &entry.metadata()?))
    }

    fn rename(&self) -> io::Result<()> {
        #[cfg(test)]
        tests::fault(tests::Fault::Rename)?;
        // One syscall in the pinned destination directory. No fallback
        // can turn a competing final entry into a replacement.
        let fd = self.parent.as_raw_fd();
        let rc = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                fd,
                self.name.as_ptr(),
                fd,
                c"config.json".as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

impl Drop for Stage<'_> {
    fn drop(&mut self) {
        if !self.published && self.owns_name().unwrap_or(false) {
            // This descriptor and the unique name belong to this attempt.
            unsafe { libc::unlinkat(self.parent.as_raw_fd(), self.name.as_ptr(), 0) };
        }
    }
}

fn attempt(tree: &Tree<'_>, action: Action, defaults: &Config) -> Result<State> {
    if tree.has_saved_config()? {
        return Ok(State::Present);
    }

    defaults.validate()?;
    tree.check_empty_roots(&defaults.paths)?;
    let bytes = serde_json::to_vec(defaults)?;
    inspection::document(&bytes).map_err(|_| anyhow::anyhow!("invalid initial configuration"))?;
    if action == Action::CheckInitial {
        return Ok(if tree.has_saved_config()? {
            State::Present
        } else {
            State::Missing
        });
    }

    let chain = tree
        .walk(tree.parent(), 2)?
        .context("config parent unavailable")?;
    let parent = chain.last().expect("config parent");
    let present = tree.read_saved(parent)?.is_some();
    tree.current(tree.parent(), &chain)?;
    if present {
        return Ok(State::Present);
    }
    let mut stage = Stage::new(parent)?;
    stage.file.set_permissions(Permissions::from_mode(0o600))?;
    stage.file.write_all(&bytes)?;
    stage.file.sync_all()?;
    let staged = stage.file.metadata()?;
    #[cfg(test)]
    tests::fault(tests::Fault::BeforePublication)?;

    tree.check_empty_roots(&defaults.paths)?;
    tree.current(tree.parent(), &chain)?;
    ensure!(
        stage.owns_name()? && same_version(&staged, &stage.file.metadata()?),
        "configuration stage changed"
    );
    match stage.rename() {
        Ok(()) => stage.published = true,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let present = tree.read_saved(parent)?.is_some();
            tree.current(tree.parent(), &chain)?;
            return Ok(if present {
                State::Present
            } else {
                State::Blocked
            });
        }
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(libc::ENOSYS | libc::EINVAL | libc::EOPNOTSUPP | libc::EXDEV)
            ) =>
        {
            return Ok(State::Blocked)
        }
        Err(_) => return Ok(State::Unknown),
    }

    let readback = || -> Result<()> {
        #[cfg(test)]
        tests::fault(tests::Fault::AfterPublication)?;
        // Sync the containing dir and both potential new links back to home,
        // even if a concurrent Check/Create observed an already-made parent.
        for dir in chain.iter().rev().take(3) {
            dir.sync_all()?;
        }
        let (identity, observed) = tree.read_saved(parent)?.context("configuration missing")?;
        ensure!(
            same_inode(&stage.file.metadata()?, &identity) && observed == bytes,
            "published configuration readback changed"
        );
        tree.current(tree.parent(), &chain)
    };
    Ok(if readback().is_ok() {
        State::Created
    } else {
        State::Unknown
    })
}

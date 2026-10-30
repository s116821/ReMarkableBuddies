//! Existing-only descriptor-relative metadata access. No Store construction.
use crate::storage::{StorePaths, MAX_ITEMS, MAX_METADATA};
use nix::libc;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CStr, CString, OsString},
    fs::{File, Metadata},
    io::{self, Read},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::MetadataExt,
        },
    },
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Failure {
    Missing,
    Unsafe,
    Unreadable,
    Changed,
    TooLarge,
    WorkLimit,
}
fn failure(error: io::Error) -> Failure {
    match error.raw_os_error() {
        Some(libc::ENOENT) => Failure::Missing,
        Some(libc::ELOOP | libc::ENOTDIR) => Failure::Unsafe,
        _ => Failure::Unreadable,
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    len: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    mode: u32,
}
impl Stamp {
    fn of(meta: &Metadata) -> Self {
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
            len: meta.len(),
            modified: (meta.mtime(), meta.mtime_nsec()),
            changed: (meta.ctime(), meta.ctime_nsec()),
            mode: meta.mode(),
        }
    }
    fn identity(self) -> (u64, u64) {
        (self.dev, self.ino)
    }
}
fn cstring(name: &std::ffi::OsStr) -> Result<CString, Failure> {
    CString::new(name.as_bytes()).map_err(|_| Failure::Unsafe)
}
fn open_at(parent: &File, name: &std::ffi::OsStr, flags: i32) -> Result<File, Failure> {
    let name = cstring(name)?;
    // No creation flags; every returned descriptor is close-on-exec.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        Err(failure(io::Error::last_os_error()))
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}
fn directory_flags() -> i32 {
    libc::O_RDONLY | libc::O_DIRECTORY
}
struct Root {
    path: PathBuf,
    file: File,
    ancestors: Vec<(u64, u64)>,
    ancestor_handles: Vec<File>,
}
impl Root {
    fn open(path: &Path) -> Result<Self, Failure> {
        if !path.is_absolute() || path.as_os_str().len() > 4096 || path.to_str().is_none() {
            return Err(Failure::Unsafe);
        }
        let mut file = File::open("/").map_err(failure)?;
        let mut ancestors = vec![Stamp::of(&file.metadata().map_err(failure)?).identity()];
        let mut ancestor_handles = vec![file.try_clone().map_err(failure)?];
        for component in path.components() {
            match component {
                Component::RootDir => (),
                Component::Normal(name) => {
                    file = open_at(&file, name, directory_flags())?;
                    ancestors.push(Stamp::of(&file.metadata().map_err(failure)?).identity());
                    ancestor_handles.push(file.try_clone().map_err(failure)?);
                }
                _ => return Err(Failure::Unsafe),
            }
        }
        Ok(Self {
            path: path.to_owned(),
            file,
            ancestors,
            ancestor_handles,
        })
    }
    fn verify(&self) -> Result<(), Failure> {
        for (file, identity) in self.ancestor_handles.iter().zip(&self.ancestors) {
            if Stamp::of(&file.metadata().map_err(failure)?).identity() != *identity {
                return Err(Failure::Changed);
            }
        }
        let named = Self::open(&self.path)?;
        if named.ancestors != self.ancestors {
            return Err(Failure::Changed);
        }
        Ok(())
    }
}
struct Directory(*mut libc::DIR);
impl Drop for Directory {
    fn drop(&mut self) {
        unsafe { libc::closedir(self.0) };
    }
}

pub(super) struct Access {
    data: Root,
    cache: Option<Root>,
    witnesses: BTreeMap<PathBuf, Stamp>,
    held: BTreeMap<PathBuf, File>,
    absent: BTreeSet<PathBuf>,
    path_bytes: usize,
}
impl Access {
    pub(super) fn new(paths: &StorePaths) -> Result<Self, Failure> {
        let roots = [&paths.data, &paths.cache, &paths.credentials];
        for (index, root) in roots.iter().enumerate() {
            if !root.is_absolute()
                || root.to_str().is_none()
                || root.as_os_str().len() > 4096
                || root
                    .components()
                    .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
            {
                return Err(Failure::Unsafe);
            }
            for other in roots.iter().skip(index + 1) {
                if root.starts_with(other) || other.starts_with(root) {
                    return Err(Failure::Unsafe);
                }
            }
        }
        let data = Root::open(&paths.data)?;
        let cache = match Root::open(&paths.cache) {
            Ok(root) => Some(root),
            Err(Failure::Missing) => None,
            Err(error) => return Err(error),
        };
        if let Some(cache) = &cache {
            if Stamp::of(&data.file.metadata().map_err(failure)?).identity()
                == Stamp::of(&cache.file.metadata().map_err(failure)?).identity()
            {
                return Err(Failure::Unsafe);
            }
        }
        let mut result = Self {
            data,
            cache,
            witnesses: BTreeMap::new(),
            held: BTreeMap::new(),
            absent: BTreeSet::new(),
            path_bytes: 0,
        };
        let data_path = result.data.path.clone();
        result.metadata(&data_path)?;
        if let Some(path) = result.cache.as_ref().map(|root| root.path.clone()) {
            result.metadata(&path)?;
        }
        Ok(result)
    }
    fn remember(&mut self, path: &Path, stamp: Stamp, file: &File) -> Result<(), Failure> {
        if self.absent.contains(path) {
            return Err(Failure::Changed);
        }
        if let Some(previous) = self.witnesses.get(path) {
            if *previous != stamp {
                return Err(Failure::Changed);
            }
        } else {
            if self.witnesses.len() + self.absent.len() >= MAX_ITEMS
                || self.path_bytes.saturating_add(path.as_os_str().len()) > MAX_METADATA
            {
                return Err(Failure::WorkLimit);
            }
            self.path_bytes += path.as_os_str().len();
            self.held
                .insert(path.to_owned(), file.try_clone().map_err(failure)?);
            self.witnesses.insert(path.to_owned(), stamp);
        }
        Ok(())
    }
    fn root_and_relative(&self, path: &Path) -> Result<(&Root, PathBuf), Failure> {
        for root in std::iter::once(&self.data).chain(self.cache.iter()) {
            if let Ok(relative) = path.strip_prefix(&root.path) {
                if path.as_os_str().len() > 4096
                    || relative
                        .components()
                        .any(|part| !matches!(part, Component::Normal(_)))
                {
                    return Err(Failure::Unsafe);
                }
                return Ok((root, relative.to_owned()));
            }
        }
        Err(Failure::Unsafe)
    }
    fn open(&mut self, path: &Path, flags: i32) -> Result<File, Failure> {
        let (root, relative) = self.root_and_relative(path)?;
        // A fresh open-file description avoids sharing a directory iteration
        // offset with the retained root or a previous readdir call.
        let mut file = open_at(
            &root.file,
            std::ffi::OsStr::new("."),
            if relative.as_os_str().is_empty() {
                flags
            } else {
                directory_flags()
            },
        )?;
        let mut current = root.path.clone();
        let components: Vec<_> = relative.components().collect();
        for (index, part) in components.iter().enumerate() {
            let Component::Normal(name) = part else {
                return Err(Failure::Unsafe);
            };
            file = open_at(
                &file,
                name,
                if index + 1 == components.len() {
                    flags
                } else {
                    directory_flags()
                },
            )?;
            current.push(name);
            let meta = file.metadata().map_err(failure)?;
            if meta.file_type().is_symlink() {
                return Err(Failure::Unsafe);
            }
            self.remember(&current, Stamp::of(&meta), &file)?;
        }
        let meta = file.metadata().map_err(failure)?;
        self.remember(path, Stamp::of(&meta), &file)?;
        Ok(file)
    }
    pub(super) fn metadata(&mut self, path: &Path) -> Result<Metadata, Failure> {
        let file = self.open(path, libc::O_PATH)?;
        file.metadata().map_err(failure)
    }
    pub(super) fn exists(&mut self, path: &Path) -> Result<bool, Failure> {
        match self.metadata(path) {
            Ok(_) => Ok(true),
            Err(Failure::Missing) => {
                if self.witnesses.contains_key(path) {
                    return Err(Failure::Changed);
                }
                if !self.absent.contains(path)
                    && (self.absent.len() + self.witnesses.len() >= MAX_ITEMS
                        || self.path_bytes.saturating_add(path.as_os_str().len()) > MAX_METADATA)
                {
                    return Err(Failure::WorkLimit);
                }
                if self.absent.insert(path.to_owned()) {
                    self.path_bytes += path.as_os_str().len();
                }
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }
    /// The charged length bounds actual reads even if an I/O/identity failure
    /// occurs. Contents are read once through the same admitted file descriptor.
    pub(super) fn read(&mut self, path: &Path, limit: usize) -> (Result<Vec<u8>, Failure>, usize) {
        let mut charged = 0;
        let result = (|| {
            let mut file = self.open(path, libc::O_RDONLY)?;
            let before = file.metadata().map_err(failure)?;
            if !before.is_file() {
                return Err(Failure::Unsafe);
            }
            let size = usize::try_from(before.len()).map_err(|_| Failure::TooLarge)?;
            if size > limit {
                return Err(Failure::TooLarge);
            }
            charged = size;
            let mut bytes = Vec::with_capacity(size);
            file.by_ref()
                .take(before.len())
                .read_to_end(&mut bytes)
                .map_err(failure)?;
            let after = file.metadata().map_err(failure)?;
            if bytes.len() != size || Stamp::of(&before) != Stamp::of(&after) {
                return Err(Failure::Changed);
            }
            self.remember(path, Stamp::of(&after), &file)?;
            Ok(bytes)
        })();
        (result, charged)
    }
    pub(super) fn entries(&mut self, path: &Path, limit: usize) -> Result<Vec<PathBuf>, Failure> {
        let file = self.open(path, directory_flags())?;
        let fd = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if fd < 0 {
            return Err(failure(io::Error::last_os_error()));
        }
        let directory = unsafe { libc::fdopendir(fd) };
        if directory.is_null() {
            unsafe { libc::close(fd) };
            return Err(failure(io::Error::last_os_error()));
        }
        let directory = Directory(directory);
        let mut result = Vec::new();
        loop {
            unsafe {
                *libc::__errno_location() = 0;
            }
            let entry = unsafe { libc::readdir(directory.0) };
            if entry.is_null() {
                if unsafe { *libc::__errno_location() } != 0 {
                    return Err(failure(io::Error::last_os_error()));
                }
                break;
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            if result.len() >= limit {
                return Err(Failure::WorkLimit);
            }
            result.push(path.join(OsString::from_vec(name.to_vec())));
        }
        self.remember(path, Stamp::of(&file.metadata().map_err(failure)?), &file)?;
        result.sort();
        Ok(result)
    }
    pub(super) fn lease(&mut self, path: &Path) -> Result<File, Failure> {
        let file = self.open(path, libc::O_RDONLY)?;
        if !file.metadata().map_err(failure)?.is_file() {
            return Err(Failure::Unsafe);
        }
        Ok(file)
    }
    pub(super) fn verify(&mut self) -> Result<(), Failure> {
        self.data.verify()?;
        if let Some(root) = &self.cache {
            root.verify()?;
        }
        let witnesses: Vec<_> = self
            .witnesses
            .iter()
            .map(|(path, stamp)| (path.clone(), *stamp))
            .collect();
        for (path, stamp) in witnesses {
            if !self
                .held
                .get(&path)
                .is_some_and(|file| file.metadata().is_ok_and(|meta| Stamp::of(&meta) == stamp))
            {
                return Err(Failure::Changed);
            }
            if Stamp::of(&self.metadata(&path)?) != stamp {
                return Err(Failure::Changed);
            }
        }
        let absent: Vec<_> = self.absent.iter().cloned().collect();
        for path in absent {
            match self.metadata(&path) {
                Err(Failure::Missing) => (),
                Ok(_) => return Err(Failure::Changed),
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

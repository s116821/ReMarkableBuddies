//! Bounded external input, separate from owned Store-object and credential I/O.
use super::*;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputRefusal {
    InvalidInput,
    Unsupported,
}
impl std::fmt::Display for InputRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "notebook input refused",
            Self::Unsupported => "notebook safe-open unsupported",
        })
    }
}
impl std::error::Error for InputRefusal {}

/// Freeze the exact observed buffer after safe-open, binding and expected-hash
/// checks. No Store/model/destination effects occur here. Windows remains
/// unsupported until its handle identity/sharing contract is implemented.
pub fn read_document_file(
    path: &Path,
    expected: &str,
) -> std::result::Result<ValidatedNotebook, InputRefusal> {
    if !crate::storage::valid_digest(expected) {
        return Err(InputRefusal::InvalidInput);
    }
    #[cfg(target_os = "linux")]
    {
        linux::read(path, expected, |_| {})
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        Err(InputRefusal::Unsupported)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use nix::libc;
    use std::{
        ffi::{CString, OsString},
        fs::{File, Metadata},
        io::Read,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::MetadataExt},
        },
        path::Component,
    };
    fn open(parent: &File, name: &std::ffi::OsStr, flags: i32) -> std::io::Result<File> {
        let name = CString::new(name.as_bytes()).map_err(|_| std::io::ErrorKind::InvalidInput)?;
        // SAFETY: name is a live NUL-terminated C string; parent is a retained
        // directory descriptor. A successful new fd is adopted exactly once.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    fn same(a: &Metadata, b: &Metadata) -> bool {
        a.dev() == b.dev()
            && a.ino() == b.ino()
            && a.len() == b.len()
            && a.mtime() == b.mtime()
            && a.mtime_nsec() == b.mtime_nsec()
            && a.ctime() == b.ctime()
            && a.ctime_nsec() == b.ctime_nsec()
            && a.file_type() == b.file_type()
    }
    fn same_binding(a: &Metadata, b: &Metadata) -> bool {
        a.dev() == b.dev() && a.ino() == b.ino() && a.file_type() == b.file_type()
    }
    fn local(file: &File) -> std::result::Result<(), InputRefusal> {
        let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: stat points to writable storage of the platform's statfs type.
        if unsafe { libc::fstatfs(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
            return Err(InputRefusal::Unsupported);
        }
        let kind = unsafe { stat.assume_init() }.f_type as u64 & 0xffff_ffff;
        // Explicitly supported local disk filesystems, including the RM2 ext4.
        if ![0xef53, 0x58465342, 0x9123683e].contains(&kind) {
            return Err(InputRefusal::Unsupported);
        }
        Ok(())
    }
    pub(super) fn read(
        path: &Path,
        expected: &str,
        mut hook: impl FnMut(&'static str),
    ) -> std::result::Result<ValidatedNotebook, InputRefusal> {
        let invalid = |_| InputRefusal::InvalidInput;
        let absolute = if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir().map_err(invalid)?.join(path)
        };
        let mut names = Vec::<OsString>::new();
        for component in absolute.components() {
            match component {
                Component::RootDir => (),
                Component::Normal(name) => names.push(name.to_owned()),
                _ => return Err(InputRefusal::InvalidInput),
            }
        }
        let leaf = names.pop().ok_or(InputRefusal::InvalidInput)?;
        let mut dirs = vec![File::open("/").map_err(invalid)?];
        let mut bindings = Vec::new();
        for name in &names {
            let child = open(
                dirs.last().unwrap(),
                name,
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NONBLOCK,
            )
            .map_err(invalid)?;
            let metadata = child.metadata().map_err(invalid)?;
            if !metadata.is_dir() {
                return Err(InputRefusal::InvalidInput);
            }
            bindings.push(metadata);
            dirs.push(child);
        }
        let parent = dirs.last().unwrap();
        let named = open(parent, &leaf, libc::O_PATH | libc::O_NONBLOCK)
            .map_err(invalid)?
            .metadata()
            .map_err(invalid)?;
        if !named.is_file() || named.len() > MAX_CANONICAL_BYTES as u64 {
            return Err(InputRefusal::InvalidInput);
        }
        hook("before-open");
        // NONBLOCK is set on the open itself, before fstat: a FIFO swap cannot
        // block while awaiting a writer, and is rejected without a probe read.
        let mut file = open(parent, &leaf, libc::O_RDONLY | libc::O_NONBLOCK).map_err(invalid)?;
        let before = file.metadata().map_err(invalid)?;
        if !before.is_file() || !same(&named, &before) {
            return Err(InputRefusal::InvalidInput);
        }
        local(&file)?;
        hook("after-open");
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_CANONICAL_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(invalid)?;
        hook("after-read");
        let after = file.metadata().map_err(invalid)?;
        let named_after = open(parent, &leaf, libc::O_PATH | libc::O_NONBLOCK)
            .map_err(invalid)?
            .metadata()
            .map_err(invalid)?;
        if bytes.len() as u64 != before.len()
            || !same(&before, &after)
            || !same(&after, &named_after)
        {
            return Err(InputRefusal::InvalidInput);
        }
        for (index, name) in names.iter().enumerate() {
            let named = open(&dirs[index], name, libc::O_PATH | libc::O_NONBLOCK)
                .map_err(invalid)?
                .metadata()
                .map_err(invalid)?;
            if !named.is_dir()
                || !same_binding(&bindings[index], &named)
                || !same_binding(
                    &bindings[index],
                    &dirs[index + 1].metadata().map_err(invalid)?,
                )
            {
                return Err(InputRefusal::InvalidInput);
            }
        }
        ValidatedNotebook::from_bytes(&bytes, expected).map_err(|_| InputRefusal::InvalidInput)
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        use std::{fs, os::unix::fs::symlink};
        struct Fixture(std::path::PathBuf);
        impl Fixture {
            fn new() -> Self {
                let root = std::env::temp_dir().join(format!("notebook-input-{}", Uuid::new_v4()));
                fs::create_dir(&root).unwrap();
                Self(root)
            }
            fn file(&self) -> std::path::PathBuf {
                self.0.join("input.json")
            }
            fn write(&self) -> Vec<u8> {
                let bytes =
                    include_bytes!("../../../tests/fixtures/writer_notebook_identity_vectors.json");
                let vector: serde_json::Value = serde_json::from_slice(bytes).unwrap();
                let raw = vector["raw_document_utf8"]
                    .as_str()
                    .unwrap()
                    .as_bytes()
                    .to_vec();
                fs::write(self.file(), &raw).unwrap();
                raw
            }
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        #[test]
        fn regular_hash_json_symlink_and_growth_are_checked_without_path_disclosure() {
            let f = Fixture::new();
            let raw = f.write();
            let expected = digest(&raw);
            let result = read(&f.file(), &expected, |_| {});
            if matches!(result, Err(InputRefusal::Unsupported)) {
                return;
            } // Filesystem support is explicit.
            assert_eq!(result.unwrap().raw_sha256(), expected);
            let sibling = read(&f.file(), &expected, |stage| {
                if stage == "after-open" {
                    fs::write(f.0.join("unrelated-sibling"), b"unrelated").unwrap();
                }
            });
            assert_eq!(sibling.unwrap().raw_sha256(), expected);
            assert!(read(&f.file(), &"a".repeat(64), |_| {}).is_err());
            let link = f.0.join("link");
            symlink(f.file(), &link).unwrap();
            assert!(read(&link, &expected, |_| {}).is_err());
            assert!(read(&f.0, &expected, |_| {}).is_err());
            let changed = read(&f.file(), &expected, |stage| {
                if stage == "after-open" {
                    fs::write(f.file(), b"changed").unwrap();
                }
            });
            assert_eq!(changed.err(), Some(InputRefusal::InvalidInput));
            assert!(!InputRefusal::InvalidInput
                .to_string()
                .contains(f.0.to_str().unwrap()));
        }
        #[test]
        fn fifo_swap_and_parent_replacement_are_refused_before_any_probe_read() {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "writer::notebook::input::linux::tests::fifo_swap_child",
                    "--nocapture",
                ])
                .env("RMB_NOTEBOOK_FIFO_CHILD", "1")
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("FIFO admission child exceeded deadline");
                }
                // Bounded process-status polling; this wait never implies success.
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let f = Fixture::new();
            let raw = f.write();
            let expected = digest(&raw);
            let child = f.0.join("parent");
            fs::create_dir(&child).unwrap();
            fs::write(child.join("source"), &raw).unwrap();
            let result = read(&child.join("source"), &expected, |stage| {
                if stage == "after-open" {
                    fs::rename(&child, f.0.join("renamed")).unwrap();
                    fs::create_dir(&child).unwrap();
                    fs::write(child.join("source"), &raw).unwrap();
                }
            });
            if !matches!(result, Err(InputRefusal::Unsupported)) {
                assert_eq!(result.err(), Some(InputRefusal::InvalidInput));
            }
        }
        #[test]
        fn fifo_swap_child() {
            if std::env::var_os("RMB_NOTEBOOK_FIFO_CHILD").is_none() {
                return;
            }
            let f = Fixture::new();
            let raw = f.write();
            let expected = digest(&raw);
            let result = read(&f.file(), &expected, |stage| {
                if stage == "before-open" {
                    fs::remove_file(f.file()).unwrap();
                    let name = CString::new(f.file().as_os_str().as_bytes()).unwrap();
                    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
                }
            });
            assert_eq!(result.err(), Some(InputRefusal::InvalidInput));
        }
        #[test]
        fn aliases_replacement_invalid_encoding_and_one_over_bound_refuse() {
            let f = Fixture::new();
            let raw = f.write();
            let expected = digest(&raw);
            let socket = f.0.join("socket");
            let _socket = std::os::unix::net::UnixListener::bind(&socket).unwrap();
            assert_eq!(
                read(&socket, &expected, |_| {}).err(),
                Some(InputRefusal::InvalidInput)
            );
            assert_eq!(
                read(Path::new("/dev/null"), &expected, |_| {}).err(),
                Some(InputRefusal::InvalidInput)
            );
            assert_eq!(
                read(&f.0.join("../input.json"), &expected, |_| {}).err(),
                Some(InputRefusal::InvalidInput)
            );
            for bytes in [
                vec![0xff],
                b"{}".to_vec(),
                vec![b'x'; MAX_CANONICAL_BYTES + 1],
            ] {
                fs::write(f.file(), &bytes).unwrap();
                let result = read(&f.file(), &digest(&bytes), |_| {});
                assert!(matches!(
                    result,
                    Err(InputRefusal::InvalidInput | InputRefusal::Unsupported)
                ));
            }
            f.write();
            let result = read(&f.file(), &expected, |stage| {
                if stage == "after-read" {
                    fs::rename(f.file(), f.0.join("original")).unwrap();
                    fs::write(f.file(), &raw).unwrap();
                }
            });
            assert!(matches!(
                result,
                Err(InputRefusal::InvalidInput | InputRefusal::Unsupported)
            ));
        }
    }
}

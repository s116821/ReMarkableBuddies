//! Developer visual capture of the runtime's immutable private executable copy.
use anyhow::{ensure, Result};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Binding {
    path: PathBuf,
    hash: [u8; 32],
}

impl Binding {
    pub(super) fn new(nonce: &str, hash: &str) -> Result<Self> {
        let hex = |value: &str, size| {
            value.len() == size
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        };
        ensure!(
            hex(nonce, 32) && hex(hash, 64),
            "Invalid resident capture binding"
        );
        let mut digest = [0; 32];
        for (index, value) in digest.iter_mut().enumerate() {
            *value = u8::from_str_radix(&hash[index * 2..index * 2 + 2], 16)?;
        }
        Ok(Self {
            path: PathBuf::from(format!("/run/rsdk-bootstrap-{nonce}/session/reader")),
            hash: digest,
        })
    }

    fn check_link(&self, proc_exe: &Path) -> Result<()> {
        ensure!(
            std::fs::read_link(proc_exe)? == self.path
                && std::fs::canonicalize(&self.path)? == self.path,
            "Resident capture executable path changed"
        );
        Ok(())
    }

    pub(super) fn check_path(&self, pid: u32) -> Result<()> {
        self.check_link(Path::new(&format!("/proc/{pid}/exe")))
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{
        fs::{File, Metadata},
        os::unix::fs::{FileExt, MetadataExt},
    };

    const MAX_EXECUTABLE: u64 = 64 * 1024 * 1024;

    #[derive(Debug, PartialEq, Eq)]
    struct MetadataKey {
        identity: (u64, u64, u64),
        ownership: (u32, u32, u32),
        size: u64,
        times: (i64, i64, i64, i64),
    }
    impl MetadataKey {
        fn read(value: &Metadata) -> Result<Self> {
            ensure!(
                value.is_file()
                    && value.uid() == 0
                    && value.mode() & 0o7777 == 0o500
                    && value.nlink() == 1
                    && (1..=MAX_EXECUTABLE).contains(&value.len()),
                "Untrusted resident capture executable"
            );
            Ok(Self {
                identity: (value.dev(), value.ino(), value.nlink()),
                ownership: (value.uid(), value.gid(), value.mode()),
                size: value.len(),
                times: (
                    value.mtime(),
                    value.mtime_nsec(),
                    value.ctime(),
                    value.ctime_nsec(),
                ),
            })
        }
    }

    pub(in super::super) struct Pin {
        binding: Binding,
        proc_exe: PathBuf,
        file: File,
        metadata: MetadataKey,
    }
    impl Pin {
        pub(in super::super) fn verify_content(&self) -> Result<()> {
            self.current()?;
            let mut hash = Sha256::new();
            let mut total = 0u64;
            let mut buffer = [0; 32 * 1024];
            loop {
                let size = self.file.read_at(&mut buffer, total)?;
                if size == 0 {
                    break;
                }
                total += size as u64;
                ensure!(
                    total <= MAX_EXECUTABLE,
                    "Resident executable exceeds capture bound"
                );
                hash.update(&buffer[..size]);
            }
            ensure!(
                <[u8; 32]>::from(hash.finalize()) == self.binding.hash
                    && total == self.metadata.size,
                "Resident capture executable hash mismatch"
            );
            self.current()
        }

        pub(in super::super) fn current(&self) -> Result<()> {
            self.binding.check_link(&self.proc_exe)?;
            ensure!(
                MetadataKey::read(&self.file.metadata()?)? == self.metadata
                    && MetadataKey::read(&std::fs::metadata(&self.proc_exe)?)? == self.metadata
                    && MetadataKey::read(&std::fs::metadata(&self.binding.path)?)? == self.metadata,
                "Resident capture executable changed"
            );
            Ok(())
        }
    }
    impl Binding {
        pub(in super::super) fn pin(&self, pid: u32) -> Result<Pin> {
            self.pin_at(PathBuf::from(format!("/proc/{pid}/exe")))
        }
        fn pin_at(&self, proc_exe: PathBuf) -> Result<Pin> {
            self.check_link(&proc_exe)?;
            let file = File::open(&proc_exe)?;
            let metadata = MetadataKey::read(&file.metadata()?)?;
            let pin = Pin {
                binding: self.clone(),
                proc_exe,
                file,
                metadata,
            };
            pin.verify_content()?;
            Ok(pin)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::{symlink, PermissionsExt};

        #[test]
        #[ignore = "requires root-owned mode0500 files in the owned Linux fixture"]
        fn executable_pin_refuses_wrong_hash_permissions_replacement_and_mutation() {
            let root = std::env::temp_dir().join(format!("capture-exe-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            let path = root.join("reader");
            let proc_exe = root.join("exe");
            std::fs::write(&path, b"immutable fixture").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o500)).unwrap();
            symlink(&path, &proc_exe).unwrap();
            let binding = Binding {
                path: path.clone(),
                hash: Sha256::digest(b"immutable fixture").into(),
            };
            let pin = binding.pin_at(proc_exe.clone()).unwrap();
            pin.current().unwrap();
            let mut wrong = binding.clone();
            wrong.hash[0] ^= 1;
            assert!(wrong.pin_at(proc_exe.clone()).is_err());
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(pin.current().is_err());
            assert!(binding.pin_at(proc_exe.clone()).is_err());
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o500)).unwrap();
            let pin = binding.pin_at(proc_exe.clone()).unwrap();
            std::fs::rename(&path, root.join("old")).unwrap();
            std::fs::write(&path, b"immutable fixture").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o500)).unwrap();
            assert!(pin.current().is_err());
            let pin = binding.pin_at(proc_exe.clone()).unwrap();
            std::fs::write(&path, b"different fixture").unwrap();
            // Same-size writes can share coarse timestamps. Closing digest
            // verification must refuse even when metadata alone is unchanged.
            assert!(pin.verify_content().is_err());
            assert!(binding.pin_at(proc_exe).is_err());
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}

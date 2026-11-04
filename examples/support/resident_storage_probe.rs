//! Sampled native-file evidence only. Closed view and quiescence are not a save fence.
use anyhow::{Result, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{fs::{self, File, Metadata, OpenOptions}, io::Read, os::{fd::AsRawFd, unix::fs::{MetadataExt, OpenOptionsExt}}, path::PathBuf, time::{Duration, Instant}};

type Identity = (u64, u64, u64, u32, u32, u32, i64, i64, i64, i64);
fn identity(value: &Metadata) -> Identity {
    (value.dev(), value.ino(), value.len(), value.uid(), value.gid(), value.mode(), value.mtime(), value.mtime_nsec(), value.ctime(), value.ctime_nsec())
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileSample {
    pub sha256: String,
    pub bytes: u64,
    #[serde(skip_serializing)]
    identity: Identity,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Samples { pub rm: FileSample, pub thumbnail: FileSample, pub content: FileSample, pub metadata: FileSample }
struct Watched {
    parent: PathBuf,
    directory: File,
    directory_identity: (u64, u64, u32, u32, u32),
    name: String,
}
impl Watched {
    fn open(parent: PathBuf, name: String) -> Result<Self> {
        let named = fs::symlink_metadata(&parent)?;
        ensure!(named.is_dir() && named.uid() == 0 && named.mode() & 0o22 == 0 && fs::canonicalize(&parent)? == parent, "unsafe target storage parent");
        let directory = OpenOptions::new().read(true).custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC).open(&parent)?;
        let actual = directory.metadata()?;
        ensure!((actual.dev(), actual.ino()) == (named.dev(), named.ino()), "target storage parent changed");
        Ok(Self { parent, directory, directory_identity: (actual.dev(), actual.ino(), actual.uid(), actual.gid(), actual.mode()), name })
    }
    fn sample(&self, deadline: Instant) -> Result<Option<FileSample>> {
        ensure!(Instant::now() < deadline, "storage observation original deadline expired");
        for current in [self.directory.metadata()?, fs::symlink_metadata(&self.parent)?] {
            ensure!(current.is_dir() && (current.dev(), current.ino(), current.uid(), current.gid(), current.mode()) == self.directory_identity, "target storage parent identity changed");
        }
        let path = format!("/proc/self/fd/{}/{}", self.directory.as_raw_fd(), self.name);
        let mut file = match OpenOptions::new().read(true).custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let before = file.metadata()?;
        ensure!(before.is_file() && before.uid() == 0 && before.mode() & 0o22 == 0 && before.nlink() == 1 && before.len() <= 16 * 1024 * 1024, "unsafe target storage file");
        let mut bytes = Vec::new();
        (&mut file).take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        let after = file.metadata()?;
        let named = match fs::symlink_metadata(self.parent.join(&self.name)) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        ensure!(Instant::now() < deadline, "storage observation returned after original deadline");
        if identity(&before) != identity(&after) || identity(&after) != identity(&named) || bytes.len() as u64 != after.len() {
            return Ok(None); // A concurrent native replacement/write is not a stable sample.
        }
        Ok(Some(FileSample { sha256: Sha256::digest(&bytes).iter().map(|value| format!("{value:02x}")).collect(), bytes: bytes.len() as u64, identity: identity(&after) }))
    }
}
pub struct StorageWatch { rm: Watched, thumbnail: Watched, content: Watched, metadata: Watched }
impl StorageWatch {
    pub fn open(document: &str, target: &str) -> Result<Self> {
        for value in [document, target] {
            ensure!(uuid::Uuid::parse_str(value)?.to_string() == value, "invalid target storage UUID");
        }
        let root = PathBuf::from("/home/root/.local/share/remarkable/xochitl");
        Ok(Self { rm: Watched::open(root.join(document), format!("{target}.rm"))?, thumbnail: Watched::open(root.join(format!("{document}.thumbnails")), format!("{target}.png"))?, content: Watched::open(root.clone(), format!("{document}.content"))?, metadata: Watched::open(root, format!("{document}.metadata"))? })
    }
    pub fn sample(&self, deadline: Instant) -> Result<Option<Samples>> {
        Ok(match (self.rm.sample(deadline)?, self.thumbnail.sample(deadline)?, self.content.sample(deadline)?, self.metadata.sample(deadline)?) {
            (Some(rm), Some(thumbnail), Some(content), Some(metadata)) => Some(Samples { rm, thumbnail, content, metadata }),
            _ => None,
        })
    }
    /// Wait for a changed .rm and unchanged target/document file samples for250ms.
    /// This is bounded storage evidence after ordinary close, not native save ack.
    pub fn changed_quiescent(&self, before: &Samples, deadline: Instant, pulse: &mut dyn FnMut() -> Result<()>) -> Result<Samples> {
        let deadline = deadline.min(Instant::now() + Duration::from_secs(5));
        let mut stable: Option<(Samples, Instant)> = None;
        loop {
            ensure!(Instant::now() < deadline, "native target did not change/quiesce before original bound");
            pulse()?;
            if let Some(sample) = self.sample(deadline)? {
                if sample.rm.sha256 != before.rm.sha256 && sample.rm.bytes > 0 {
                    if let Some((previous, since)) = &stable {
                        if *previous == sample && since.elapsed() >= Duration::from_millis(250) { return Ok(sample); }
                        if *previous != sample { stable = Some((sample, Instant::now())); }
                    } else { stable = Some((sample, Instant::now())); }
                } else { stable = None; }
            } else { stable = None; }
            std::thread::sleep(Duration::from_millis(25).min(deadline.saturating_duration_since(Instant::now())));
        }
    }
}

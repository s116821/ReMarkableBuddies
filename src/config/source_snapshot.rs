//! In-process provenance of a loaded local file. Never serialized configuration.
use super::*;
use crate::storage::{digest, selection::SelectedReadBudget};
#[derive(Clone, Debug)]
pub struct SourceSnapshot {
    path: PathBuf,
    sha256: Option<String>,
}
impl SourceSnapshot {
    pub(super) fn observed(path: PathBuf, bytes: Option<&[u8]>) -> Self {
        Self {
            path,
            sha256: bytes.map(digest),
        }
    }
    pub(crate) fn fingerprint(&self) -> (&Path, Option<&str>) {
        (&self.path, self.sha256.as_deref())
    }
    pub(crate) fn check(&self, reads: &mut SelectedReadBudget) -> Result<()> {
        reads.charge(0, 1, 0, 0)?;
        files::safe_path(&self.path)?;
        if let Some(expected) = &self.sha256 {
            let bytes = files::read_with_budget(&self.path, MAX_RECORD as u64, Some(reads))?;
            ensure!(
                digest(&bytes) == *expected,
                "original local configuration changed"
            );
        } else {
            ensure!(
                matches!(std::fs::symlink_metadata(&self.path),Err(error) if error.kind()==std::io::ErrorKind::NotFound),
                "previously absent local configuration changed"
            );
        }
        Ok(())
    }
}

//! Sampled inheritance checks, not group drainage authority. The caller retains
//! actual cgroup directory ownership and the trusted no-migration premise.
use super::*;

pub(super) struct Binding {
    parent: u32,
    membership: Vec<u8>,
    // Retain namespace descriptors so their inode identities cannot be reused.
    namespaces: [File; 2],
}

impl Binding {
    pub(super) fn capture() -> Result<Self> {
        let parent = std::process::id();
        let result = Self {
            parent,
            membership: membership(parent)?,
            namespaces: [
                File::open(format!("/proc/{parent}/ns/mnt"))?,
                File::open(format!("/proc/{parent}/ns/cgroup"))?,
            ],
        };
        result.parent_current()?;
        Ok(result)
    }

    pub(super) fn parent_current(&self) -> Result<()> {
        ensure!(
            self.parent == std::process::id(),
            "companion group owner changed"
        );
        self.process_current(self.parent)
    }

    pub(super) fn child_current(&self, pid: u32) -> Result<()> {
        self.parent_current()?;
        self.process_current(pid)?;
        self.parent_current()
    }

    fn process_current(&self, pid: u32) -> Result<()> {
        ensure!(
            membership(pid)? == self.membership,
            "companion group membership changed"
        );
        for (name, original) in ["mnt", "cgroup"].iter().zip(&self.namespaces) {
            let observed = fs::metadata(format!("/proc/{pid}/ns/{name}"))?;
            let original = original.metadata()?;
            ensure!(
                observed.dev() == original.dev() && observed.ino() == original.ino(),
                "companion group namespace changed"
            );
        }
        ensure!(
            membership(pid)? == self.membership,
            "companion group membership changed during sample"
        );
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn inject_mismatch(&mut self, namespace: bool) {
        if namespace {
            self.namespaces[0] = File::open("/proc/self/ns/pid").unwrap();
        } else {
            self.membership.push(b'x');
        }
    }
}

fn membership(pid: u32) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(format!("/proc/{pid}/cgroup"))?
        .take(16 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= 16 * 1024,
        "companion membership exceeds bound or is empty"
    );
    Ok(bytes)
}

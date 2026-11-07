use anyhow::{ensure, Result};
use remarkable_reader_buddy::storage::{
    drive::qualification::{scope_digest, Resource, MULTIPART_MAX},
    files,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

pub const ROLES: [&str; 6] = [
    "browser",
    "electron",
    "tablet",
    "contender",
    "wrong-account",
    "wrong-client",
];
pub const IDS: usize = 27;
const META_MAX: usize = 8 * 1024;
const MAX_BODY: usize = 344 * 1024;
const MAX_JSON: u64 = 128 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub version: u32,
    pub run: Uuid,
    pub group: Uuid,
    pub buddy_revision: String,
    pub docs_revision: String,
    pub expires_unix: u64,
    pub request_seconds: u64,
    pub rendezvous_millis: u64,
    pub grants: [Grant; 6],
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub binding_sha256: String,
    pub stimulate_refresh: bool,
}

impl Plan {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1
                && !self.run.is_nil()
                && !self.group.is_nil()
                && self.run != self.group
                && hex(&self.buddy_revision, 40)
                && hex(&self.docs_revision, 40)
                && self.request_seconds == 10
                && (1..=10_000).contains(&self.rendezvous_millis)
                && self.grants.iter().all(|g| hex(&g.binding_sha256, 64))
                && self.grants[3].binding_sha256 == self.grants[0].binding_sha256
                && self.grants[4..].iter().all(|g| !g.stimulate_refresh),
            "invalid qualification plan"
        );
        Ok(())
    }

    pub fn verify_build(&self) -> Result<()> {
        ensure!(
            self.buddy_revision == env!("VERGEN_GIT_SHA") && env!("VERGEN_GIT_DIRTY") == "false",
            "qualification requires the pinned clean source build"
        );
        Ok(())
    }
}

pub fn deadline(unix: u64) -> Result<Instant> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
    let remaining = Duration::from_secs(unix)
        .checked_sub(now)
        .filter(|time| !time.is_zero() && *time <= Duration::from_secs(3600))
        .ok_or_else(|| anyhow::anyhow!("qualification deadline unavailable"))?;
    Ok(Instant::now() + remaining)
}

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frozen {
    pub plan: Plan,
    pub ids: Vec<String>,
    pub allocator_refreshed: bool,
}

impl Frozen {
    pub fn validate(&self) -> Result<()> {
        self.plan.validate()?;
        let unique: BTreeSet<_> = self.ids.iter().collect();
        ensure!(
            unique.len() == IDS
                && self.ids.len() == IDS
                && self.ids.iter().all(|id| !id.is_empty()
                    && id.len() <= 111
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')),
            "invalid qualification IDs"
        );
        Ok(())
    }

    pub fn scope(&self, label: &str) -> String {
        scope_digest(self.plan.run, self.plan.group, label)
    }

    pub fn resource(&self, index: usize, contender: bool) -> Result<Resource> {
        Resource::frozen(self.plan.run, self.plan.group, &self.ids, index, contender)
    }

    pub fn descriptor(&self, id: &str) -> Result<Resource> {
        let index = self
            .ids
            .iter()
            .position(|candidate| candidate == id)
            .filter(|index| (18..25).contains(index))
            .ok_or_else(|| anyhow::anyhow!("descriptor not frozen"))?;
        self.resource(index, false)
    }

    pub fn owner(index: usize, contender: bool) -> usize {
        match index {
            11..=13 => index - 11,
            20..=22 => index - 20,
            19 => 3,
            0..=9 if contender => 3,
            _ => 0,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Original,
    Contender,
    WinnerReplay,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Contender => "contender",
            Self::WinnerReplay => "winner-replay",
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Evidence {
    Registration {
        left: usize,
        right: usize,
        shared: bool,
    },
    Refresh {
        role: usize,
        stimulated: bool,
        observed: bool,
    },
    Race {
        sample: usize,
        winner: usize,
    },
    Replay {},
    ChangedContext {},
    LostAcknowledgement {},
    Matrix {
        origin: usize,
        reader: usize,
        exact: bool,
    },
    Negative {
        index: usize,
    },
    UnrelatedScope {},
}

impl Evidence {
    pub fn label(&self) -> String {
        match *self {
            Self::Registration {
                left,
                right,
                shared,
            } => format!(
                "registration {} {} shared={shared}",
                ROLES[left], ROLES[right]
            ),
            Self::Refresh {
                role,
                stimulated,
                observed,
            } => format!(
                "refresh {} stimulated={stimulated} observed={observed}",
                ROLES[role]
            ),
            Self::Race { sample, winner } => format!("race {sample} winner {}", ROLES[winner]),
            Self::Replay {} => "winner replay conflict".into(),
            Self::ChangedContext {} => "changed account and client refused".into(),
            Self::LostAcknowledgement {} => {
                "original slot observed after acknowledgement interruption".into()
            }
            Self::Matrix {
                origin,
                reader,
                exact,
            } => {
                format!(
                    "matrix {} -> {} exact={exact}",
                    ROLES[origin], ROLES[reader]
                )
            }
            Self::Negative { index } => format!("negative resource {index} refused"),
            Self::UnrelatedScope {} => "absent and unrelated scope controls passed".into(),
        }
    }

    fn valid(&self) -> bool {
        match *self {
            Self::Registration { left, right, .. } => left < right && right < 4,
            Self::Refresh { role, .. } => role < 4,
            Self::Race { sample, winner } => (1..=10).contains(&sample) && matches!(winner, 0 | 3),
            Self::Matrix { origin, reader, .. } => origin < 3 && reader < 3,
            Self::Negative { index } => (14..=16).contains(&index),
            _ => true,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Send {
    pub index: usize,
    pub action: Action,
    pub bytes: usize,
    pub response: Option<u16>,
    pub returned: bool,
    pub canceled_before_send: bool,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    pub started: bool,
    pub passed: bool,
    pub sends: Vec<Send>,
    pub evidence: Vec<Evidence>,
    pub observed: BTreeMap<usize, Action>,
    pub cleanup_started: bool,
    pub removed: BTreeSet<usize>,
}

impl Journal {
    pub fn admit_run(&self) -> Result<()> {
        ensure!(
            !self.started && !self.cleanup_started,
            "frozen run already consumed"
        );
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        let mut rebuilt = Self::default();
        for send in &self.sends {
            rebuilt.add(send.index, send.action, send.bytes)?;
            ensure!(
                send.response
                    .is_none_or(|code| send.returned && (100..=599).contains(&code)),
                "invalid response status"
            );
            ensure!(
                !send.canceled_before_send || (!send.returned && send.response.is_none()),
                "invalid canceled send"
            );
        }
        ensure!(
            (self.started || (self.sends.is_empty() && !self.passed))
                && self.evidence.len() <= 160
                && self.evidence.iter().all(Evidence::valid)
                && self
                    .observed
                    .iter()
                    .all(|(index, action)| self.sends.iter().any(|s| {
                        s.index == *index && s.action == *action && !s.canceled_before_send
                    }))
                && self
                    .removed
                    .iter()
                    .all(|i| self.sends.iter().any(|s| s.index == *i))
                && (self.cleanup_started || self.removed.is_empty()),
            "invalid qualification journal"
        );
        Ok(())
    }

    pub fn add(&mut self, index: usize, action: Action, bytes: usize) -> Result<usize> {
        let media = (18..25).contains(&index);
        ensure!(
            index < 25
                && bytes > 0
                && bytes <= if media { MULTIPART_MAX } else { META_MAX }
                && match action {
                    Action::Original => true,
                    Action::Contender => index < 10,
                    Action::WinnerReplay => index == 0,
                }
                && !self
                    .sends
                    .iter()
                    .any(|send| send.index == index && send.action == action)
                && self.sends.iter().map(|send| send.bytes).sum::<usize>() + bytes <= MAX_BODY,
            "qualification send budget exceeded"
        );
        let count = self
            .sends
            .iter()
            .filter(|send| (18..25).contains(&send.index) == media)
            .count();
        ensure!(
            count < if media { 7 } else { 29 },
            "qualification send count exceeded"
        );
        self.sends.push(Send {
            index,
            action,
            bytes,
            response: None,
            returned: false,
            canceled_before_send: false,
        });
        Ok(self.sends.len() - 1)
    }
}

pub struct Directory {
    root: PathBuf,
    _lock: File,
}

impl Directory {
    pub fn open(root: &Path) -> Result<Self> {
        #[cfg(not(unix))]
        {
            let _ = root;
            anyhow::bail!("qualification directory protection requires Unix");
        }

        #[cfg(unix)]
        {
            use std::{
                fs::OpenOptions,
                os::unix::fs::{MetadataExt, OpenOptionsExt},
            };
            ensure!(
                root.is_absolute(),
                "qualification directory must be absolute"
            );
            files::safe_path(root)?;
            let metadata = std::fs::symlink_metadata(root)?;
            ensure!(
                metadata.is_dir() && metadata.mode() & 0o077 == 0,
                "qualification directory must be private"
            );
            let lock_path = root.join("qualification.lock");
            files::safe_path(&lock_path)?;
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(lock_path)?;
            ensure!(
                lock.metadata()?.is_file() && lock.metadata()?.nlink() == 1,
                "unsafe qualification lock"
            );
            lock.try_lock()?;
            Ok(Self {
                root: root.to_owned(),
                _lock: lock,
            })
        }
    }

    pub fn grant(&self, role: usize) -> Result<PathBuf> {
        let path = self.root.join(format!("{}.json", ROLES[role]));
        files::safe_path(&path)?;
        let metadata = std::fs::symlink_metadata(&path)?;
        ensure!(metadata.is_file(), "grant must be a dedicated regular file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            ensure!(
                metadata.mode() & 0o077 == 0 && metadata.nlink() == 1,
                "grant must be private and dedicated"
            );
        }
        Ok(path)
    }

    pub fn exists(&self, name: &str) -> bool {
        self.root.join(name).exists()
    }

    pub fn load<T: DeserializeOwned>(&self, name: &str) -> Result<(T, String)> {
        let path = self.root.join(name);
        let bytes = files::read(&path, MAX_JSON)?;
        Ok((serde_json::from_slice(&bytes)?, digest(&bytes)))
    }

    pub fn save(&self, name: &str, value: &impl Serialize) -> Result<()> {
        let bytes = serde_json::to_vec(value)?;
        ensure!(bytes.len() as u64 <= MAX_JSON, "qualification ledger bound");
        files::atomic(&self.root.join(name), &bytes)
    }

    pub fn frozen(&self) -> Result<(Frozen, String)> {
        let (value, sha): (Frozen, _) = self.load("frozen.json")?;
        value.validate()?;
        Ok((value, sha))
    }

    pub fn journal(&self) -> Result<(Journal, String)> {
        let (value, sha): (Journal, _) = self.load("journal.json")?;
        value.validate()?;
        Ok((value, sha))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserve_enforces_action_identity_and_encoded_limits() {
        let mut journal = Journal::default();
        for index in 0..25 {
            journal
                .add(
                    index,
                    Action::Original,
                    if index < 18 { META_MAX } else { MULTIPART_MAX },
                )
                .unwrap();
        }
        for index in 0..10 {
            journal.add(index, Action::Contender, META_MAX).unwrap();
        }
        journal.add(0, Action::WinnerReplay, META_MAX).unwrap();
        assert_eq!(
            journal.sends.iter().map(|send| send.bytes).sum::<usize>(),
            MAX_BODY
        );
        assert!(journal.add(0, Action::WinnerReplay, 1).is_err());
        assert!(Journal::default().add(25, Action::Original, 1).is_err());
        assert!(Journal::default()
            .add(18, Action::Original, MULTIPART_MAX + 1)
            .is_err());
    }

    #[test]
    fn consumed_run_and_non_schema_evidence_are_refused() {
        let mut journal = Journal::default();
        journal.admit_run().unwrap();
        journal.started = true;
        assert!(journal.admit_run().is_err());
        assert!(
            serde_json::from_str::<Evidence>(r#"{"kind":"replay","token":"private"}"#).is_err()
        );
        journal.evidence.push(Evidence::Matrix {
            origin: 0,
            reader: 6,
            exact: false,
        });
        assert!(journal.validate().is_err());
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::{
            fs,
            os::unix::fs::{symlink, DirBuilderExt, PermissionsExt},
        };

        struct Scratch(PathBuf);

        impl Scratch {
            fn new() -> Self {
                let root = std::env::temp_dir()
                    .canonicalize()
                    .unwrap()
                    .join(format!("buddy-drive-ledger-{}", Uuid::new_v4()));
                fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
                Self(root)
            }
        }

        impl Drop for Scratch {
            fn drop(&mut self) {
                for name in [
                    "qualification.lock",
                    "frozen.json",
                    "journal.json",
                    "browser.json",
                    "synthetic.json",
                ] {
                    let _ = fs::remove_file(self.0.join(name));
                }
                let _ = fs::remove_dir(&self.0);
            }
        }

        #[test]
        fn private_directory_excludes_a_second_owner_until_release() {
            let scratch = Scratch::new();
            fs::set_permissions(&scratch.0, fs::Permissions::from_mode(0o755)).unwrap();
            assert!(Directory::open(&scratch.0).is_err());
            assert!(!scratch.0.join("qualification.lock").exists());

            fs::set_permissions(&scratch.0, fs::Permissions::from_mode(0o700)).unwrap();
            let owner = Directory::open(&scratch.0).unwrap();
            let second = Directory::open(&scratch.0).err().unwrap();
            assert!(matches!(
                second.downcast_ref::<fs::TryLockError>(),
                Some(fs::TryLockError::WouldBlock)
            ));
            drop(owner);
            assert!(Directory::open(&scratch.0).is_ok());
        }

        #[test]
        fn frozen_ledger_reopens_with_original_hashes_and_refuses_a_consumed_run() {
            let scratch = Scratch::new();
            let owner = Directory::open(&scratch.0).unwrap();
            let frozen = Frozen {
                plan: Plan {
                    version: 1,
                    run: Uuid::from_u128(1),
                    group: Uuid::from_u128(2),
                    buddy_revision: "1".repeat(40),
                    docs_revision: "2".repeat(40),
                    expires_unix: 0,
                    request_seconds: 10,
                    rendezvous_millis: 100,
                    grants: std::array::from_fn(|_| Grant {
                        binding_sha256: "3".repeat(64),
                        stimulate_refresh: false,
                    }),
                },
                ids: (0..IDS).map(|index| format!("synthetic-{index}")).collect(),
                allocator_refreshed: false,
            };
            let mut journal = Journal::default();
            journal.admit_run().unwrap();
            journal.started = true;
            journal.add(0, Action::Original, 100).unwrap();
            let frozen_bytes = serde_json::to_vec(&frozen).unwrap();
            let journal_bytes = serde_json::to_vec(&journal).unwrap();
            owner.save("frozen.json", &frozen).unwrap();
            owner.save("journal.json", &journal).unwrap();
            drop(owner);

            let reopened = Directory::open(&scratch.0).unwrap();
            let (saved_frozen, frozen_sha) = reopened.frozen().unwrap();
            let (saved_journal, journal_sha) = reopened.journal().unwrap();
            assert_eq!(frozen_sha, digest(&frozen_bytes));
            assert_eq!(journal_sha, digest(&journal_bytes));
            assert_eq!(saved_frozen.ids, frozen.ids);
            assert_eq!(saved_journal.sends.len(), 1);
            assert!(!saved_journal.sends[0].returned);
            assert!(saved_journal.admit_run().is_err());
            assert_eq!(reopened.journal().unwrap().1, journal_sha);
        }

        #[test]
        fn grant_path_requires_a_private_dedicated_regular_file() {
            let scratch = Scratch::new();
            let owner = Directory::open(&scratch.0).unwrap();
            let target = scratch.0.join("synthetic.json");
            let grant = scratch.0.join("browser.json");
            owner
                .save("synthetic.json", &"synthetic test bytes")
                .unwrap();

            symlink(&target, &grant).unwrap();
            assert!(owner.grant(0).is_err());
            fs::remove_file(&grant).unwrap();

            fs::hard_link(&target, &grant).unwrap();
            assert!(owner.grant(0).is_err());
            fs::remove_file(&grant).unwrap();

            fs::rename(&target, &grant).unwrap();
            fs::set_permissions(&grant, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(owner.grant(0).is_err());
            fs::set_permissions(&grant, fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(owner.grant(0).unwrap(), grant);
        }
    }
}

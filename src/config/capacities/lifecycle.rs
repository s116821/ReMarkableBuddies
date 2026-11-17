//! Linux-only, per-grant ownership and durable OAuth fences. No HTTP operations.
use super::{private_text, AccessTokenLease, Grant, MAX_GRANT};
use crate::{
    config::{
        inspection::strict_json,
        model_credential::linux::{metadata_hash, owner, parent, read_private},
        Secret,
    },
    note_export::Failure,
    storage::{digest, files, valid_digest, Uuid},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Failure>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Enrollment {
    pub operation_id: Uuid,
    pub tablet_id: String,
    pub connection_id: String,
    pub boot_id: Uuid,
    pub config_sha256: String,
    pub client_id: String,
}

impl Enrollment {
    fn valid(&self) -> bool {
        !self.operation_id.is_nil()
            && !self.boot_id.is_nil()
            && private_text(&self.tablet_id, 256)
            && valid_digest(&self.connection_id)
            && valid_digest(&self.config_sha256)
            && private_text(&self.client_id, 256)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Absent,
    PendingExchange,
    Staged,
    Active,
    PendingRotation,
    LocalDisabled,
    RevocationPending,
    Revoked,
    RetiredUnconfirmed,
}

/// Local evidence only. Spaces and operation identifiers are not credentials.
#[derive(Debug, PartialEq, Eq)]
pub struct Status {
    pub phase: Phase,
    pub grant_id: Uuid,
    pub space_id: Option<Uuid>,
    pub enrollment: Option<Enrollment>,
    pub operation_id: Option<Uuid>,
    pub target_config_sha256: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    lifecycle_version: u32,
    grant_id: Uuid,
    enrollment: Option<Enrollment>,
    target_config_sha256: Option<String>,
    state: State,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "kebab-case", deny_unknown_fields)]
enum State {
    PendingExchange {},
    Staged {
        grant: Grant,
    },
    Active {
        grant: Grant,
    },
    PendingRotation {
        space_id: Uuid,
        operation_id: Uuid,
    },
    LocalDisabled {
        space_id: Uuid,
        revocation: Option<RevocationMaterial>,
    },
    RevocationPending {
        space_id: Uuid,
        operation_id: Uuid,
        revocation: RevocationMaterial,
    },
    Revoked {
        space_id: Uuid,
        operation_id: Uuid,
    },
    RetiredUnconfirmed {
        space_id: Uuid,
        operation_id: Option<Uuid>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RevocationMaterial {
    client_id: String,
    refresh_token: String,
}

impl RevocationMaterial {
    fn valid(&self) -> bool {
        private_text(&self.client_id, 256) && private_text(&self.refresh_token, 4096)
    }
}

struct Snapshot {
    record: Record,
    sha256: String,
}

/// Holds one nonblocking OS lock on a fixed inode for this grant. The lock
/// file is never retired or renamed by application code.
pub struct GrantOwner {
    id: Uuid,
    root: PathBuf,
    root_identity: String,
    record_path: PathBuf,
    lock_path: PathBuf,
    lock_identity: String,
    lock: File,
}

impl GrantOwner {
    pub fn open(root: &Path, id: Uuid) -> Result<Self> {
        if id.is_nil() {
            return Err(Failure::InvalidInput);
        }
        let root_identity = metadata_hash(
            &parent(root).map_err(|_| Failure::MissingCapacitiesCredentials)?,
            false,
        );
        let lock_path = root.join(format!("capacities-grant-{id}.lock"));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
            .open(&lock_path)
            .map_err(|_| Failure::Storage)?;
        let meta = lock.metadata().map_err(|_| Failure::Storage)?;
        if !meta.is_file()
            || meta.nlink() != 1
            || meta.len() != 0
            || !owner(&meta)
            || meta.mode() & 0o7777 != 0o600
        {
            return Err(Failure::Storage);
        }
        let lock_identity = metadata_hash(&meta, true);
        lock.try_lock().map_err(|error| match error {
            fs::TryLockError::WouldBlock => Failure::Conflict,
            fs::TryLockError::Error(_) => Failure::Storage,
        })?;
        let held = Self {
            id,
            root: root.to_owned(),
            root_identity,
            record_path: root.join(format!("capacities-grant-{id}.json")),
            lock_path,
            lock_identity,
            lock,
        };
        held.current()?;
        Ok(held)
    }

    fn current(&self) -> Result<()> {
        let metadata = self.lock.metadata().map_err(|_| Failure::Storage)?;
        let linked = fs::symlink_metadata(&self.lock_path).map_err(|_| Failure::Storage)?;
        let root = parent(&self.root).map_err(|_| Failure::Storage)?;
        if metadata_hash(&metadata, true) != self.lock_identity
            || metadata_hash(&linked, true) != self.lock_identity
            || metadata_hash(&root, false) != self.root_identity
        {
            return Err(Failure::Conflict);
        }
        Ok(())
    }

    fn read(&self) -> Result<Option<Snapshot>> {
        self.current()?;
        let metadata = match fs::symlink_metadata(&self.record_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.current()?;
                return Ok(None);
            }
            Err(_) => return Err(Failure::Storage),
            Ok(value) => value,
        };
        let bytes = read_private(&self.root, &self.record_path, MAX_GRANT)
            .map_err(|_| Failure::MissingCapacitiesCredentials)?;
        self.current()?;
        let raw = strict_json(&bytes).map_err(|_| Failure::MissingCapacitiesCredentials)?;
        if !raw.is_object() {
            return Err(Failure::MissingCapacitiesCredentials);
        }
        let record = if raw.get("lifecycle_version").is_some() {
            if metadata.mode() & 0o7777 != 0o600 {
                return Err(Failure::Storage);
            }
            serde_json::from_value(raw).map_err(|_| Failure::MissingCapacitiesCredentials)?
        } else {
            // The previously supported v1 credential file stays readable and
            // is replaced only by an explicit, fenced lifecycle transition.
            let grant =
                serde_json::from_value(raw).map_err(|_| Failure::MissingCapacitiesCredentials)?;
            Record {
                lifecycle_version: 1,
                grant_id: self.id,
                enrollment: None,
                target_config_sha256: None,
                state: State::Active { grant },
            }
        };
        record.validate(self.id)?;
        Ok(Some(Snapshot {
            record,
            sha256: digest(&bytes),
        }))
    }

    fn publish(&self, before: Option<&str>, record: &Record) -> Result<String> {
        record.validate(self.id)?;
        let bytes = serde_json::to_vec(record).map_err(|_| Failure::Storage)?;
        if bytes.len() as u64 > MAX_GRANT {
            return Err(Failure::Storage);
        }
        if self
            .read()?
            .as_ref()
            .map(|snapshot| snapshot.sha256.as_str())
            != before
        {
            return Err(Failure::Conflict);
        }
        files::atomic(&self.record_path, &bytes).map_err(|_| Failure::Storage)?;
        let observed = self.read()?.ok_or(Failure::Storage)?;
        let hash = digest(&bytes);
        if observed.sha256 != hash {
            return Err(Failure::Conflict);
        }
        Ok(hash)
    }

    pub fn status(&self) -> Result<Status> {
        self.status_after_sync(files::sync_dir)
    }

    fn status_after_sync(&self, sync: impl FnOnce(&Path) -> anyhow::Result<()>) -> Result<Status> {
        let snapshot = self.durable_snapshot(sync)?;
        let (phase, space_id, operation_id) = snapshot
            .as_ref()
            .map_or((Phase::Absent, None, None), |snapshot| {
                snapshot.record.state.summary()
            });
        let (enrollment, target_config_sha256) = snapshot
            .map(|snapshot| {
                (
                    snapshot.record.enrollment,
                    snapshot.record.target_config_sha256,
                )
            })
            .unwrap_or_default();
        let operation_id =
            operation_id.or_else(|| enrollment.as_ref().map(|value| value.operation_id));
        Ok(Status {
            phase,
            grant_id: self.id,
            space_id,
            enrollment,
            operation_id,
            target_config_sha256,
        })
    }

    fn required(&self) -> Result<Snapshot> {
        self.read()?.ok_or(Failure::MissingCapacitiesCredentials)
    }

    fn verify_fence(&self, expected: &str) -> Result<()> {
        if self.required()?.sha256 != expected {
            return Err(Failure::Conflict);
        }
        Ok(())
    }

    fn durable_snapshot(
        &self,
        sync: impl FnOnce(&Path) -> anyhow::Result<()>,
    ) -> Result<Option<Snapshot>> {
        let snapshot = self.read()?;
        if let Some(value) = &snapshot {
            // A previous writer may have seen a rename error after replacing
            // the file. A new owner must establish durability before use.
            sync(&self.root).map_err(|_| Failure::Storage)?;
            self.verify_fence(&value.sha256)?;
        }
        Ok(snapshot)
    }

    pub(super) fn access_token(self, space_id: Uuid, may_write: bool) -> Result<AccessTokenLease> {
        let snapshot = self
            .durable_snapshot(files::sync_dir)?
            .ok_or(Failure::MissingCapacitiesCredentials)?;
        let State::Active { grant } = snapshot.record.state else {
            return Err(Failure::Authentication);
        };
        if grant.space_id != space_id
            || grant.expires_at <= now()?
            || (may_write && !grant.scopes.iter().any(|scope| scope == "api:write"))
        {
            return Err(Failure::Authentication);
        }
        Ok(AccessTokenLease {
            token: Secret(grant.access_token),
            _owner: self,
        })
    }

    /// A caller may exchange once only after this fresh durable fence returns.
    pub fn begin_exchange(self, enrollment: Enrollment) -> Result<Exchange> {
        if !enrollment.valid() {
            return Err(Failure::InvalidInput);
        }
        let record = Record {
            lifecycle_version: 1,
            grant_id: self.id,
            enrollment: Some(enrollment),
            target_config_sha256: None,
            state: State::PendingExchange {},
        };
        let sha256 = self.publish(None, &record)?;
        Ok(Exchange {
            owner: self,
            record,
            sha256,
            dispatched: false,
        })
    }

    /// The configuration publisher is responsible for verifying the exact
    /// committed pointer before passing its readback digest here.
    pub fn activate_after_config_readback(
        &self,
        operation_id: Uuid,
        config_sha256: &str,
    ) -> Result<()> {
        let mut snapshot = self
            .durable_snapshot(files::sync_dir)?
            .ok_or(Failure::MissingCapacitiesCredentials)?;
        let State::Staged { grant } = snapshot.record.state else {
            return Err(Failure::Conflict);
        };
        if snapshot
            .record
            .enrollment
            .as_ref()
            .map(|value| value.operation_id)
            != Some(operation_id)
            || snapshot.record.target_config_sha256.as_deref() != Some(config_sha256)
            || grant.expires_at <= now()?
        {
            return Err(Failure::Conflict);
        }
        snapshot.record.state = State::Active { grant };
        self.publish(Some(&snapshot.sha256), &snapshot.record)?;
        Ok(())
    }

    pub fn begin_rotation(self, space_id: Uuid, operation_id: Uuid) -> Result<Rotation> {
        let mut snapshot = self.required()?;
        let State::Active { grant } = snapshot.record.state else {
            return Err(Failure::Conflict);
        };
        if grant.space_id != space_id || operation_id.is_nil() {
            return Err(Failure::InvalidInput);
        }
        snapshot.record.state = State::PendingRotation {
            space_id,
            operation_id,
        };
        let sha256 = self.publish(Some(&snapshot.sha256), &snapshot.record)?;
        Ok(Rotation {
            owner: self,
            record: snapshot.record,
            sha256,
            space_id,
            client_id: grant.client_id,
            scopes: grant.scopes,
            old_refresh_sha256: digest(grant.refresh_token.as_bytes()),
            refresh: Secret(grant.refresh_token),
            released: false,
        })
    }

    pub fn disable_local(&self, space_id: Uuid) -> Result<()> {
        let mut snapshot = self.required()?;
        let revocation = match snapshot.record.state {
            State::Active { grant } if grant.space_id == space_id => Some(RevocationMaterial {
                client_id: grant.client_id,
                refresh_token: grant.refresh_token,
            }),
            State::PendingRotation {
                space_id: current, ..
            } if current == space_id => None,
            _ => return Err(Failure::Conflict),
        };
        snapshot.record.state = State::LocalDisabled {
            space_id,
            revocation,
        };
        self.publish(Some(&snapshot.sha256), &snapshot.record)?;
        Ok(())
    }

    pub fn begin_revocation(self, space_id: Uuid, operation_id: Uuid) -> Result<Revocation> {
        let mut snapshot = self.required()?;
        let State::LocalDisabled {
            space_id: current,
            revocation: Some(revocation),
        } = snapshot.record.state
        else {
            return Err(Failure::Conflict);
        };
        if current != space_id || operation_id.is_nil() {
            return Err(Failure::InvalidInput);
        }
        snapshot.record.state = State::RevocationPending {
            space_id,
            operation_id,
            revocation,
        };
        let sha256 = self.publish(Some(&snapshot.sha256), &snapshot.record)?;
        Ok(Revocation {
            owner: self,
            record: snapshot.record,
            sha256,
            released: false,
        })
    }

    /// Deliberately discard retained revocation material without claiming the
    /// provider removed an enrollment or an unknown successor token.
    pub fn retire_unconfirmed(&self, space_id: Uuid) -> Result<()> {
        let mut snapshot = self.required()?;
        let (phase, current, operation_id) = snapshot.record.state.summary();
        if current != Some(space_id)
            || !matches!(phase, Phase::LocalDisabled | Phase::RevocationPending)
        {
            return Err(Failure::Conflict);
        }
        snapshot.record.state = State::RetiredUnconfirmed {
            space_id,
            operation_id,
        };
        self.publish(Some(&snapshot.sha256), &snapshot.record)?;
        Ok(())
    }
}

pub struct Exchange {
    owner: GrantOwner,
    record: Record,
    sha256: String,
    dispatched: bool,
}

impl Exchange {
    /// Check the persisted fence at the original caller's only dispatch boundary.
    pub fn authorize_dispatch(&mut self) -> Result<()> {
        if self.dispatched {
            return Err(Failure::Conflict);
        }
        self.owner.verify_fence(&self.sha256)?;
        self.dispatched = true;
        Ok(())
    }

    /// Store verified bytes prior to publishing the configuration pointer.
    pub fn stage(
        mut self,
        space_id: Uuid,
        grant_bytes: &[u8],
        target_config_sha256: String,
    ) -> Result<GrantOwner> {
        if !self.dispatched {
            return Err(Failure::Conflict);
        }
        let grant = replacement(grant_bytes, self.owner.id, space_id)?;
        let enrollment = self.record.enrollment.as_ref().ok_or(Failure::Storage)?;
        if grant.client_id != enrollment.client_id || !valid_digest(&target_config_sha256) {
            return Err(Failure::Conflict);
        }
        self.record.target_config_sha256 = Some(target_config_sha256);
        self.record.state = State::Staged { grant };
        self.owner.publish(Some(&self.sha256), &self.record)?;
        Ok(self.owner)
    }
}

pub struct Rotation {
    owner: GrantOwner,
    record: Record,
    sha256: String,
    space_id: Uuid,
    client_id: String,
    scopes: Vec<String>,
    old_refresh_sha256: String,
    refresh: Secret,
    released: bool,
}

impl Rotation {
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// The borrowed token is available only while this live owner is retained.
    pub fn release_refresh_token(&mut self) -> Result<&str> {
        if self.released {
            return Err(Failure::Conflict);
        }
        self.owner.verify_fence(&self.sha256)?;
        self.released = true;
        Ok(self.refresh.expose())
    }

    pub fn install(mut self, grant_bytes: &[u8]) -> Result<()> {
        if !self.released {
            return Err(Failure::Conflict);
        }
        let grant = replacement(grant_bytes, self.owner.id, self.space_id)?;
        let old_scopes: BTreeSet<_> = self.scopes.iter().collect();
        let new_scopes: BTreeSet<_> = grant.scopes.iter().collect();
        if old_scopes != new_scopes
            || grant.client_id != self.client_id
            || digest(grant.refresh_token.as_bytes()) == self.old_refresh_sha256
        {
            return Err(Failure::Conflict);
        }
        self.record.state = State::Active { grant };
        self.owner.publish(Some(&self.sha256), &self.record)?;
        Ok(())
    }
}

pub struct Revocation {
    owner: GrantOwner,
    record: Record,
    sha256: String,
    released: bool,
}

impl Revocation {
    pub fn client_id(&self) -> &str {
        let State::RevocationPending { revocation, .. } = &self.record.state else {
            unreachable!("revocation owns only a pending record");
        };
        &revocation.client_id
    }

    /// The retained revocation-only token cannot outlive this permit.
    pub fn release_refresh_token(&mut self) -> Result<&str> {
        if self.released {
            return Err(Failure::Conflict);
        }
        self.owner.verify_fence(&self.sha256)?;
        let State::RevocationPending { revocation, .. } = &self.record.state else {
            return Err(Failure::Conflict);
        };
        self.released = true;
        Ok(&revocation.refresh_token)
    }

    /// Call only after a positive provider receipt for this live request.
    pub fn confirm(mut self) -> Result<()> {
        if !self.released {
            return Err(Failure::Conflict);
        }
        let State::RevocationPending {
            space_id,
            operation_id,
            ..
        } = self.record.state
        else {
            return Err(Failure::Conflict);
        };
        self.record.state = State::Revoked {
            space_id,
            operation_id,
        };
        self.owner.publish(Some(&self.sha256), &self.record)?;
        Ok(())
    }
}

impl Record {
    fn validate(&self, id: Uuid) -> Result<()> {
        if self.lifecycle_version != 1 {
            return Err(Failure::Unsupported);
        }
        let (phase, space, operation) = self.state.summary();
        if self.grant_id != id
            || space.is_some_and(|value| value.is_nil())
            || operation.is_some_and(|value| value.is_nil())
            || self.enrollment.as_ref().is_some_and(|value| !value.valid())
            || self
                .target_config_sha256
                .as_ref()
                .is_some_and(|value| !valid_digest(value))
            || (self.enrollment.is_some()
                != (phase == Phase::PendingExchange || self.target_config_sha256.is_some()))
            || (phase == Phase::PendingExchange && self.target_config_sha256.is_some())
            || (phase == Phase::Staged && self.target_config_sha256.is_none())
        {
            return Err(Failure::Conflict);
        }
        match &self.state {
            State::Staged { grant } | State::Active { grant } => {
                grant.validate(id)?;
                if self
                    .enrollment
                    .as_ref()
                    .is_some_and(|binding| binding.client_id != grant.client_id)
                {
                    return Err(Failure::Conflict);
                }
            }
            State::LocalDisabled {
                revocation: Some(value),
                ..
            }
            | State::RevocationPending {
                revocation: value, ..
            } if !value.valid() => return Err(Failure::Conflict),
            _ => {}
        }
        Ok(())
    }
}

impl State {
    fn summary(&self) -> (Phase, Option<Uuid>, Option<Uuid>) {
        match self {
            Self::PendingExchange {} => (Phase::PendingExchange, None, None),
            Self::Staged { grant } => (Phase::Staged, Some(grant.space_id), None),
            Self::Active { grant } => (Phase::Active, Some(grant.space_id), None),
            Self::PendingRotation {
                space_id,
                operation_id,
            } => (Phase::PendingRotation, Some(*space_id), Some(*operation_id)),
            Self::LocalDisabled { space_id, .. } => (Phase::LocalDisabled, Some(*space_id), None),
            Self::RevocationPending {
                space_id,
                operation_id,
                ..
            } => (
                Phase::RevocationPending,
                Some(*space_id),
                Some(*operation_id),
            ),
            Self::Revoked {
                space_id,
                operation_id,
            } => (Phase::Revoked, Some(*space_id), Some(*operation_id)),
            Self::RetiredUnconfirmed {
                space_id,
                operation_id,
            } => (Phase::RetiredUnconfirmed, Some(*space_id), *operation_id),
        }
    }
}

fn replacement(bytes: &[u8], id: Uuid, space_id: Uuid) -> Result<Grant> {
    if bytes.len() as u64 > MAX_GRANT {
        return Err(Failure::InvalidInput);
    }
    let raw = strict_json(bytes).map_err(|_| Failure::InvalidInput)?;
    if !raw.is_object() {
        return Err(Failure::InvalidInput);
    }
    let grant: Grant = serde_json::from_value(raw).map_err(|_| Failure::InvalidInput)?;
    grant.validate(id)?;
    if grant.space_id != space_id || grant.expires_at <= now()? {
        return Err(Failure::Authentication);
    }
    Ok(grant)
}

fn now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Failure::Authentication)?
        .as_secs())
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;

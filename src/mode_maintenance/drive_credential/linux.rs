use super::{ModeReport, Report};
use crate::{
    config::{
        basic::Status,
        inspection::strict_json,
        model_credential::linux::{leaf, Phase},
        Config,
    },
    mode_maintenance::OutputLease,
    storage::{
        digest,
        drive::{Credentials, APP_SCOPE},
        files,
        maintenance::Refusal,
        sync::mode::{SyncMode, SyncModeSnapshot, SyncModeToken},
        valid_digest, Store, Uuid,
    },
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    fs,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_INPUT: usize = 68 * 1024;
const MAX_CREDENTIAL: usize = 48 * 1024;
type Checked<T> = Result<T, Status>;

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ModeToken {
    actor: Uuid,
    sha256: String,
}
impl ModeToken {
    fn valid(&self) -> bool {
        SyncModeToken::from_maintenance_request(self.actor, self.sha256.clone()).is_ok()
    }
}
impl From<&SyncModeToken> for ModeToken {
    fn from(value: &SyncModeToken) -> Self {
        let (actor, sha256) = value.fields();
        Self {
            actor,
            sha256: sha256.into(),
        }
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Slot {
    Absent {},
    Present {
        sha256: String,
        metadata_sha256: String,
        binding_sha256: String,
        #[serde(deserialize_with = "canonical_collection")]
        collection: Uuid,
    },
}

impl Slot {
    fn valid(&self) -> bool {
        match self {
            Self::Absent {} => true,
            Self::Present {
                sha256,
                metadata_sha256,
                binding_sha256,
                collection,
            } => {
                valid_digest(sha256)
                    && valid_digest(metadata_sha256)
                    && valid_binding(binding_sha256, *collection)
            }
        }
    }

    fn binding_matches(&self, binding: &str, id: Uuid) -> bool {
        match self {
            Self::Absent {} => true,
            Self::Present {
                binding_sha256,
                collection,
                ..
            } => binding_sha256 == binding && *collection == id,
        }
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Original {
    config_sha256: String,
    credential_path_sha256: String,
    parent_identity_sha256: String,
    device: Uuid,
    epoch: Uuid,
    mode: SyncMode,
    mode_token: ModeToken,
    slot: Slot,
}

impl Original {
    fn valid(&self) -> bool {
        [
            &self.config_sha256,
            &self.credential_path_sha256,
            &self.parent_identity_sha256,
        ]
        .into_iter()
        .all(|value| valid_digest(value))
            && !self.device.is_nil()
            && !self.epoch.is_nil()
            && self.mode_token.valid()
            && self.slot.valid()
            && match &self.mode {
                SyncMode::LocalOnly => true,
                SyncMode::IsolatedBackup {
                    backup,
                    binding_sha256,
                } => {
                    !backup.is_nil()
                        && valid_digest(binding_sha256)
                        && match &self.slot {
                            Slot::Absent {} => true,
                            Slot::Present {
                                binding_sha256: installed,
                                ..
                            } => binding_sha256 == installed,
                        }
                }
                SyncMode::SharedGroup { .. } => false,
            }
    }

    fn same_custody(&self, other: &Self) -> bool {
        self.config_sha256 == other.config_sha256
            && self.credential_path_sha256 == other.credential_path_sha256
            && self.parent_identity_sha256 == other.parent_identity_sha256
            && self.device == other.device
            && self.epoch == other.epoch
            && self.mode == other.mode
            && self.mode_token == other.mode_token
    }

    fn admits_binding(&self, binding: &str, collection: Uuid) -> bool {
        valid_binding(binding, collection)
            && self.slot.binding_matches(binding, collection)
            && match &self.mode {
                SyncMode::LocalOnly => true,
                SyncMode::IsolatedBackup { binding_sha256, .. } => binding_sha256 == binding,
                SyncMode::SharedGroup { .. } => false,
            }
    }
}

fn valid_binding(binding: &str, collection: Uuid) -> bool {
    valid_digest(binding) && !collection.is_nil()
}

fn canonical_collection<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Uuid, D::Error> {
    let text = String::deserialize(deserializer)?;
    let uuid = Uuid::parse_str(&text).map_err(serde::de::Error::custom)?;
    if uuid.is_nil() || text != uuid.hyphenated().to_string() {
        return Err(serde::de::Error::custom("noncanonical Drive collection"));
    }
    Ok(uuid)
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Change {
    Create {
        #[serde(deserialize_with = "canonical_collection")]
        collection: Uuid,
        binding_sha256: String,
        credential_b64: String,
    },
    Replace {
        #[serde(deserialize_with = "canonical_collection")]
        collection: Uuid,
        binding_sha256: String,
        credential_b64: String,
    },
    Remove {
        #[serde(deserialize_with = "canonical_collection")]
        collection: Uuid,
        binding_sha256: String,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Desired {
    Absent {},
    Present {
        sha256: String,
        binding_sha256: String,
        #[serde(deserialize_with = "canonical_collection")]
        collection: Uuid,
    },
}
impl Desired {
    fn valid_for(&self, original: &Original) -> bool {
        match self {
            Self::Absent {} => matches!(original.slot, Slot::Present { .. }),
            Self::Present {
                sha256,
                binding_sha256,
                collection,
            } => {
                valid_digest(sha256)
                    && original.admits_binding(binding_sha256, *collection)
                    && !self.matches(&original.slot)
            }
        }
    }

    fn matches(&self, observed: &Slot) -> bool {
        match (self, observed) {
            (Self::Absent {}, Slot::Absent {}) => true,
            (
                Self::Present {
                    sha256,
                    binding_sha256,
                    collection,
                },
                Slot::Present {
                    sha256: seen,
                    binding_sha256: binding,
                    collection: id,
                    ..
                },
            ) => sha256 == seen && binding_sha256 == binding && collection == id,
            _ => false,
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
enum Request {
    Inspect {
        control_version: u32,
        expected_mode: ModeToken,
    },
    Apply {
        control_version: u32,
        operation: Uuid,
        original: Original,
        change: Change,
    },
    Readback {
        control_version: u32,
        operation: Uuid,
        original: Original,
        desired: Desired,
    },
}

impl Request {
    fn identity(&self) -> (Option<&'static str>, Option<Uuid>) {
        match self {
            Self::Inspect { .. } => (Some("inspect"), None),
            Self::Apply { operation, .. } => (Some("apply"), Some(*operation)),
            Self::Readback { operation, .. } => (Some("readback"), Some(*operation)),
        }
    }

    fn validate(&self) -> Checked<()> {
        let (version, valid) = match self {
            Self::Inspect {
                control_version,
                expected_mode,
            } => (*control_version, expected_mode.valid()),
            Self::Apply {
                control_version,
                operation,
                original,
                ..
            } => (*control_version, !operation.is_nil() && original.valid()),
            Self::Readback {
                control_version,
                operation,
                original,
                desired,
            } => (
                *control_version,
                !operation.is_nil() && original.valid() && desired.valid_for(original),
            ),
        };
        if version != 1 {
            return Err(Status::Unsupported);
        }
        if !valid {
            return Err(Status::InvalidInput);
        }
        Ok(())
    }
}

struct Observed {
    target: PathBuf,
    original: Original,
    mode: Option<u32>,
}

fn valid_config(config: &Config, path: &Path) -> Checked<()> {
    if config.sync.enabled
        || config.reader.selection.is_some()
        || config.memory.selection.is_some()
        || path.starts_with(&config.paths.credentials)
        || config.paths.credentials.starts_with(path)
    {
        return Err(Status::Unsupported);
    }
    files::safe_path(path).map_err(|_| Status::Unsupported)
}

fn observe(path: &Path, store: &Store, mode: SyncModeSnapshot) -> Checked<Observed> {
    let config = Config::load(Some(path.to_owned())).map_err(|_| Status::Unavailable)?;
    valid_config(&config, path)?;
    if config.paths.data != store.paths.data
        || config.paths.cache != store.paths.cache
        || config.paths.credentials != store.paths.credentials
    {
        return Err(Status::Conflict);
    }
    let config_sha256 = config
        .source_snapshot
        .as_ref()
        .and_then(|saved| saved.fingerprint().1)
        .ok_or(Status::Unavailable)?
        .to_owned();
    let config_meta = fs::symlink_metadata(path).map_err(|_| Status::Unavailable)?;
    if !config_meta.is_file() {
        return Err(Status::Unsupported);
    }
    let root = &store.paths.credentials;
    let parent_identity_sha256 = leaf::parent_identity(root)?;
    store
        .verify_drive_credential_root()
        .map_err(refusal_status)?;
    let root_path = fs::canonicalize(root).map_err(|_| Status::Unavailable)?;
    let root_meta = fs::symlink_metadata(root).map_err(|_| Status::Unavailable)?;
    let config_path = fs::canonicalize(path).map_err(|_| Status::Unavailable)?;
    if root_path.starts_with(&config_path) || config_path.starts_with(&root_path) {
        return Err(Status::Unsupported);
    }
    for other in [&store.paths.cache, &store.paths.data] {
        let canonical = fs::canonicalize(other).map_err(|_| Status::Unavailable)?;
        let meta = fs::metadata(other).map_err(|_| Status::Unavailable)?;
        if canonical.starts_with(&root_path)
            || root_path.starts_with(&canonical)
            || (meta.dev(), meta.ino()) == (root_meta.dev(), root_meta.ino())
        {
            return Err(Status::Unsupported);
        }
    }
    let target = root.join("drive.json");
    let credential_path_sha256 = digest(
        format!(
            "urn:remarkable-buddies:drive-credential-path:v1:{}",
            target.to_str().ok_or(Status::Unsupported)?
        )
        .as_bytes(),
    );
    let selected = mode.mode.ok_or(Status::Unsupported)?;
    if matches!(selected, SyncMode::SharedGroup { .. }) {
        return Err(Status::Unsupported);
    }
    let (slot, mode_bits) = match leaf::observe(&target, &config_meta, MAX_CREDENTIAL)? {
        None => (Slot::Absent {}, None),
        Some(leaf) => {
            let value = decode(&leaf.bytes).map_err(|_| Status::Unsupported)?;
            (
                Slot::Present {
                    sha256: digest(&leaf.bytes),
                    metadata_sha256: leaf.metadata_sha256,
                    binding_sha256: value.binding_digest(),
                    collection: value.collection,
                },
                Some(leaf.mode),
            )
        }
    };
    store
        .verify_drive_credential_root()
        .map_err(refusal_status)?;
    if leaf::parent_identity(root)? != parent_identity_sha256
        || digest(
            &files::read(path, crate::storage::MAX_RECORD as u64)
                .map_err(|_| Status::Unavailable)?,
        ) != config_sha256
    {
        return Err(Status::Conflict);
    }
    let original = Original {
        config_sha256,
        credential_path_sha256,
        parent_identity_sha256,
        device: mode.device.ok_or(Status::Unsupported)?,
        epoch: mode.epoch.ok_or(Status::Unsupported)?,
        mode: selected,
        mode_token: ModeToken::from(&mode.token),
        slot,
    };
    if !original.valid() {
        return Err(Status::Conflict);
    }
    Ok(Observed {
        target,
        original,
        mode: mode_bits,
    })
}

fn decode(bytes: &[u8]) -> Checked<Credentials> {
    let value = strict_json(bytes).map_err(|_| Status::InvalidInput)?;
    let credentials: Credentials =
        serde_json::from_value(value).map_err(|_| Status::InvalidInput)?;
    credentials.validate().map_err(|_| Status::InvalidInput)?;
    Ok(credentials)
}

fn unix_seconds() -> Checked<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| Status::Unavailable)
}

fn credential_bytes(encoded: &str, binding: &str, collection: Uuid) -> Checked<(Vec<u8>, u64)> {
    let bytes = STANDARD.decode(encoded).map_err(|_| Status::InvalidInput)?;
    if bytes.len() > MAX_CREDENTIAL || STANDARD.encode(&bytes) != encoded {
        return Err(Status::InvalidInput);
    }
    let raw = strict_json(&bytes).map_err(|_| Status::InvalidInput)?;
    if raw.get("collection").and_then(serde_json::Value::as_str)
        != Some(collection.hyphenated().to_string().as_str())
    {
        return Err(Status::InvalidInput);
    }
    let value: Credentials = serde_json::from_value(raw).map_err(|_| Status::InvalidInput)?;
    value.validate().map_err(|_| Status::InvalidInput)?;
    let current_seconds = unix_seconds()?;
    let ascii_secret = |text: &str, maximum: usize| {
        !text.is_empty()
            && text.len() <= maximum
            && text.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    };
    if value.collection != collection
        || value.binding_digest() != binding
        || value.scopes != [APP_SCOPE]
        || !ascii_secret(&value.access_token, 16_384)
        || !ascii_secret(&value.refresh_token, 16_384)
        || !ascii_secret(&value.client_id, 256)
        || !ascii_secret(&value.account_permission_id, 200)
        || value
            .client_secret
            .as_ref()
            .is_some_and(|secret| !ascii_secret(secret, 1024))
        || value.expires_at <= current_seconds
        || value.expires_at > current_seconds.saturating_add(86_400)
    {
        return Err(Status::InvalidInput);
    }
    Ok((bytes, value.expires_at))
}

enum Publication {
    Create(Vec<u8>),
    Replace(Vec<u8>, u32),
    Remove,
}

fn prepare(change: Change, observed: &Observed) -> Checked<(Publication, Desired, Option<u64>)> {
    let original = &observed.original;
    let (collection, binding_sha256, encoded) = match change {
        Change::Create {
            collection,
            binding_sha256,
            credential_b64,
        } if observed.mode.is_none() => (collection, binding_sha256, credential_b64),
        Change::Replace {
            collection,
            binding_sha256,
            credential_b64,
        } if observed.mode.is_some() => (collection, binding_sha256, credential_b64),
        Change::Remove {
            collection,
            binding_sha256,
        } if observed.mode.is_some() => {
            if !original.admits_binding(&binding_sha256, collection) {
                return Err(Status::Conflict);
            }
            return Ok((Publication::Remove, Desired::Absent {}, None));
        }
        _ => return Err(Status::Conflict),
    };
    if !original.admits_binding(&binding_sha256, collection) {
        return Err(Status::Conflict);
    }
    let (bytes, expires_at) = credential_bytes(&encoded, &binding_sha256, collection)?;
    let desired = Desired::Present {
        sha256: digest(&bytes),
        binding_sha256,
        collection,
    };
    if !desired.valid_for(original) {
        return Err(Status::Conflict);
    }
    let publication = match observed.mode {
        Some(mode) => Publication::Replace(bytes, mode),
        None => Publication::Create(bytes),
    };
    Ok((publication, desired, Some(expires_at)))
}

fn refusal_status(error: Refusal) -> Status {
    match error {
        Refusal::Busy => Status::Busy,
        Refusal::Unsupported => Status::Unsupported,
        Refusal::Unavailable => Status::Unavailable,
    }
}

fn refusal(identity: (Option<&'static str>, Option<Uuid>), status: Status) -> Report {
    let outcome = match status {
        Status::InvalidInput => "invalid-input",
        Status::Unsupported => "unsupported",
        Status::Busy => "busy",
        Status::Conflict => "conflict",
        _ => "unavailable",
    };
    Report::new(identity.0, identity.1, outcome)
}

fn evidence(
    identity: (Option<&'static str>, Option<Uuid>),
    outcome: &'static str,
    observed: &Observed,
) -> Checked<Report> {
    let mut report = Report::new(identity.0, identity.1, outcome);
    report.current =
        Some(serde_json::to_value(&observed.original).map_err(|_| Status::Unavailable)?);
    report.disk_readback_verified = true;
    Ok(report)
}

fn handle(
    request: Request,
    observe_current: impl Fn() -> Checked<Observed>,
    hook: impl FnMut(Phase) -> Checked<()>,
) -> Report {
    let identity = request.identity();
    let mut attempted = false;
    let result = (|| {
        let current = match observe_current() {
            Ok(current) => current,
            Err(_) if matches!(&request, Request::Readback { .. }) => {
                return Ok(Report::new(identity.0, identity.1, "unresolved"));
            }
            Err(status) => return Err(status),
        };
        match request {
            Request::Inspect { expected_mode, .. } => {
                if current.original.mode_token != expected_mode {
                    return Err(Status::Conflict);
                }
                evidence(identity, "observed", &current)
            }
            Request::Apply {
                original, change, ..
            } => {
                if current.original != original {
                    return Err(Status::Conflict);
                }
                let (publication, desired, expires_at) = prepare(change, &current)?;
                let mutation = match &publication {
                    Publication::Create(bytes) => leaf::Mutation::Create(bytes, 0o600),
                    Publication::Replace(bytes, mode) => leaf::Mutation::Replace(bytes, *mode),
                    Publication::Remove => leaf::Mutation::Remove,
                };
                leaf::publish(
                    &current.target,
                    mutation,
                    || {
                        if observe_current()?.original != original {
                            return Err(Status::Conflict);
                        }
                        if let Some(expires_at) = expires_at {
                            if expires_at <= unix_seconds()? {
                                return Err(Status::InvalidInput);
                            }
                        }
                        Ok(())
                    },
                    hook,
                    &mut attempted,
                )?;
                let observed = observe_current()?;
                let expected_mode = match publication {
                    Publication::Create(_) => Some(0o600),
                    Publication::Replace(_, mode) => Some(mode),
                    Publication::Remove => None,
                };
                if !observed.original.same_custody(&original)
                    || !desired.matches(&observed.original.slot)
                    || observed.mode != expected_mode
                {
                    return Err(Status::Unavailable);
                }
                evidence(identity, "verified", &observed)
            }
            Request::Readback {
                original, desired, ..
            } => {
                if !current.original.same_custody(&original) {
                    return Ok(Report::new(identity.0, identity.1, "unresolved"));
                }
                if desired.matches(&current.original.slot) {
                    evidence(identity, "desired-state-observed", &current)
                } else if current.original.slot == original.slot {
                    evidence(identity, "original-state-observed", &current)
                } else {
                    Ok(Report::new(identity.0, identity.1, "unresolved"))
                }
            }
        }
    })();
    match result {
        Ok(report) => report,
        Err(_) if attempted => Report::new(identity.0, identity.1, "outcome-unknown"),
        Err(status) => refusal(identity, status),
    }
}

fn read_request(stdin: &mut impl Read) -> Checked<Request> {
    let mut bytes = Vec::new();
    stdin
        .take((MAX_INPUT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| Status::InvalidInput)?;
    if bytes.len() > MAX_INPUT {
        return Err(Status::InvalidInput);
    }
    serde_json::from_value(strict_json(&bytes).map_err(|_| Status::InvalidInput)?)
        .map_err(|_| Status::InvalidInput)
}

pub(super) fn run(
    report: ModeReport,
    config_path: Option<PathBuf>,
    stdin: &mut impl Read,
) -> ModeReport {
    run_with(report, config_path, stdin, |_| Ok(()))
}

fn run_with(
    mut report: ModeReport,
    config_path: Option<PathBuf>,
    stdin: &mut impl Read,
    hook: impl FnMut(Phase) -> Checked<()>,
) -> ModeReport {
    let mut identity = (None, None);
    let result = (|| {
        let request = read_request(stdin)?;
        identity = request.identity();
        request.validate()?;
        let path = config_path.as_deref().ok_or(Status::Unavailable)?;
        let config = Config::load(config_path.clone()).map_err(|_| Status::Unavailable)?;
        valid_config(&config, path)?;
        let store = Arc::new(Store::open_existing_for_mode(config.paths).map_err(refusal_status)?);
        if !store
            .ordinary_isolated_family()
            .map_err(|_| Status::Unavailable)?
        {
            return Err(Status::Unsupported);
        }
        let snapshot = store
            .sync_mode_snapshot()
            .map_err(|_| Status::Unavailable)?;
        report._mode = Some(OutputLease::acquire(&store, &snapshot).map_err(|_| Status::Conflict)?);
        let result = store
            .with_credential_mode(|read_mode| {
                handle(
                    request,
                    || observe(path, &store, read_mode().map_err(|_| Status::Unavailable)?),
                    hook,
                )
            })
            .map_err(|_| Status::Unavailable)?;
        report._store = Some(store);
        Ok(result)
    })();
    report.drive_credential = Some(result.unwrap_or_else(|status| refusal(identity, status)));
    report
}

#[cfg(test)]
mod tests;

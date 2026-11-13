//! Explicit provisioning and current admission for an ordinary isolated root.
use super::*;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    control_version: u32,
    operation: Uuid,
    expected: Expected,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Expected {
    actor: Uuid,
    sha256: String,
}

impl Request {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        let value = crate::config::inspection::strict_json(bytes)
            .map_err(|_| anyhow::anyhow!("invalid isolated provisioning request"))?;
        let request: Self = serde_json::from_value(value)?;
        request.validate()?;
        Ok(request)
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            self.control_version == 1 && !self.operation.is_nil(),
            "invalid root intent"
        );
        self.token()?;
        Ok(())
    }
    fn token(&self) -> Result<mode::SyncModeToken> {
        mode::SyncModeToken::from_maintenance_request(
            self.expected.actor,
            self.expected.sha256.clone(),
        )
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum State {
    RootVerified,
    OutcomeUnknown,
    AuthorizationRequired,
    RetryLater,
    RecoveryRequired,
    ProviderBindingUnqualified,
}

#[derive(Serialize)]
pub(crate) struct Outcome {
    pub outcome: State,
    operation: Uuid,
    device: Uuid,
    backup: Uuid,
    epoch: Uuid,
    currency: Currency,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum Currency {
    Current,
    Historical,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct RootRecord {
    request: Request,
    binding: Binding,
    admissions: BTreeMap<Uuid, Admission>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Admission {
    request: Request,
    epoch: Uuid,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Candidate {
    provisioning_version: u32,
    request: Request,
    actor: Uuid,
    device: Uuid,
    backup: Uuid,
    epoch: Uuid,
    grant: String,
    root: Option<String>,
}

/// The two shapes share a single checkpoint path. A verified root and its
/// empty initial publication progress are committed by one atomic write.
#[derive(Deserialize)]
#[serde(untagged)]
enum Custody {
    Candidate(Candidate),
    Publisher(Box<Checkpoint>),
}

#[derive(Debug)]
pub(crate) enum Failure {
    Conflict,
    Unsupported,
    Recovery,
}

impl Candidate {
    fn new(request: &Request, original: &mode::IsolatedOriginal) -> Self {
        Self {
            provisioning_version: 1,
            actor: request.expected.actor,
            request: request.clone(),
            device: original.device,
            backup: original.backup,
            epoch: original.epoch,
            grant: original.grant.clone(),
            root: None,
        }
    }

    fn matches(&self, request: &Request, original: &mode::IsolatedOriginal) -> bool {
        self.provisioning_version == 1
            && &self.request == request
            && self.actor == request.expected.actor
            && self.device == original.device
            && self.backup == original.backup
            && self.epoch == original.epoch
            && self.grant == original.grant
    }

    fn binding(&self) -> Result<Binding> {
        let root = self.root.clone().context("original root ID absent")?;
        validate_properties(&BTreeMap::from([("predecessor".into(), root.clone())]))?;
        Ok(Binding {
            actor: self.actor,
            device: self.device,
            backup: self.backup,
            epoch: self.epoch,
            grant: self.grant.clone(),
            root,
        })
    }
}

impl RootRecord {
    pub(super) fn validate(&self, store: &Store, publisher: &Binding) -> Result<()> {
        self.request.validate()?;
        let history = mode::isolated_history(store)?;
        ensure!(self.binding.actor == store.actor_id, "root actor differs");
        history.verify(
            self.request.operation,
            &self.request.token()?,
            (
                self.binding.backup,
                self.binding.device,
                &self.binding.grant,
                self.binding.epoch,
            ),
        )?;
        ensure!(
            self.admissions.len() <= MAX_ITEMS,
            "isolated admission limit exceeded"
        );
        let baseline = Binding {
            epoch: publisher.epoch,
            ..self.binding.clone()
        };
        ensure!(publisher == &baseline, "publisher root identity differs");
        let mut active_is_known = publisher.epoch == self.binding.epoch;
        for (operation, admission) in &self.admissions {
            admission.request.validate()?;
            ensure!(
                *operation == admission.request.operation && *operation != self.request.operation,
                "admission operation differs"
            );
            history.verify(
                *operation,
                &admission.request.token()?,
                (
                    self.binding.backup,
                    self.binding.device,
                    &self.binding.grant,
                    admission.epoch,
                ),
            )?;
            active_is_known |= admission.epoch == publisher.epoch;
        }
        ensure!(active_is_known, "current publisher admission is absent");
        validate_properties(&self.binding.root_properties())?;
        Ok(())
    }
}

/// No production implementation is eligible until installed qualification
/// supplies the supported app/grant handoff. The injected fixture invokes
/// the same control and rooted Worker constructor with a loopback adapter.
pub(in crate::storage::sync) fn installed_eligible() -> bool {
    false
}

pub(crate) fn run(store: Arc<Store>, request: Request) -> std::result::Result<Outcome, Failure> {
    let token = request.token().map_err(|_| Failure::Conflict)?;
    let original = mode::isolated_original(&store, request.operation, &token)
        .map_err(|_| Failure::Recovery)?
        .ok_or(Failure::Conflict)?;
    if !store
        .ordinary_isolated_family()
        .map_err(|_| Failure::Recovery)?
    {
        return Err(Failure::Unsupported);
    }
    if !installed_eligible() {
        return Ok(outcome(
            &request,
            &original,
            State::ProviderBindingUnqualified,
        ));
    }
    let drive = GoogleDrive::open(store.paths.credentials.join("drive.json"));
    run_with(store, request, drive)
}

fn outcome(request: &Request, original: &mode::IsolatedOriginal, state: State) -> Outcome {
    Outcome {
        outcome: state,
        operation: request.operation,
        device: original.device,
        backup: original.backup,
        epoch: original.epoch,
        currency: if original.current {
            Currency::Current
        } else {
            Currency::Historical
        },
    }
}

fn checkpoint_dir(store: &Store, backup: Uuid) -> PathBuf {
    store
        .paths
        .data
        .join("sync/isolated")
        .join(backup.to_string())
}
fn publisher_lock(directory: &Path) -> Result<File> {
    publisher_directory(directory)?;
    let lock_path = directory.join("publisher.lock");
    files::safe_path(&lock_path)?;
    if !absent(&lock_path)? {
        ensure!(
            fs::symlink_metadata(&lock_path)?.is_file(),
            "invalid publisher lock"
        );
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    file.try_lock()?;
    Ok(file)
}

fn publisher_directory(path: &Path) -> Result<()> {
    if absent(path)? {
        return files::directory(path);
    }
    ensure!(
        fs::symlink_metadata(path)?.is_dir(),
        "invalid publisher directory"
    );
    Ok(())
}

fn read_publisher_custody(directory: &Path) -> Result<Option<Custody>> {
    let namespace = directory
        .parent()
        .context("isolated checkpoint parent missing")?;
    // Windows may report NotFound for a child of a regular file. Check
    // each owned directory itself before treating an absent child as new.
    for parent in [namespace, directory] {
        if absent(parent)? {
            return Ok(None);
        }
        ensure!(
            fs::symlink_metadata(parent)?.is_dir(),
            "invalid publisher directory"
        );
    }
    let checkpoint = directory.join("publisher.json");
    if absent(&checkpoint)? {
        return Ok(None);
    }
    read_private(&checkpoint).map(Some)
}

fn map_provider(error: anyhow::Error, unknown: bool) -> State {
    if error.is::<InvalidRemoteData>() {
        return State::RecoveryRequired;
    }
    if let Some(failure) = error.downcast_ref::<TransportFailure>() {
        if failure.authorization_required {
            return State::AuthorizationRequired;
        }
        if !failure.retryable && failure.status != 404 {
            return State::RecoveryRequired;
        }
    }
    if unknown {
        State::OutcomeUnknown
    } else {
        State::RetryLater
    }
}

pub(crate) fn run_with(
    store: Arc<Store>,
    request: Request,
    drive: Result<GoogleDrive>,
) -> std::result::Result<Outcome, Failure> {
    let token = request.token().map_err(|_| Failure::Conflict)?;
    let original = mode::isolated_original(&store, request.operation, &token)
        .map_err(|_| Failure::Recovery)?
        .ok_or(Failure::Conflict)?;
    if !store
        .ordinary_isolated_family()
        .map_err(|_| Failure::Recovery)?
    {
        return Err(Failure::Unsupported);
    }
    let _lease = if original.current {
        Some(
            store
                .sync_mode_lease(&token)
                .map_err(|_| Failure::Conflict)?,
        )
    } else {
        None
    };
    let result = advance(&store, &request, &original, drive)?;
    let mut result = outcome(&request, &original, result);
    if store
        .sync_mode_snapshot()
        .map_err(|_| Failure::Recovery)?
        .token
        != original.token
    {
        result.currency = Currency::Historical;
    }
    Ok(result)
}

fn advance(
    store: &Store,
    request: &Request,
    original: &mode::IsolatedOriginal,
    drive: Result<GoogleDrive>,
) -> std::result::Result<State, Failure> {
    let directory = checkpoint_dir(store, original.backup);
    let path = directory.join("publisher.json");
    let _exclusive = publisher_lock(&directory).map_err(|_| Failure::Recovery)?;
    let missing = absent(&path).map_err(|_| Failure::Recovery)?;
    let reservations =
        mode::isolated_reservations(store, original.backup).map_err(|_| Failure::Recovery)?;
    if missing && !reservations.is_empty() {
        return Ok(State::RecoveryRequired);
    }
    let custody = if missing {
        ensure_current(original)?;
        let candidate = Candidate::new(request, original);
        write_private(&path, &candidate).map_err(|_| Failure::Recovery)?;
        Custody::Candidate(candidate)
    } else {
        read_private(&path).map_err(|_| Failure::Recovery)?
    };
    if let Custody::Candidate(candidate) = &custody {
        if !candidate.matches(request, original) {
            return if candidate.epoch == original.epoch {
                Err(Failure::Conflict)
            } else {
                Ok(State::RecoveryRequired)
            };
        }
        if !original.reserved {
            ensure_current(original)?;
            if candidate.root.is_some() {
                return Ok(State::RecoveryRequired);
            }
            mode::reserve_isolated(store, request.operation, original)
                .map_err(|_| Failure::Recovery)?;
        }
    }
    let mut drive = match drive {
        Ok(drive) => drive,
        Err(_) => return Ok(State::AuthorizationRequired),
    };
    drive.set_request_window(
        Some(std::time::Instant::now() + Duration::from_secs(30)),
        None,
    );
    match custody {
        Custody::Candidate(candidate) => {
            advance_candidate(store, request, original, &path, candidate, &mut drive)
        }
        Custody::Publisher(checkpoint) => {
            advance_verified(store, request, original, &path, *checkpoint, &mut drive)
        }
    }
}

fn ensure_current(original: &mode::IsolatedOriginal) -> std::result::Result<(), Failure> {
    if original.current {
        Ok(())
    } else {
        Err(Failure::Conflict)
    }
}

fn advance_candidate(
    store: &Store,
    request: &Request,
    original: &mode::IsolatedOriginal,
    path: &Path,
    mut candidate: Candidate,
    drive: &mut GoogleDrive,
) -> std::result::Result<State, Failure> {
    let reservation = mode::isolated_original(store, request.operation, &original.token)
        .map_err(|_| Failure::Recovery)?
        .filter(|entry| entry.reserved)
        .ok_or(Failure::Recovery)?;
    if candidate.root.is_none() && !reservation.current {
        return Ok(State::OutcomeUnknown);
    }
    if let Err(error) = drive.qualify_isolated_grant(&candidate.grant) {
        return Ok(map_provider(error, false));
    }
    if candidate.root.is_none() {
        let root = match drive.allocate() {
            Ok(root) => root,
            Err(error) => return Ok(map_provider(error, false)),
        };
        candidate.root = Some(root);
        candidate.binding().map_err(|_| Failure::Recovery)?;
        write_private(path, &candidate).map_err(|_| Failure::Recovery)?;
        if read_private::<Candidate>(path).map_err(|_| Failure::Recovery)? != candidate {
            return Ok(State::RecoveryRequired);
        }
    }
    let binding = candidate.binding().map_err(|_| Failure::Recovery)?;
    let properties = binding.root_properties();
    match drive.observe_isolated_root(&binding.root, &properties) {
        Ok(true) => {}
        Ok(false) if reservation.current => {
            let sent = drive.create_isolated_root(&binding.root, &properties, |drive| {
                if !drive.original_grant_present(&binding.grant) {
                    return Err(isolated_authorization_required());
                }
                let found = integrity(mode::isolated_original(
                    store,
                    request.operation,
                    &original.token,
                ))?;
                if !found.is_some_and(|entry| entry.current && entry.reserved)
                    || integrity(read_private::<Candidate>(path))? != candidate
                {
                    return Err(InvalidRemoteData.into());
                }
                Ok(())
            });
            if let Err(error) = sent {
                return Ok(map_provider(error, true));
            }
            match drive.observe_isolated_root(&binding.root, &properties) {
                Ok(true) => {}
                Ok(false) => return Ok(State::OutcomeUnknown),
                Err(error) => return Ok(map_provider(error, true)),
            }
        }
        Ok(false) => return Ok(State::OutcomeUnknown),
        Err(error) => return Ok(map_provider(error, true)),
    }
    let checkpoint = Checkpoint {
        format: 1,
        binding: binding.clone(),
        latest: None,
        attempt: None,
        settled_stage: None,
        provisioning: Some(RootRecord {
            request: request.clone(),
            binding,
            admissions: BTreeMap::new(),
        }),
    };
    write_private(path, &checkpoint).map_err(|_| Failure::Recovery)?;
    let observed =
        Engine::read_checkpoint(path, &checkpoint.binding).map_err(|_| Failure::Recovery)?;
    if observed.provisioning != checkpoint.provisioning {
        return Ok(State::RecoveryRequired);
    }
    Ok(State::RootVerified)
}

fn advance_verified(
    store: &Store,
    request: &Request,
    original: &mode::IsolatedOriginal,
    path: &Path,
    mut checkpoint: Checkpoint,
    drive: &mut GoogleDrive,
) -> std::result::Result<State, Failure> {
    checkpoint
        .validate(&checkpoint.binding)
        .map_err(|_| Failure::Recovery)?;
    let record = checkpoint.provisioning.as_mut().ok_or(Failure::Recovery)?;
    record
        .validate(store, &checkpoint.binding)
        .map_err(|_| Failure::Recovery)?;
    let replay = if record.request.operation == request.operation {
        if &record.request != request {
            return Err(Failure::Conflict);
        }
        true
    } else if let Some(admission) = record.admissions.get(&request.operation) {
        if &admission.request != request {
            return Err(Failure::Conflict);
        }
        true
    } else {
        false
    };
    if !replay {
        ensure_current(original)?;
        if checkpoint.attempt.is_some()
            || checkpoint.settled_stage.is_some()
            || checkpoint.binding.epoch == original.epoch
        {
            return Ok(State::RecoveryRequired);
        }
        if !original.reserved {
            mode::reserve_isolated(store, request.operation, original)
                .map_err(|_| Failure::Recovery)?;
        }
        let verified_intent = mode::isolated_original(store, request.operation, &original.token)
            .map_err(|_| Failure::Recovery)?;
        if !verified_intent.is_some_and(|intent| intent.current && intent.reserved) {
            return Err(Failure::Conflict);
        }
    }
    if let Err(error) = drive.qualify_isolated_grant(&record.binding.grant) {
        return Ok(map_provider(error, false));
    }
    match drive.observe_isolated_root(&record.binding.root, &record.binding.root_properties()) {
        Ok(true) => {}
        Ok(false) => return Ok(State::RecoveryRequired),
        Err(error) => return Ok(map_provider(error, false)),
    }
    if !replay {
        record.admissions.insert(
            request.operation,
            Admission {
                request: request.clone(),
                epoch: original.epoch,
            },
        );
        checkpoint.binding.epoch = original.epoch;
        write_private(path, &checkpoint).map_err(|_| Failure::Recovery)?;
        let observed =
            Engine::read_checkpoint(path, &checkpoint.binding).map_err(|_| Failure::Recovery)?;
        if observed.provisioning != checkpoint.provisioning {
            return Ok(State::RecoveryRequired);
        }
    }
    Ok(State::RootVerified)
}

pub(in crate::storage::sync) fn launch_rooted(
    store: Arc<Store>,
    policy: SyncPolicy,
    config: &Path,
    drive: Result<GoogleDrive>,
) -> Result<Option<Worker>> {
    let snapshot = store.sync_mode_snapshot()?;
    let Some(mode::SyncMode::IsolatedBackup { backup, .. }) = snapshot.mode else {
        return Ok(None);
    };
    ensure!(
        !policy.enabled && store.ordinary_isolated_family()?,
        "isolated Worker requires ordinary format 1"
    );
    let directory = checkpoint_dir(&store, backup);
    let path = directory.join("publisher.json");
    let reservations = mode::isolated_reservations(&store, backup)?;
    let state = read_publisher_custody(&directory).transpose();
    let (state, reason, status) = match state {
        Some(Ok(Custody::Publisher(publisher))) => {
            if publisher.validate(&publisher.binding).is_err()
                || publisher
                    .provisioning
                    .as_ref()
                    .is_none_or(|record| record.validate(&store, &publisher.binding).is_err())
            {
                (None, "root-custody-invalid", Status::RecoveryRequired)
            } else if !publisher.binding.matches(&store, &snapshot) {
                (
                    Some(*publisher),
                    "root-admission-stale",
                    Status::RecoveryRequired,
                )
            } else {
                (
                    Some(*publisher),
                    "authorization-required",
                    Status::AuthorizationRequired,
                )
            }
        }
        Some(Ok(Custody::Candidate(candidate))) => {
            let original = candidate.request.token().and_then(|token| {
                mode::isolated_original(&store, candidate.request.operation, &token)
            });
            if candidate.request.validate().is_ok()
                && original.ok().flatten().is_some_and(|original| {
                    candidate.matches(&candidate.request, &original)
                        && (original.reserved || candidate.root.is_none())
                })
                && candidate
                    .root
                    .as_ref()
                    .is_none_or(|_| candidate.binding().is_ok())
            {
                (None, "root-provisioning-unresolved", Status::Pending)
            } else {
                (None, "root-custody-invalid", Status::RecoveryRequired)
            }
        }
        None if reservations.is_empty() => (
            None,
            "root-provisioning-required",
            Status::UnsupportedOrCorrupt,
        ),
        _ => (None, "root-custody-invalid", Status::RecoveryRequired),
    };
    if let (Some(state), Ok(transport)) = (&state, drive) {
        if state.binding.matches(&store, &snapshot) {
            let binding = state.binding.clone();
            let lease = store.sync_mode_lease(&snapshot.token)?;
            let local = (|| -> Result<_> {
                let exclusive = publisher_lock(&directory)?;
                let parent = store
                    .paths
                    .data
                    .parent()
                    .context("Store parent unavailable")?;
                let staging = parent
                    .join(format!(".buddy-isolated-{}", store.actor_id))
                    .join(backup.to_string());
                let sessions = store
                    .paths
                    .credentials
                    .join("isolated")
                    .join(backup.to_string());
                publisher_directory(&staging)?;
                publisher_directory(&sessions)?;
                Ok((exclusive, staging, sessions))
            })();
            let Ok((exclusive, staging, sessions)) = local else {
                unavailable_health(
                    &store,
                    &snapshot,
                    backup,
                    Some(state),
                    Status::RecoveryRequired,
                    "publisher-storage-unavailable",
                );
                return Ok(None);
            };
            let cancel = Arc::new(AtomicBool::new(false));
            let engine = Engine {
                store: store.clone(),
                lease,
                binding,
                saved_config: config.to_owned(),
                checkpoint: path,
                staging,
                sessions,
                transport,
                policy: policy.clone(),
                cancel: cancel.clone(),
                state: state.clone(),
                search: Search::default(),
                members: None,
                checked: 0,
                observe_cursor: 0,
                stale: false,
                qualified_root: true,
                _publisher: exclusive,
            };
            return Worker::launch(store, policy, cancel, engine);
        }
    }
    unavailable_health(&store, &snapshot, backup, state.as_ref(), status, reason);
    Ok(None)
}

fn unavailable_health(
    store: &Store,
    snapshot: &mode::SyncModeSnapshot,
    backup: Uuid,
    state: Option<&Checkpoint>,
    status: Status,
    reason: &str,
) {
    let latest = state.and_then(|state| state.latest.as_ref()).map(|latest| {
        json!({
            "marker_sha256":latest.marker.sha256,"complete_media":latest.complete_media,
            "verified_at_unix_seconds":latest.verified_at_unix_seconds,
        })
    });
    let _ = files::atomic_json(
        &store.paths.data.join("sync/health.json"),
        &json!({
            "version":1,"status":status,"mode":"isolated-backup","reason":reason,
            "device":snapshot.device,"backup":backup,"latest_verified":latest,
        }),
    );
}

#[cfg(test)]
mod tests;

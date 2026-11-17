//! Linux one-shot confidential Capacities actor. One request and one sanitized frame.
use super::{
    lifecycle::{Enrollment, Exchange, GrantOwner, Phase, Status},
    oauth::{Budget, OAuth, Proof, Registration},
    ExportConnection,
};
use crate::{
    config::{
        basic::{checked_document, Custody, Ordered},
        inspection::strict_json,
        Config,
    },
    note_export::Failure,
    storage::{digest, files, valid_digest, ConfigPublication, Store, Uuid, MAX_RECORD},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::watch;

const MAX_BODY: usize = 32 * 1024;
const MAX_MILLIS: u64 = 30_000;
const BOOT_ID: &str = "/proc/sys/kernel/random/boot_id";

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Selector {
    Exchange,
    Status,
    Resume,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    tablet_id: String,
    connection_id: String,
    boot_id: Uuid,
    config_sha256: String,
    operation_id: Uuid,
    grant_id: Uuid,
}

impl Binding {
    fn valid(&self) -> bool {
        Uuid::parse_str(&self.tablet_id)
            .ok()
            .is_some_and(|id| !id.is_nil() && id.to_string() == self.tablet_id)
            && valid_digest(&self.connection_id)
            && valid_digest(&self.config_sha256)
            && !self.boot_id.is_nil()
            && !self.operation_id.is_nil()
            && !self.grant_id.is_nil()
            && self.operation_id != self.grant_id
    }

    fn enrollment(&self, client_id: String) -> Enrollment {
        Enrollment {
            operation_id: self.operation_id,
            tablet_id: self.tablet_id.clone(),
            connection_id: self.connection_id.clone(),
            boot_id: self.boot_id,
            config_sha256: self.config_sha256.clone(),
            client_id,
        }
    }

    fn matches_enrollment(&self, value: &Status) -> bool {
        value.grant_id == self.grant_id
            && value
                .enrollment
                .as_ref()
                .is_some_and(|original| *original == self.enrollment(original.client_id.clone()))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    contract_version: u32,
    selector: Selector,
    budget_ms: u64,
    binding: Binding,
    proof: Option<Proof>,
}

impl Request {
    fn parse(frame: &[u8]) -> Option<Self> {
        if frame.len() > MAX_BODY {
            return None;
        }
        let raw = strict_json(frame).ok()?;
        if !raw.is_object()
            || !raw.get("binding")?.is_object()
            || raw.get("proof").is_some_and(|proof| !proof.is_object())
        {
            return None;
        }
        let binding = raw.get("binding")?;
        for field in ["boot_id", "operation_id", "grant_id"] {
            let value = binding.get(field)?.as_str()?;
            if value != Uuid::parse_str(value).ok()?.to_string() {
                return None;
            }
        }
        let request: Self = serde_json::from_value(raw).ok()?;
        if request.contract_version == 1
            && (1..=MAX_MILLIS).contains(&request.budget_ms)
            && request.binding.valid()
            && (request.selector == Selector::Exchange) == request.proof.is_some()
        {
            Some(request)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Outcome {
    NotDispatched,
    Staged,
    Unresolved,
    Committed,
}

#[derive(Serialize)]
struct Report {
    contract_version: u32,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    binding: Option<Binding>,
    status: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    installed_config_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    space_id: Option<Uuid>,
    #[serde(skip)]
    _permit: Option<Exchange>,
    #[serde(skip)]
    _owner: Option<GrantOwner>,
    #[serde(skip)]
    _store: Option<Custody>,
}

impl Report {
    fn new(binding: Option<Binding>) -> Self {
        Self {
            contract_version: 1,
            binding,
            status: Outcome::NotDispatched,
            installed_config_sha256: None,
            space_id: None,
            _permit: None,
            _owner: None,
            _store: None,
        }
    }

    fn set(&mut self, outcome: Settlement) {
        self.status = match outcome {
            Settlement::Staged => Outcome::Staged,
            Settlement::Unresolved => Outcome::Unresolved,
            Settlement::Committed {
                space,
                config_sha256,
            } => {
                self.space_id = Some(space);
                self.installed_config_sha256 = Some(config_sha256);
                Outcome::Committed
            }
        };
    }

    fn write_to(self, output: &mut impl Write) -> anyhow::Result<()> {
        let mut frame = serde_json::to_vec(&self).map_err(|_| anyhow::anyhow!("output-failure"))?;
        frame.push(b'\n');
        output
            .write_all(&frame)
            .and_then(|()| output.flush())
            .map_err(|_| anyhow::anyhow!("output-failure"))
    }
}

struct Document {
    path: PathBuf,
    config: Config,
    raw: Value,
    sha256: String,
}

impl Document {
    fn load(path: Option<PathBuf>) -> Option<Self> {
        let path = path.filter(|value| value.is_absolute())?;
        let bytes = files::read(&path, MAX_RECORD as u64).ok()?;
        let (config, raw) = checked_document(&bytes).ok()?;
        config.validate().ok()?;
        Some(Self {
            path,
            config,
            raw,
            sha256: digest(&bytes),
        })
    }

    fn target(&self, grant: Uuid, space: Uuid) -> Option<Vec<u8>> {
        let mut raw = self.raw.clone();
        let connection = ExportConnection {
            schema_version: 1,
            space_id: space,
            grant_id: grant,
            tags: vec![],
        };
        raw.as_object_mut()?.insert(
            "capacities_export".to_owned(),
            serde_json::to_value(connection).ok()?,
        );
        let encoded = serde_json::to_vec(&Ordered(&raw)).ok()?;
        if encoded.len() > MAX_RECORD || checked_document(&encoded).is_err() {
            return None;
        }
        Some(encoded)
    }
}

fn config_matches(path: &Path, expected: &str) -> bool {
    files::read(path, MAX_RECORD as u64)
        .ok()
        .is_some_and(|bytes| digest(&bytes) == expected)
}

fn durable_config_matches(path: &Path, expected: &str) -> bool {
    config_matches(path, expected)
        && path
            .parent()
            .is_some_and(|parent| files::sync_dir(parent).is_ok())
        && config_matches(path, expected)
}

enum Settlement {
    Staged,
    Unresolved,
    Committed { space: Uuid, config_sha256: String },
}

fn settle(
    owner: &GrantOwner,
    store: &Store,
    doc: &Document,
    binding: &Binding,
    status: Status,
    publication_client: Option<&str>,
    budget: &Budget,
) -> Settlement {
    if !binding.matches_enrollment(&status) {
        return Settlement::Unresolved;
    }
    let (Some(space), Some(target)) = (status.space_id, status.target_config_sha256) else {
        return Settlement::Unresolved;
    };
    if budget.current().is_err() {
        return Settlement::Unresolved;
    }
    match status.phase {
        Phase::Active if durable_config_matches(&doc.path, &target) && budget.current().is_ok() => {
            Settlement::Committed {
                space,
                config_sha256: target,
            }
        }
        Phase::Staged => {
            if !config_matches(&doc.path, &target)
                && !config_matches(&doc.path, &binding.config_sha256)
            {
                return Settlement::Unresolved;
            }
            if !status
                .enrollment
                .as_ref()
                .is_some_and(|enrollment| publication_client == Some(enrollment.client_id.as_str()))
            {
                return Settlement::Staged;
            }
            if !durable_config_matches(&doc.path, &target) {
                if !config_matches(&doc.path, &binding.config_sha256)
                    || doc.sha256 != binding.config_sha256
                    || budget.current().is_err()
                {
                    return Settlement::Unresolved;
                }
                let Some(bytes) = doc.target(binding.grant_id, space) else {
                    return Settlement::Unresolved;
                };
                if digest(&bytes) != target {
                    return Settlement::Unresolved;
                }
                if owner.check_staged(binding.operation_id, &target).is_err() {
                    return Settlement::Unresolved;
                }
                if budget.current().is_err() {
                    return Settlement::Staged;
                }
                match store.replace_config_document(&doc.path, &binding.config_sha256, &bytes) {
                    ConfigPublication::Verified(hash) if hash == target => {}
                    ConfigPublication::Unknown => return Settlement::Staged,
                    _ => return Settlement::Unresolved,
                }
                if !durable_config_matches(&doc.path, &target) {
                    return Settlement::Staged;
                }
            }
            if budget.current().is_err()
                || owner
                    .activate_current(binding.operation_id, &target, || {
                        if !config_matches(&doc.path, &target) {
                            return Err(Failure::Conflict);
                        }
                        budget.current().map(|_| ())
                    })
                    .is_err()
            {
                return Settlement::Staged;
            }
            if !durable_config_matches(&doc.path, &target) || budget.current().is_err() {
                return Settlement::Unresolved;
            }
            Settlement::Committed {
                space,
                config_sha256: target,
            }
        }
        _ => Settlement::Unresolved,
    }
}

async fn handle(
    path: Option<PathBuf>,
    frame: &[u8],
    started: Instant,
    canceled: watch::Receiver<bool>,
    registration: Option<&Registration>,
    boot: Option<Uuid>,
) -> Report {
    let Some(request) = Request::parse(frame) else {
        return Report::new(None);
    };
    let binding = &request.binding;
    let mut report = Report::new(Some(binding.clone()));
    let authority = match request.selector {
        Selector::Exchange => {
            let (Some(registration), Some(proof)) = (registration, &request.proof) else {
                return report;
            };
            if !proof.valid(registration) {
                return report;
            }
            Some((registration, proof))
        }
        Selector::Status | Selector::Resume => None,
    };
    let budget = Budget::new(started + Duration::from_millis(request.budget_ms), canceled);
    if boot != Some(binding.boot_id) || budget.current().is_err() {
        report.status = Outcome::Unresolved;
        return report;
    }
    let Some(doc) = Document::load(path) else {
        if authority.is_none() {
            report.status = Outcome::Unresolved;
        }
        return report;
    };
    let Ok(store) = Store::open_existing_for_export_reconciliation(doc.config.paths.clone()) else {
        report.status = Outcome::Unresolved;
        return report;
    };
    let store = Arc::new(store);
    if store.actor_id.to_string() != binding.tablet_id {
        report.status = Outcome::Unresolved;
        return report;
    }
    let Ok(custody) = Custody::acquire(store.clone()) else {
        report.status = Outcome::Unresolved;
        return report;
    };
    report._store = Some(custody);
    let root = &doc.config.paths.credentials;

    if let Some((registration, proof)) = authority {
        if doc.sha256 != binding.config_sha256
            || doc
                .config
                .capacities_export
                .as_ref()
                .is_some_and(|value| value.grant_id == binding.grant_id)
            || budget.current().is_err()
        {
            report.status = Outcome::Unresolved;
            return report;
        }
        let Ok(owner) = GrantOwner::open(root, binding.grant_id) else {
            report.status = Outcome::Unresolved;
            return report;
        };
        if !owner
            .status()
            .ok()
            .is_some_and(|value| value.phase == Phase::Absent)
        {
            report.status = Outcome::Unresolved;
            report._owner = Some(owner);
            return report;
        }
        let Ok(oauth) = OAuth::new(registration) else {
            report._owner = Some(owner);
            return report;
        };
        if oauth.verify_metadata(&budget).await.is_err() {
            report._owner = Some(owner);
            return report;
        }
        if !config_matches(&doc.path, &binding.config_sha256) || budget.current().is_err() {
            report.status = Outcome::Unresolved;
            report._owner = Some(owner);
            return report;
        }
        let mut permit = match owner
            .begin_exchange_retained(binding.enrollment(registration.client_id.clone()))
        {
            Ok(permit) => permit,
            Err((owner, _)) => {
                report.status = Outcome::Unresolved;
                report._owner = Some(*owner);
                return report;
            }
        };
        let outcome = match oauth
            .redeem(proof, &mut permit, binding.grant_id, &budget, || {
                if config_matches(&doc.path, &binding.config_sha256) {
                    Ok(())
                } else {
                    Err(Failure::Conflict)
                }
            })
            .await
        {
            Ok(grant) => {
                let space = grant.space_id;
                if budget.current().is_err() {
                    Settlement::Unresolved
                } else if let (Some(target), Ok(bytes)) = (
                    doc.target(binding.grant_id, space),
                    serde_json::to_vec(&grant),
                ) {
                    let target_digest = digest(&target);
                    if budget.current().is_err() {
                        Settlement::Unresolved
                    } else {
                        match permit.stage_in_place(space, &bytes, target_digest) {
                            Ok(()) => match permit.owner().status() {
                                Ok(status) => settle(
                                    permit.owner(),
                                    &store,
                                    &doc,
                                    binding,
                                    status,
                                    Some(&registration.client_id),
                                    &budget,
                                ),
                                Err(_) => Settlement::Unresolved,
                            },
                            Err(_) => Settlement::Unresolved,
                        }
                    }
                } else {
                    Settlement::Unresolved
                }
            }
            Err(_) => Settlement::Unresolved,
        };
        report.set(outcome);
        report._permit = Some(permit);
        return report;
    }

    match GrantOwner::open_existing(root, binding.grant_id) {
        Ok(Some(owner)) => {
            match owner.status() {
                Ok(status)
                    if status.phase == Phase::Absent && doc.sha256 == binding.config_sha256 => {}
                Ok(status) => report.set(settle(
                    &owner,
                    &store,
                    &doc,
                    binding,
                    status,
                    registration
                        .filter(|_| request.selector == Selector::Resume)
                        .map(|registration| registration.client_id.as_str()),
                    &budget,
                )),
                Err(_) => report.status = Outcome::Unresolved,
            }
            report._owner = Some(owner);
        }
        Ok(None) if doc.sha256 == binding.config_sha256 => {}
        _ => report.status = Outcome::Unresolved,
    }
    report
}

fn local_boot() -> Option<Uuid> {
    let mut line = String::new();
    File::open(BOOT_ID)
        .ok()?
        .take(65)
        .read_to_string(&mut line)
        .ok()?;
    if line.len() != 37 || !line.ends_with('\n') {
        return None;
    }
    Uuid::parse_str(line.strip_suffix('\n')?).ok()
}

fn read_frame(deadline: Instant) -> std::result::Result<Vec<u8>, Failure> {
    let mut frame = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(Failure::Deadline)?;
        let timeout = (remaining.as_millis() + 1).min(u128::from(i32::MAX as u32)) as i32;
        let mut descriptor = nix::libc::pollfd {
            fd: nix::libc::STDIN_FILENO,
            events: nix::libc::POLLIN,
            revents: 0,
        };
        // One initialized pollfd and a bounded timeout. Only this command
        // reads its inherited stdin, without a buffered or blocking thread.
        let ready = unsafe { nix::libc::poll(&mut descriptor, 1, timeout) };
        if ready < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if ready == 0 {
            return Err(Failure::Deadline);
        }
        if ready < 0 || (descriptor.revents & (nix::libc::POLLIN | nix::libc::POLLHUP)) == 0 {
            return Err(Failure::InvalidInput);
        }
        // The initialized chunk is writable for its declared length.
        let read =
            unsafe { nix::libc::read(descriptor.fd, chunk.as_mut_ptr().cast(), chunk.len()) };
        if read < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if read < 0 {
            return Err(Failure::InvalidInput);
        }
        let read = read as usize;
        if read == 0 {
            return Ok(frame);
        }
        if read > MAX_BODY - frame.len() {
            return Err(Failure::InvalidInput);
        }
        frame.extend_from_slice(&chunk[..read]);
    }
}

pub fn run_cli(path: Option<PathBuf>, output: &mut impl Write) -> anyhow::Result<()> {
    let started = Instant::now();
    let Ok(frame) = read_frame(started + Duration::from_millis(MAX_MILLIS)) else {
        return Report::new(None).write_to(output);
    };
    // Runtime::block_on polls the actor on the calling thread. The worker keeps
    // cancellation and the IO driver running during synchronous disk custody.
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
    else {
        return Report::new(None).write_to(output);
    };
    let registration = Registration::production();
    let report = runtime.block_on(async {
        use tokio::signal::unix::{signal, SignalKind};
        let Ok(mut interrupt) = signal(SignalKind::interrupt()) else {
            return Report::new(None);
        };
        let (sender, canceled) = watch::channel(false);
        let watcher = tokio::spawn(async move {
            let _ = interrupt.recv().await;
            let _ = sender.send(true);
        });
        let report = handle(
            path,
            &frame,
            started,
            canceled,
            registration.as_ref(),
            local_boot(),
        )
        .await;
        watcher.abort();
        report
    });
    report.write_to(output)
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;

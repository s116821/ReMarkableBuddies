//! One-shot selected preference admission and original-only receipt observation.
use super::*;
use crate::config::{
    inspection,
    portable::{
        record::{self, Condition, SelectionCondition},
        selected::{Artifact, LocalConfigSnapshot},
    },
};
use crate::storage::selection::{
    SelectedReadBudget, SelectedSnapshot, SelectionChange, SelectionMutation,
};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const MAX_INPUT: usize = 262_144;
pub const MAX_OUTPUT: usize = 131_072;
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    Inspect,
    Edit,
    Status,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inspect {
    shared_preferences_version: u32,
    reveal_text: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    shared_preferences_version: u32,
    operation: Uuid,
    guard: Condition,
    patch: serde_json::Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    shared_preferences_version: u32,
    operation: Uuid,
    guard: Condition,
    patch_sha256: String,
}
enum Request {
    Inspect(bool),
    Edit(Uuid, Condition, Artifact),
    Status(Query),
}
impl Request {
    fn decode(operation: Operation, bytes: &[u8]) -> Result<Self> {
        let value = inspection::strict_json(bytes)
            .map_err(|_| anyhow::anyhow!("invalid preference input"))?;
        Ok(match operation {
            Operation::Inspect => {
                let v: Inspect = serde_json::from_value(value)?;
                ensure!(
                    v.shared_preferences_version == 1,
                    "unsupported preference input"
                );
                Self::Inspect(v.reveal_text)
            }
            Operation::Edit => {
                let v: Edit = serde_json::from_value(value)?;
                ensure!(
                    v.shared_preferences_version == 1 && !v.operation.is_nil(),
                    "invalid preference identity"
                );
                v.guard.validate()?;
                let patch = Artifact::from_value(v.patch)
                    .map_err(|_| anyhow::anyhow!("invalid preference patch"))?;
                Self::Edit(v.operation, v.guard, patch)
            }
            Operation::Status => {
                let v: Query = serde_json::from_value(value)?;
                ensure!(
                    v.shared_preferences_version == 1
                        && !v.operation.is_nil()
                        && valid_digest(&v.patch_sha256),
                    "invalid preference identity"
                );
                v.guard.validate()?;
                Self::Status(v)
            }
        })
    }
}

// No Debug: only an explicit private Inspect may contain personalization text.
#[derive(Serialize)]
pub struct Report {
    report_version: u32,
    operation: Operation,
    status: &'static str,
    disposition: &'static str,
    runtime_activation: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    guard: Option<Condition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    original_operation: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    patch_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    view: Option<serde_json::Value>,
    #[serde(skip)]
    store: Option<Arc<Store>>,
    #[serde(skip)]
    mode: Option<mode::SyncModeLease>,
    #[serde(skip)]
    config_pin: Option<(PathBuf, String)>,
}
impl Report {
    fn new(operation: Operation, status: &'static str, disposition: &'static str) -> Self {
        Self {
            report_version: 1,
            operation,
            status,
            disposition,
            runtime_activation: "unverified",
            guard: None,
            original_operation: None,
            patch_sha256: None,
            view: None,
            store: None,
            mode: None,
            config_pin: None,
        }
    }
    fn correspondence(&mut self, request: &Request) {
        match request {
            Request::Edit(op, guard, patch) => {
                self.original_operation = Some(*op);
                self.guard = Some(guard.clone());
                self.patch_sha256 = Some(patch.sha256());
            }
            Request::Status(q) => {
                self.original_operation = Some(q.operation);
                self.guard = Some(q.guard.clone());
                self.patch_sha256 = Some(q.patch_sha256.clone());
            }
            _ => {}
        }
    }
    pub fn write_to(mut self, output: &mut impl Write) -> Result<()> {
        if let Some((path, hash)) = &self.config_pin {
            if files::read(path, MAX_RECORD as u64)
                .ok()
                .is_none_or(|bytes| digest(&bytes) != *hash)
            {
                self.status = "unavailable";
                self.disposition = if self.operation == Operation::Edit {
                    "unknown"
                } else {
                    "not-admitted"
                };
                self.view = None;
                if self.operation == Operation::Inspect {
                    self.guard = None;
                }
            }
        }
        let mut bytes =
            serde_json::to_vec(&self).map_err(|_| anyhow::anyhow!("preference output failed"))?;
        bytes.push(b'\n');
        ensure!(bytes.len() <= MAX_OUTPUT, "preference output bound");
        output
            .write_all(&bytes)
            .map_err(|_| anyhow::anyhow!("preference output failed"))?;
        output
            .flush()
            .map_err(|_| anyhow::anyhow!("preference output failed"))
    }
}
pub(super) struct Capture {
    pub(super) path: PathBuf,
    pub(super) sha: String,
    pub(super) local: LocalConfigSnapshot,
}
impl Capture {
    pub(super) fn read(path: PathBuf, reads: &mut SelectedReadBudget) -> Result<Self> {
        ensure!(path.is_absolute(), "preference config path unavailable");
        let bytes = files::read_with_budget(&path, MAX_RECORD as u64, Some(reads))?;
        let local = LocalConfigSnapshot::parse(&bytes)
            .map_err(|_| anyhow::anyhow!("preference config unavailable"))?;
        local.base_config().validate()?;
        Ok(Self {
            path,
            sha: digest(&bytes),
            local,
        })
    }
    pub(super) fn check(&self, reads: &mut SelectedReadBudget) -> Result<()> {
        ensure!(
            digest(&files::read_with_budget(
                &self.path,
                MAX_RECORD as u64,
                Some(reads)
            )?) == self.sha,
            "preference config changed"
        );
        Ok(())
    }
}
pub(super) fn budget() -> SelectedReadBudget {
    // Eight worst-case agreed-slot observations, with one shared allowance for
    // Config/checkpoint/selected original reads. Exhaustion never proves absence.
    SelectedReadBudget::new(
        64,
        MAX_ITEMS as u64,
        8 * (3 * MAX_METADATA as u64 + 64 * 1024),
        0,
    )
}
fn scope(guard: &Condition) -> SelectionScope {
    SelectionScope {
        group: guard.group,
        key_sha256: record::key_sha256(),
        binding_sha256: guard.binding_sha256.clone(),
    }
}
fn selection(snapshot: &SelectedSnapshot, projection: &record::Projection) -> SelectionCondition {
    SelectionCondition::Selected {
        aggregate_generation: snapshot.token.aggregate_generation(),
        accepted_base_sha256: snapshot.token.accepted_base_sha256().into(),
        selection_sha256: snapshot.token.selection_sha256().into(),
        revision: projection.revision(),
    }
}

/// No mode/provider/config re-admission: this observes one exact local original.
fn original_status(
    store: &Arc<Store>,
    query: &Query,
    reads: &mut SelectedReadBudget,
) -> Result<bool> {
    let scope = scope(&query.guard);
    let Some(read) = store.selected_snapshot_with_budget(&scope, MAX_ITEMS, reads)? else {
        return Ok(false);
    };
    let Some(position) = read
        .accepted_history
        .iter()
        .position(|t| t.transaction.operation == query.operation)
    else {
        return Ok(false);
    };
    let original = &read.accepted_history[position].transaction;
    ensure!(
        original.store_generation == query.guard.store_generation,
        "preference original generation differs"
    );
    match (&query.guard.selection, original.mutation) {
        (SelectionCondition::Absent {}, SelectionMutation::Initialize) => {
            ensure!(
                original.previous_sha256.is_none(),
                "preference original predecessor differs"
            );
        }
        (
            SelectionCondition::Selected {
                aggregate_generation,
                accepted_base_sha256,
                selection_sha256,
                revision,
            },
            SelectionMutation::Commit,
        ) => {
            let prior = read
                .accepted_history
                .get(position + 1)
                .context("preference original predecessor absent")?;
            let projection =
                preferences::project(store, &scope, &prior.transaction.selected, reads)?;
            ensure!(
                prior.sha256 == *selection_sha256
                    && prior.transaction.aggregate_generation == *aggregate_generation
                    && prior.transaction.accepted_base_sha256 == *accepted_base_sha256
                    && projection.revision() == *revision,
                "preference original predecessor differs"
            );
        }
        _ => anyhow::bail!("not an explicit preference original"),
    }
    let result = preferences::project(store, &scope, &original.selected, reads)?;
    record::verify_original(
        &result,
        store.actor_id,
        query.operation,
        &query.guard,
        &query.patch_sha256,
    )?;
    Ok(true)
}

pub(super) fn checkpoint(
    store: &Store,
    binding: &Binding,
    epoch: Uuid,
    reads: &mut SelectedReadBudget,
) -> Result<Option<Checkpoint>> {
    let path = store.paths.data.join("sync/state.json");
    if !path.exists() {
        return Ok(None);
    }
    let bytes = files::read_with_budget(&path, MAX_METADATA as u64, Some(reads))?;
    let state: State = serde_json::from_slice(&bytes)?;
    ensure!(
        state.format == FORMAT,
        "preference checkpoint format differs"
    );
    let Some(selected) = state.selected else {
        return Ok(None);
    };
    ensure!(selected.format == 1, "preference checkpoint format differs");
    control::validate_controls(&selected, store.actor_id)?;
    let Some(cp) = selected.epochs.get(&epoch).cloned() else {
        return Ok(None);
    };
    ensure!(
        cp.actor == store.actor_id
            && cp.epoch == epoch
            && cp.group == binding.group
            && cp.binding == binding.digest
            && cp.root_id == binding.root_id
            && cp.format == 1,
        "preference checkpoint binding differs"
    );
    catalog::validate_checkpoint(&cp, binding)?;
    Ok(Some(cp))
}

pub fn run(
    operation: Operation,
    input: &Path,
    config_path: Option<PathBuf>,
    stdin: &mut impl Read,
) -> Report {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut fail = Report::new(operation, "invalid-input", "not-dispatched");
    if input.as_os_str() != std::ffi::OsStr::new("-") {
        return fail;
    }
    let mut bytes = Vec::new();
    if stdin
        .take((MAX_INPUT + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > MAX_INPUT
    {
        return fail;
    }
    let request = match Request::decode(operation, &bytes) {
        Ok(v) => v,
        Err(_) => return fail,
    };
    fail.correspondence(&request);
    fail.status = "unavailable";
    let mut reads = budget();
    let Some(path) = config_path else { return fail };
    let capture = match Capture::read(path, &mut reads) {
        Ok(v) => v,
        Err(_) => return fail,
    };
    let store = match Store::open_existing_for_mode(capture.local.base_config().paths.clone()) {
        Ok(v) => Arc::new(v),
        Err(refusal) => {
            fail.status = match refusal {
                crate::storage::maintenance::Refusal::Busy => "busy",
                crate::storage::maintenance::Refusal::Unsupported => "unsupported",
                _ => "unavailable",
            };
            return fail;
        }
    };
    fail.store = Some(store.clone());
    if let Request::Status(query) = &request {
        fail.disposition = "unknown";
        let admission = match SelectedAdmission::new(store.clone(), scope(&query.guard)) {
            Ok(v) => v,
            Err(_) => return fail,
        };
        if SelectedAdmission::with_gates(&[&admission], |_| {
            original_status(&store, query, &mut reads)
        })
        .ok()
            == Some(true)
        {
            fail.status = "ok";
            fail.disposition = "local-committed";
        }
        return fail;
    }
    let current = match store.sync_mode_snapshot() {
        Ok(v) => v,
        Err(_) => return fail,
    };
    let Some(chosen) = current.mode.as_ref() else {
        fail.status = "unsupported";
        return fail;
    };
    let binding = match Binding::qualified(chosen) {
        Ok(v) => v,
        Err(_) => {
            fail.status = "unsupported";
            return fail;
        }
    };
    let transport_path = store.paths.credentials.join("drive.json");
    match admitted(
        store,
        capture,
        request,
        binding,
        deadline,
        &mut reads,
        || GoogleDrive::open(transport_path),
    ) {
        Ok(report) => report,
        Err(_) => {
            fail.disposition = "unknown";
            fail
        }
    }
}

fn admitted(
    store: Arc<Store>,
    capture: Capture,
    request: Request,
    binding: Binding,
    deadline: Instant,
    reads: &mut SelectedReadBudget,
    transport: impl FnOnce() -> Result<GoogleDrive>,
) -> Result<Report> {
    let current = store.sync_mode_snapshot()?;
    require_mode(&current, &binding)?;
    let epoch = current.epoch.context("preference epoch absent")?;
    let lease = store.sync_mode_lease(&current.token)?;
    let scope = SelectionScope {
        group: binding.group,
        key_sha256: record::key_sha256(),
        binding_sha256: binding.digest.clone(),
    };
    lease.require_shared_scope(&scope)?;
    let admission = SelectedAdmission::new(store.clone(), scope.clone())?;
    let mut report = SelectedAdmission::with_gates(&[&admission], |_| {
        let expected = match &request {
            Request::Edit(_, expected, _) => Some(expected),
            _ => None,
        };
        let view = match read_current_locked(
            &store,
            &capture,
            ReadContext {
                binding,
                epoch,
                deadline,
                expected,
            },
            reads,
            transport,
        )? {
            Readiness::Current(view) => *view,
            Readiness::Joining => {
                return Ok(Report::new(request.operation(), "joining", "not-admitted"))
            }
            Readiness::Conflict => {
                return Ok(Report::new(
                    request.operation(),
                    "conflict",
                    "not-dispatched",
                ))
            }
        };
        let CurrentView {
            guard,
            read,
            prior,
            root_sha,
            ..
        } = view;
        let mut report = Report::new(request.operation(), "ok", "read-only");
        report.correspondence(&request);
        match &request {
            Request::Inspect(reveal) => {
                report.view = Some(
                    capture
                        .local
                        .view(prior.as_ref().map(|p| p.artifact()), *reveal)
                        .map_err(|_| anyhow::anyhow!("preference view unavailable"))?,
                );
                report.guard = Some(guard);
            }
            Request::Edit(operation, expected, patch) => {
                if *expected != guard {
                    report.status = "conflict";
                    report.disposition = "not-dispatched";
                    return Ok(report);
                }
                // Retries never mint a second edit. Status is the only recovery
                // path after a possible original dispatch.
                ensure!(
                    store.selected_receipt(&scope, *operation)?.is_none(),
                    "preference original already exists"
                );
                if let Some(envelope) =
                    record::prepare(store.actor_id, *operation, &guard, prior.as_ref(), patch)?
                {
                    if let Some(read) = &read {
                        capture.check(reads)?;
                        ensure!(Instant::now() < deadline, "preference request expired");
                        store.commit_selected(
                            &read.snapshot.token,
                            *operation,
                            vec![envelope],
                            BTreeMap::new(),
                        )?;
                    } else {
                        let bytes = serde_json::to_vec(&envelope)?;
                        let hash = digest(&bytes);
                        let manifest = Manifest {
                            format: 1,
                            transaction_id: *operation,
                            scope: Scope::SelectedRecords,
                            records: vec![ObjectRef {
                                sha256: hash.clone(),
                                bytes: bytes.len() as u64,
                            }],
                            record_namespaces: BTreeMap::from([(
                                hash.clone(),
                                Namespace::PortableConfiguration,
                            )]),
                            media: vec![],
                            media_coverage: vec![],
                        };
                        capture.check(reads)?;
                        ensure!(Instant::now() < deadline, "preference request expired");
                        store.initialize_selected(
                            &scope,
                            SelectionChange {
                                operation: *operation,
                                accepted_base_sha256: genesis_base(&root_sha, &scope.key_sha256),
                                selected: manifest,
                                retained: vec![],
                            },
                            BTreeMap::from([(hash, bytes)]),
                        )?;
                    }
                    report.disposition = "local-committed";
                } else {
                    report.disposition = "no-change";
                }
            }
            Request::Status(_) => anyhow::bail!("original status uses its historical path"),
        }
        Ok(report)
    })?;
    report.correspondence(&request);
    report.store = Some(store);
    report.mode = Some(lease);
    // A known non-dispatch/no-change remains known if Config later changes.
    // Editable inspection or a possibly committed effect needs a current output
    // pin; uncertainty never turns into an automatic replay or a second report.
    if report.guard.is_some() && report.operation == Operation::Inspect
        || report.disposition == "local-committed"
    {
        report.config_pin = Some((capture.path, capture.sha));
    }
    Ok(report)
}
impl Request {
    fn operation(&self) -> Operation {
        match self {
            Self::Inspect(_) => Operation::Inspect,
            Self::Edit(..) => Operation::Edit,
            Self::Status(_) => Operation::Status,
        }
    }
}

/// Private typed capture only. It must still be revalidated at every consuming
/// handoff/publication; serializable Condition values cannot construct it.
pub(super) struct CurrentView {
    pub(super) guard: Condition,
    pub(super) read: Option<crate::storage::selection::SelectedReadSnapshot>,
    pub(super) prior: Option<record::Projection>,
    pub(super) root_sha: String,
    pub(super) profile: Uuid,
    pub(super) absence: Option<(String, String)>,
}
pub(super) enum Readiness {
    Current(Box<CurrentView>),
    Joining,
    Conflict,
}
pub(super) struct ReadContext<'a> {
    pub(super) binding: Binding,
    pub(super) epoch: Uuid,
    pub(super) deadline: Instant,
    pub(super) expected: Option<&'a Condition>,
}
/// Caller retains actual current mode and the ordered preference gate. This is
/// shared by the command and request-scoped product consumers, not a second
/// readiness engine or a constructor from saved JSON.
pub(super) fn read_current_locked(
    store: &Arc<Store>,
    capture: &Capture,
    context: ReadContext<'_>,
    reads: &mut SelectedReadBudget,
    transport: impl FnOnce() -> Result<GoogleDrive>,
) -> Result<Readiness> {
    let ReadContext {
        binding,
        epoch,
        deadline,
        expected,
    } = context;
    let current = store.sync_mode_snapshot()?;
    require_mode(&current, &binding)?;
    ensure!(
        current.epoch == Some(epoch),
        "preference mode epoch changed"
    );
    let scope = SelectionScope {
        group: binding.group,
        key_sha256: record::key_sha256(),
        binding_sha256: binding.digest.clone(),
    };
    ensure!(Instant::now() < deadline, "preference request expired");
    let generation = catalog::current_generation(store)?;
    crate::storage::compound_generation::validate_ready(&store.paths, store.actor_id, generation)?;
    capture.check(reads)?;
    let read = store.selected_snapshot_with_budget(&scope, MAX_ITEMS, reads)?;
    if let Some(expected) = expected {
        if expected.group != scope.group
            || expected.mode_epoch != epoch
            || expected.binding_sha256 != scope.binding_sha256
            || expected.store_generation != generation
            || expected.local_config_sha256 != capture.sha
            || matches!(
                (&expected.selection, &read),
                (SelectionCondition::Selected { .. }, None)
            )
        {
            return Ok(Readiness::Conflict);
        }
    }
    let prior = read
        .as_ref()
        .map(|r| preferences::project(store, &scope, &r.snapshot.transaction.selected, reads))
        .transpose()?;
    let (root_sha, condition, absence, profile) = if let Some(read) = &read {
        // Established selection uses only immutable local custody. No
        // credential file, refresh, or provider GET is a prerequisite.
        let cp =
            checkpoint(store, &binding, epoch, reads)?.context("preference root custody absent")?;
        let view = catalog_view(&binding, &cp)?.context("preference root absent")?;
        ensure!(!cp.catalog.is_empty(), "preference bootstrap absent");
        if let Some(entry) = view.entries.get(&scope.key_sha256) {
            ensure!(
                entry.owner == Owner::preferences(),
                "preference owner differs"
            );
        } else {
            preferences::require_initial_publication(store, &scope, &view.root_sha, reads)?;
        }
        (
            view.root_sha,
            selection(
                &read.snapshot,
                prior.as_ref().context("preference projection absent")?,
            ),
            None,
            view.root.profile,
        )
    } else {
        if let Some(cp) = checkpoint(store, &binding, epoch, reads)? {
            let pending = cp
                .catalog_pending
                .as_ref()
                .map(CatalogPending::key)
                .transpose()?;
            let historical = cp.scopes.get(&scope.key_sha256).is_some_and(|p| {
                !p.chain.is_empty()
                    || p.pending.is_some()
                    || p.receive.is_some()
                    || p.applied.is_some()
                    || !p.receipts.is_empty()
                    || !p.retired_pending.is_empty()
            });
            if pending.as_ref() == Some(&scope.key_sha256)
                || historical
                || cp.retired_catalog.iter().any(|p| p.key == scope.key_sha256)
            {
                // Missing selected data does not erase an original attempt
                // or prove fresh absence after partial recovery.
                return Ok(Readiness::Joining);
            }
        }
        // Engine reloads its original bounded state, independently of the
        // local uncertainty inspection above. Reserve that second read in
        // this same request allowance before even opening credentials.
        reads.charge(0, 1, MAX_METADATA as u64, 0)?;
        let mut engine = Engine::new(
            store.clone(),
            capture.local.base_config().sync.clone(),
            transport()?,
            binding,
            Arc::new(AtomicBool::new(false)),
        )?;
        engine
            .transport
            .set_request_window(Some(deadline), Some(engine.cancel.clone()));
        let mut items = MAX_ITEMS;
        let refreshed = engine.refresh_root_catalog_bounded(&mut items, Some(reads));
        engine.transport.set_request_window(None, None);
        let Some(view) = refreshed? else {
            return Ok(Readiness::Joining);
        };
        if !view.complete
            || engine.checkpoint.catalog.is_empty()
            || view.entries.contains_key(&scope.key_sha256)
        {
            return Ok(Readiness::Joining);
        }
        (
            view.root_sha,
            SelectionCondition::Absent {},
            Some((view.base, view.slot)),
            view.root.profile,
        )
    };
    let guard = Condition {
        group: scope.group,
        mode_epoch: epoch,
        binding_sha256: scope.binding_sha256.clone(),
        store_generation: generation,
        local_config_sha256: capture.sha.clone(),
        selection: condition,
    };
    guard.validate()?;
    capture.check(reads)?;
    ensure!(
        Instant::now() < deadline && catalog::current_generation(store)? == generation,
        "preference request expired or generation changed"
    );

    Ok(Readiness::Current(Box::new(CurrentView {
        guard,
        read,
        prior,
        root_sha,
        profile,
        absence,
    })))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(super) fn read_fixture_model_request(
    stream: &mut std::net::TcpStream,
    bound: usize,
) -> serde_json::Value {
    use std::io::Read;

    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut chunk).unwrap();
        assert!(n > 0 && bytes.len() + n <= bound);
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(at) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let headers = std::str::from_utf8(&bytes[..header_end]).unwrap();
    assert!(headers.starts_with("POST /v1/chat/completions "));
    let length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
        })
        .unwrap();
    assert!(header_end + length <= bound);
    while bytes.len() - header_end < length {
        let n = stream.read(&mut chunk).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&chunk[..n]);
    }
    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap()
}

#[cfg(all(test, target_os = "linux"))]
pub(super) fn set_fixture_reader_preference(
    store: Arc<Store>,
    path: &std::path::Path,
    binding: Binding,
    wire: &crate::storage::drive::shared_fixture::Wire,
    enabled: bool,
) -> Result<()> {
    let run = |request| {
        let mut reads = budget();
        let capture = Capture::read(path.to_owned(), &mut reads)?;
        admitted(
            store.clone(),
            capture,
            request,
            binding.clone(),
            Instant::now() + Duration::from_secs(60),
            &mut reads,
            || Ok(wire.client(store.paths.credentials.join("drive.json"), binding.group)),
        )
    };
    let guard = run(Request::Inspect(false))?
        .guard
        .context("fixture preference guard absent")?;
    let artifact = Artifact::from_value(serde_json::json!({
        "portable_configuration_version": 1,
        "settings": { "product_settings": { "reader_enabled": enabled } }
    }))
    .map_err(|_| anyhow::anyhow!("invalid fixture preference artifact"))?;
    let report = run(Request::Edit(Uuid::new_v4(), guard, artifact))?;
    ensure!(
        report.disposition == "local-committed",
        "fixture preference edit refused"
    );
    Ok(())
}

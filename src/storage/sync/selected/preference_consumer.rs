//! Private request snapshots; saved conditions are correspondence, never permits.
use super::preference_command::{self as command, Capture, ReadContext, Readiness};
use super::*;
use crate::config::{portable::record, Config};
use crate::storage::selection::{SelectedReadBudget, SelectionToken};
use std::{path::PathBuf, sync::Mutex};

#[cfg(any(target_os = "linux", test))]
pub(super) mod reader;
#[cfg(any(target_os = "linux", test))]
pub(crate) use reader::{ReaderPhase, ReaderPreferences};

#[derive(Clone, Copy)]
pub(crate) enum NotebookPhase {
    Admission,
    Handoff,
    Publication,
}

#[derive(Debug)]
pub(crate) struct NotebookSelectionChanged;

impl std::fmt::Display for NotebookSelectionChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("selected notebook winner changed")
    }
}

impl std::error::Error for NotebookSelectionChanged {}

pub(crate) struct RequestPreferences {
    store: Arc<Store>,
    admission: SelectedAdmission,
    mode: mode::SyncModeLease,
    binding: Binding,
    capture: Capture,
    guard: record::Condition,
    catalog_root: String,
    profile: Uuid,
    selected: Option<SelectionToken>,
    absence: Option<(String, String, String)>,
    effective: Config,
    admission_deadline: Instant,
    deadline: Instant,
    reads: RequestReads,
    handoff_used: AtomicBool,
    submitted: AtomicBool,
}

// One cumulative request allowance, irrevocably partitioned once. Progress
// cannot spend the full-history verification reserved for handoff/completion.
struct RequestReads {
    admission: Mutex<SelectedReadBudget>,
    handoff: Mutex<SelectedReadBudget>,
    progress: Mutex<SelectedReadBudget>,
    finish: Mutex<SelectedReadBudget>,
    _reservations: SelectedReadBudget,
}
impl RequestReads {
    fn new() -> Result<Self> {
        let lifetime =
            Duration::from_secs(60) + crate::llm::openai::DEFAULT_TIMEOUT + Duration::from_secs(60);
        Self::for_lifetime(lifetime, 1, 0, false)
    }

    fn for_lifetime(
        lifetime: Duration,
        phases: u64,
        media: u64,
        track_document: bool,
    ) -> Result<Self> {
        let phase_bytes = 8 * (3 * MAX_METADATA as u64 + 64 * 1024) * phases;
        let aggregates = 64 * phases;
        let objects = MAX_ITEMS as u64 * phases;
        let cadence = crate::workflow::indicator::CADENCE;
        // Entire original operation lifetime, including scheduling/completion
        // overhead, plus the guarded adapter boundary checks.
        let pulses = u64::try_from(lifetime.as_nanos().div_ceil(cadence.as_nanos()))? + 4 * phases;
        let selection_headers = 1 + u64::from(track_document);
        let progress_items = pulses * (1 + selection_headers);
        let progress_bytes = pulses * (MAX_RECORD as u64 + selection_headers * MAX_METADATA as u64);
        let mut reservations = SelectedReadBudget::new(
            aggregates * 3,
            objects * 3 + progress_items,
            phase_bytes * 3 + progress_bytes,
            media * 3,
        );
        let admission = reservations.reserve(aggregates, objects, phase_bytes, media)?;
        let handoff = reservations.reserve(aggregates, objects, phase_bytes, media)?;
        let finish = reservations.reserve(aggregates, objects, phase_bytes, media)?;
        let progress = reservations.reserve(0, progress_items, progress_bytes, 0)?;
        Ok(Self {
            admission: Mutex::new(admission),
            handoff: Mutex::new(handoff),
            progress: Mutex::new(progress),
            finish: Mutex::new(finish),
            _reservations: reservations,
        })
    }
}

struct DetachedPreferences {
    binding: Binding,
    capture: Capture,
    guard: record::Condition,
    catalog_root: String,
    profile: Uuid,
    selected: Option<SelectionToken>,
    absence: Option<(String, String, String)>,
    effective: Config,
    admission_deadline: Instant,
    deadline: Instant,
    reads: RequestReads,
    handoff_used: AtomicBool,
    submitted: AtomicBool,
    mode_token: mode::SyncModeToken,
    scope: SelectionScope,
}
impl DetachedPreferences {
    fn verify_locked(&self, store: &Store, reads: &mut SelectedReadBudget) -> Result<()> {
        ensure!(Instant::now() < self.deadline, "preference request expired");
        let current = store.sync_mode_snapshot()?;
        require_mode(&current, &self.binding)?;
        ensure!(
            current.token == self.mode_token
                && current.epoch == Some(self.guard.mode_epoch)
                && catalog::current_generation(store)? == self.guard.store_generation,
            "preference mode or generation changed"
        );
        self.capture.check(reads)?;
        let current = store.selected_snapshot_with_budget(&self.scope, MAX_ITEMS, reads)?;
        match (&self.selected, current) {
            (Some(expected), Some(read)) => ensure!(
                read.snapshot.token == *expected,
                "preference selection changed"
            ),
            (None, None) => {
                let (root, base, slot) = self
                    .absence
                    .as_ref()
                    .context("qualified preference absence absent")?;
                let cp = command::checkpoint(store, &self.binding, self.guard.mode_epoch, reads)?
                    .context("preference root custody absent")?;
                let view = catalog_view(&self.binding, &cp)?.context("preference root absent")?;
                ensure!(
                    !cp.catalog.is_empty()
                        && view.root_sha == *root
                        && view.base == *base
                        && view.slot == *slot
                        && !view.entries.contains_key(&record::key_sha256())
                        && cp
                            .catalog_pending
                            .as_ref()
                            .map(CatalogPending::key)
                            .transpose()?
                            .as_deref()
                            != Some(record::key_sha256().as_str())
                        && !cp.scopes.contains_key(&record::key_sha256())
                        && !cp
                            .retired_catalog
                            .iter()
                            .any(|p| p.key == record::key_sha256()),
                    "qualified preference absence changed"
                );
            }
            _ => anyhow::bail!("preference selection or absence changed"),
        }
        Ok(())
    }
}
impl RequestPreferences {
    #[cfg(test)]
    pub(super) fn exhaust_phase_reads_for_test(&self, phase: NotebookPhase) {
        let budget = match phase {
            NotebookPhase::Admission => &self.reads.admission,
            NotebookPhase::Handoff => &self.reads.handoff,
            NotebookPhase::Publication => &self.reads.finish,
        };
        assert!(budget.lock().unwrap().charge(u64::MAX, 0, 0, 0).is_err());
    }
    pub(crate) fn open_writer_image(store: Arc<Store>, path: PathBuf) -> Result<Self> {
        ensure!(
            !store.ordinary_reader_family()?,
            "selected Writer image requires shared Store"
        );
        let current = store.sync_mode_snapshot()?;
        let binding = Binding::qualified(
            current
                .mode
                .as_ref()
                .context("Writer image shared mode absent")?,
        )?;
        Self::writer_image_from_binding(store, path, binding, |store| {
            GoogleDrive::open(store.paths.credentials.join("drive.json"))
        })
    }

    pub(in crate::storage::sync::selected) fn writer_image_from_binding(
        store: Arc<Store>,
        path: PathBuf,
        binding: Binding,
        transport: impl FnOnce(&Store) -> Result<GoogleDrive>,
    ) -> Result<Self> {
        let started = Instant::now();
        let lifetime = Duration::from_secs(300);
        Self::from_binding_at(
            store,
            path,
            binding,
            transport,
            (started + Duration::from_secs(60), started + lifetime),
            RequestReads::for_lifetime(lifetime, 16, MAX_MEDIA * 16, true)?,
        )
    }

    pub(crate) fn writer_image_deadline(&self) -> Instant {
        self.deadline
    }

    pub(crate) fn capture_writer_handwriting(
        &self,
        writer: &SelectedAdmission,
        token: &SelectionToken,
        target: &str,
    ) -> Result<Option<crate::handwriting::ConfirmedContext>> {
        self.capture_writer_targets(writer, token, &BTreeSet::from([target.to_owned()]))
    }

    pub(crate) fn regional_cutoff(&self, offset: u64) -> Result<Instant> {
        ensure!(
            self.deadline.duration_since(self.admission_deadline) == Duration::from_secs(240),
            "regional Writer requires original image policy"
        );
        Ok(self.admission_deadline - Duration::from_secs(60) + Duration::from_secs(offset))
    }

    pub(crate) fn capture_writer_targets(
        &self,
        writer: &SelectedAdmission,
        token: &SelectionToken,
        targets: &BTreeSet<String>,
    ) -> Result<Option<crate::handwriting::ConfirmedContext>> {
        self.require_writer(writer)?;
        let config = &self.effective.handwriting;
        if !config.enabled {
            return Ok(None);
        }
        ensure!(
            Instant::now() < self.admission_deadline,
            "Writer image admission expired"
        );
        let profile = config
            .profile
            .context("Writer handwriting profile absent")?;
        let mut reads = self
            .reads
            .admission
            .lock()
            .map_err(|_| anyhow::anyhow!("Writer handwriting allowance unavailable"))?;
        let captured = SelectedContextOwner::capture_profile(
            &self.store,
            &self.binding,
            profile,
            targets,
            &[writer, &self.admission],
            &mut reads,
            |store, reads| {
                self.verify_locked(store, reads)?;
                ensure!(
                    store.selection_unchanged_for_progress(writer.scope(), Some(token), reads)?,
                    "Writer image selection changed"
                );
                Ok(())
            },
        )?;
        ensure!(
            captured.selection.status() != "partial-empty",
            "selected handwriting context incomplete"
        );
        Ok(Some(captured.context))
    }

    /// Ordinary families retain their delivered local preference path. Only the
    /// actual qualified format5 source can construct a shared request snapshot.
    pub(crate) fn open(store: Arc<Store>, path: PathBuf) -> Result<Option<Self>> {
        if store.ordinary_reader_family()? {
            return Ok(None);
        }
        let current = store.sync_mode_snapshot()?;
        let chosen = current
            .mode
            .as_ref()
            .context("shared preference mode absent")?;
        let binding = Binding::qualified(chosen)?;
        Self::from_binding(store, path, binding, |store| {
            GoogleDrive::open(store.paths.credentials.join("drive.json"))
        })
        .map(Some)
    }
    pub(super) fn from_binding(
        store: Arc<Store>,
        path: PathBuf,
        binding: Binding,
        transport: impl FnOnce(&Store) -> Result<GoogleDrive>,
    ) -> Result<Self> {
        let started = Instant::now();
        let admission_deadline = started + Duration::from_secs(60);
        // Reserve the existing bounded admission, supported model wait and a
        // bounded completion window from the original request start. No reset.
        let deadline =
            admission_deadline + crate::llm::openai::DEFAULT_TIMEOUT + Duration::from_secs(60);
        let budgets = RequestReads::new()?;
        Self::from_binding_at(
            store,
            path,
            binding,
            transport,
            (admission_deadline, deadline),
            budgets,
        )
    }

    fn from_binding_at(
        store: Arc<Store>,
        path: PathBuf,
        binding: Binding,
        transport: impl FnOnce(&Store) -> Result<GoogleDrive>,
        (admission_deadline, deadline): (Instant, Instant),
        budgets: RequestReads,
    ) -> Result<Self> {
        let mut reads = budgets
            .admission
            .lock()
            .map_err(|_| anyhow::anyhow!("preference admission allowance unavailable"))?;
        let capture = Capture::read(path, &mut reads)?;
        let paths = &capture.local.base_config().paths;
        ensure!(
            paths.data == store.paths.data
                && paths.cache == store.paths.cache
                && paths.credentials == store.paths.credentials,
            "preference Config Store differs"
        );
        let current = store.sync_mode_snapshot()?;
        require_mode(&current, &binding)?;
        let epoch = current.epoch.context("shared preference epoch absent")?;
        let mode = store.sync_mode_lease(&current.token)?;
        let scope = SelectionScope {
            group: binding.group,
            key_sha256: record::key_sha256(),
            binding_sha256: binding.digest.clone(),
        };
        mode.require_shared_scope(&scope)?;
        let admission = SelectedAdmission::new(store.clone(), scope)?;
        let view =
            SelectedAdmission::with_gates(&[&admission], |_| match command::read_current_locked(
                &store,
                &capture,
                ReadContext {
                    binding: binding.clone(),
                    epoch,
                    deadline: admission_deadline,
                    expected: None,
                },
                &mut reads,
                || transport(&store),
            )? {
                Readiness::Current(view) => Ok(*view),
                _ => anyhow::bail!("shared preferences joining or unresolved"),
            })?;
        let effective = capture
            .local
            .effective(view.prior.as_ref().map(|p| p.artifact()))
            .map_err(|_| anyhow::anyhow!("effective preferences unavailable"))?;
        if effective.memory.enabled {
            let selected = effective
                .memory
                .selection
                .as_ref()
                .context("shared preferences require selected memory")?;
            ensure!(
                effective.memory.profile == Some(view.profile)
                    && selected.profile == view.profile
                    && selected.group == binding.group
                    && selected.binding_sha256 == binding.digest,
                "shared preference memory root differs"
            );
        }
        drop(reads);
        Ok(Self {
            store,
            admission,
            mode,
            binding,
            capture,
            guard: view.guard,
            catalog_root: view.root_sha.clone(),
            profile: view.profile,
            selected: view.read.map(|r| r.snapshot.token),
            absence: view.absence.map(|(base, slot)| (view.root_sha, base, slot)),
            effective,
            admission_deadline,
            deadline,
            reads: budgets,
            handoff_used: AtomicBool::new(false),
            submitted: AtomicBool::new(false),
        })
    }
    pub(crate) fn open_notebook(
        store: Arc<Store>,
        path: PathBuf,
        started: Instant,
        calls: usize,
    ) -> Result<Self> {
        ensure!(
            (1..=crate::writer::notebook::MAX_CALLS).contains(&calls)
                && !store.ordinary_reader_family()?,
            "unsupported selected notebook admission"
        );
        let mode = store.sync_mode_snapshot()?;
        let binding = Binding::qualified(mode.mode.as_ref().context("shared mode absent")?)?;
        Self::notebook_from_binding(store, path, binding, started, calls, |store| {
            GoogleDrive::open(store.paths.credentials.join("drive.json"))
        })
    }

    pub(super) fn notebook_from_binding(
        store: Arc<Store>,
        path: PathBuf,
        binding: Binding,
        started: Instant,
        calls: usize,
        transport: impl FnOnce(&Store) -> Result<GoogleDrive>,
    ) -> Result<Self> {
        ensure!(
            (1..=crate::writer::notebook::MAX_CALLS).contains(&calls),
            "notebook stage budget out of bounds"
        );
        let lifetime = Duration::from_secs(600);
        let phases = u64::try_from(calls * 2 + 3)?;
        Self::from_binding_at(
            store,
            path,
            binding,
            transport,
            (started + Duration::from_secs(60), started + lifetime),
            RequestReads::for_lifetime(
                lifetime,
                phases,
                crate::writer::notebook::MAX_CANONICAL_BYTES as u64 * phases * (phases + 2),
                true,
            )?,
        )
    }

    fn registered_document(
        &self,
        identity: &crate::memory::catalog::Identity,
        document: &SelectedAdmission,
        store: &Store,
        root: &str,
        reads: &mut SelectedReadBudget,
    ) -> Result<(
        crate::memory::catalog::Registration,
        Entry,
        crate::storage::selection::SelectedSnapshot,
    )> {
        Self::registered_document_bound(
            &self.binding,
            self.guard.mode_epoch,
            identity,
            document.scope(),
            store,
            root,
            reads,
        )
    }

    fn registered_document_bound(
        binding: &Binding,
        epoch: Uuid,
        identity: &crate::memory::catalog::Identity,
        scope: &SelectionScope,
        store: &Store,
        root: &str,
        reads: &mut SelectedReadBudget,
    ) -> Result<(
        crate::memory::catalog::Registration,
        Entry,
        crate::storage::selection::SelectedSnapshot,
    )> {
        use crate::memory::catalog::{Catalog, Owner as DocumentOwner};
        let key = &scope.key_sha256;
        let checkpoint = command::checkpoint(store, binding, epoch, reads)?
            .context("selected document accepted catalog absent")?;
        let view =
            catalog_view(binding, &checkpoint)?.context("selected document catalog root absent")?;
        ensure!(
            view.root_sha == root && view.root.profile == identity.profile,
            "selected document catalog root changed"
        );
        let entry = view
            .entries
            .get(key)
            .context("selected document document is not registered")?;
        let Owner::Memory(owner @ DocumentOwner::Document { .. }) = &entry.owner else {
            anyhow::bail!("selected selected document owner is not a document");
        };
        ensure!(
            identity.aggregate_scope(owner)? == *scope,
            "selected selected document owner differs"
        );
        let local = store
            .selected_snapshot_with_budget(&identity.scope()?, MAX_ITEMS, reads)?
            .context("selected document local registration absent")?;
        let catalog = Catalog::project(identity, &local.snapshot, &local.accepted_history)?;
        let registration = catalog
            .registrations
            .get(key)
            .context("selected document local document registration absent")?;
        ensure!(
            &registration.owner == owner,
            "selected document local registration owner differs"
        );
        let read = store
            .selected_snapshot_with_budget(scope, MAX_ITEMS, reads)?
            .context("selected document registered document is unreceived")?;
        crate::memory::aggregate::Aggregate::project(
            identity,
            registration,
            &read.snapshot,
            &read.accepted_history,
        )?;
        let genesis = read
            .accepted_history
            .last()
            .context("selected document local genesis absent")?;
        let token = &read.snapshot.token;
        if let Some(applied) = checkpoint.scopes.get(key).and_then(|p| p.applied.as_ref()) {
            ensure!(
                token.accepted_base_sha256() == applied.descriptor
                    && read.accepted_history.iter().any(|original| {
                        let transaction = &original.transaction;
                        transaction.operation == applied.operation
                            && transaction.accepted_base_sha256 == applied.descriptor
                            && super::receiving::membership(&transaction.selected)
                                .is_ok_and(|membership| membership == applied.membership)
                            && matches!(
                                transaction.mutation,
                                crate::storage::selection::SelectionMutation::Initialize
                                    | crate::storage::selection::SelectionMutation::Activate
                            )
                    }),
                "selected document applied custody differs"
            );
        } else {
            ensure!(
                genesis.transaction.accepted_base_sha256 == genesis_base(root, key)
                    && token.accepted_base_sha256() == genesis.transaction.accepted_base_sha256,
                "selected document original genesis differs"
            );
        }
        Ok((registration.clone(), entry.clone(), read.snapshot))
    }

    pub(crate) fn notebook_progress(
        &self,
        writer: &SelectedAdmission,
        token: &SelectionToken,
    ) -> Result<()> {
        self.check_joined_progress(Some((writer, token)))
    }

    /// Consume this phase's original read allowance while holding the complete
    /// document/preference/memory gate set. The callback may synchronously
    /// submit or publish, but must never wait for a model response.
    pub(crate) fn with_notebook_phase<T>(
        &self,
        writer: &SelectedAdmission,
        expected: Option<&SelectionToken>,
        phase: NotebookPhase,
        memory: Option<(
            &dyn crate::memory::Provider,
            &crate::memory::QueryResult,
            &SelectionToken,
        )>,
        operation: impl FnOnce(&Store, &mut SelectedReadBudget) -> Result<T>,
    ) -> Result<T> {
        self.with_document_phase(writer, expected, phase, memory, None, operation)
    }

    pub(crate) fn with_document_phase<T>(
        &self,
        writer: &SelectedAdmission,
        expected: Option<&SelectionToken>,
        phase: NotebookPhase,
        memory: Option<(
            &dyn crate::memory::Provider,
            &crate::memory::QueryResult,
            &SelectionToken,
        )>,
        handwriting: Option<&crate::handwriting::ConfirmedContext>,
        operation: impl FnOnce(&Store, &mut SelectedReadBudget) -> Result<T>,
    ) -> Result<T> {
        self.require_writer(writer)?;
        let budget = match phase {
            NotebookPhase::Admission => {
                ensure!(
                    Instant::now() < self.admission_deadline,
                    "notebook admission expired"
                );
                &self.reads.admission
            }
            NotebookPhase::Handoff => &self.reads.handoff,
            NotebookPhase::Publication => &self.reads.finish,
        };
        let mut reads = budget
            .lock()
            .map_err(|_| anyhow::anyhow!("notebook read allowance unavailable"))?;
        let identity = crate::memory::catalog::Identity {
            group: self.binding.group,
            profile: self.profile,
            binding_sha256: self.binding.digest.clone(),
        };
        let catalog = SelectedAdmission::new(self.store.clone(), identity.scope()?)?;
        let owner = handwriting
            .map(crate::handwriting::ConfirmedContext::selected_owner)
            .transpose()?;
        let mut additional = vec![&self.admission, &catalog];
        if let Some(owner) = owner {
            owner.join_gates(&mut additional);
        }
        let mut operation = Some(operation);
        let mut run = |store: &Store| -> Result<T> {
            let _checkpoint = owner
                .map(|owner| owner.verify_locked(store, &mut reads))
                .transpose()?;
            self.verify_locked(store, &mut reads)?;
            if let Some(expected) = expected {
                if !store.selection_unchanged_for_progress(
                    writer.scope(),
                    Some(expected),
                    &mut reads,
                )? {
                    return Err(NotebookSelectionChanged.into());
                }
            }
            let (registration, _, current) =
                self.registered_document(&identity, writer, store, &self.catalog_root, &mut reads)?;
            if expected.is_some_and(|expected| current.token != *expected) {
                return Err(NotebookSelectionChanged.into());
            }
            let crate::memory::catalog::Owner::Document { document } = registration.owner else {
                anyhow::bail!("notebook registration owner changed");
            };
            crate::conversation::reader_uncertainty::validate_document_with_reads(
                store,
                &current,
                document,
                None,
                Some(&mut reads),
            )?;
            ensure!(
                Instant::now() < self.deadline,
                "selected document phase expired"
            );
            operation
                .take()
                .context("notebook phase already consumed")?(store, &mut reads)
        };
        if let Some((memory, view, original)) = memory {
            let mut result = None;
            memory.with_selected_notebook_phase(
                view,
                writer,
                &additional,
                crate::memory::NotebookMemorySelection {
                    original,
                    current: expected.context("notebook memory requires a document selection")?,
                },
                &mut |store| {
                    result = Some(run(store)?);
                    Ok(())
                },
            )?;
            result.context("notebook memory guard did not execute")
        } else {
            additional.push(writer);
            if handwriting.is_some() {
                SelectedAdmission::with_gates(&additional, run)
            } else {
                SelectedAdmission::with_ready_gate_set(&additional, run)
            }
        }
    }

    pub(crate) fn require_writer(&self, writer: &SelectedAdmission) -> Result<()> {
        ensure!(
            writer.uses_store(&self.store)
                && writer.scope().group == self.guard.group
                && writer.scope().binding_sha256 == self.guard.binding_sha256,
            "foreign Writer preference snapshot"
        );
        Ok(())
    }
    pub(crate) fn config(&self) -> &Config {
        &self.effective
    }
    /// Called only inside the complete ordered consuming admission set.
    fn verify_locked(&self, store: &Store, reads: &mut SelectedReadBudget) -> Result<()> {
        ensure!(
            std::ptr::eq(store, self.store.as_ref()),
            "foreign preference Store"
        );
        ensure!(Instant::now() < self.deadline, "preference request expired");
        let current = store.sync_mode_snapshot()?;
        require_mode(&current, &self.binding)?;
        ensure!(
            current.token == self.mode.snapshot().token
                && current.epoch == Some(self.guard.mode_epoch)
                && catalog::current_generation(store)? == self.guard.store_generation,
            "preference mode or generation changed"
        );
        self.mode.require_shared_scope(self.admission.scope())?;
        self.capture.check(reads)?;
        let current =
            store.selected_snapshot_with_budget(self.admission.scope(), MAX_ITEMS, reads)?;
        match (&self.selected, current) {
            (Some(expected), Some(read)) => ensure!(
                read.snapshot.token == *expected,
                "preference selection changed"
            ),
            (None, None) => {
                let (root, base, slot) = self
                    .absence
                    .as_ref()
                    .context("qualified preference absence absent")?;
                let cp = command::checkpoint(store, &self.binding, self.guard.mode_epoch, reads)?
                    .context("preference root custody absent")?;
                let view = catalog_view(&self.binding, &cp)?.context("preference root absent")?;
                ensure!(
                    !cp.catalog.is_empty()
                        && view.root_sha == *root
                        && view.base == *base
                        && view.slot == *slot
                        && !view.entries.contains_key(&record::key_sha256())
                        && cp
                            .catalog_pending
                            .as_ref()
                            .map(CatalogPending::key)
                            .transpose()?
                            .as_deref()
                            != Some(record::key_sha256().as_str())
                        && !cp.scopes.contains_key(&record::key_sha256())
                        && !cp
                            .retired_catalog
                            .iter()
                            .any(|p| p.key == record::key_sha256()),
                    "qualified preference absence changed"
                );
            }
            _ => anyhow::bail!("preference selection or absence changed"),
        }
        Ok(())
    }
    pub(crate) fn check_current(&self) -> Result<()> {
        ensure!(
            Instant::now() < self.admission_deadline,
            "Writer admission window expired"
        );
        let mut reads = self
            .reads
            .admission
            .lock()
            .map_err(|_| anyhow::anyhow!("preference admission allowance unavailable"))?;
        SelectedAdmission::with_gates(&[&self.admission], |store| {
            self.verify_locked(store, &mut reads)
        })
    }
    pub(crate) fn check_progress(&self) -> Result<()> {
        self.check_joined_progress(None)
    }

    fn check_joined_progress(
        &self,
        reader: Option<(&SelectedAdmission, &SelectionToken)>,
    ) -> Result<()> {
        ensure!(Instant::now() < self.deadline, "preference request expired");
        let mut reads = self
            .reads
            .progress
            .lock()
            .map_err(|_| anyhow::anyhow!("preference progress allowance unavailable"))?;
        let mut gates = vec![&self.admission];
        if let Some((admission, _)) = reader {
            gates.push(admission);
        }
        SelectedAdmission::with_gates(&gates, |store| {
            ensure!(
                catalog::current_generation(store)? == self.guard.store_generation,
                "preference generation changed"
            );
            reads.charge(0, 1, 0, 0)?;
            self.capture.check(&mut reads)?;
            ensure!(
                store.selection_unchanged_for_progress(
                    self.admission.scope(),
                    self.selected.as_ref(),
                    &mut reads
                )?,
                "preference progress selection changed"
            );
            if let Some((admission, expected)) = reader {
                ensure!(
                    store.selection_unchanged_for_progress(
                        admission.scope(),
                        Some(expected),
                        &mut reads
                    )?,
                    "Reader progress selection changed"
                );
            }
            Ok(())
        })
    }
    pub(crate) fn handoff(
        &self,
        writer: &SelectedAdmission,
        prepared: &crate::conversation::SelectedWriterPrepared,
        memory: Option<&dyn crate::memory::Provider>,
        view: Option<&crate::memory::QueryResult>,
        submit: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        self.require_writer(writer)?;
        ensure!(
            !self.handoff_used.swap(true, Ordering::AcqRel),
            "preference model handoff already consumed"
        );
        ensure!(
            Instant::now() < self.admission_deadline,
            "Writer admission window expired"
        );
        let mut reads = self
            .reads
            .handoff
            .lock()
            .map_err(|_| anyhow::anyhow!("preference handoff allowance unavailable"))?;
        let mut verify = |store: &Store| {
            self.verify_locked(store, &mut reads)?;
            writer.writer_input_locked(store, prepared)?;
            Ok(())
        };
        let mut submit_once = || {
            ensure!(
                !self.submitted.load(Ordering::Acquire),
                "preference model already submitted"
            );
            submit()?;
            self.submitted.store(true, Ordering::Release);
            Ok(())
        };
        match (memory, view) {
            (Some(memory), Some(view)) => memory.with_selected_writer_handoff(
                view,
                writer,
                &[&self.admission],
                &mut verify,
                &mut submit_once,
            ),
            (None, None) => {
                SelectedAdmission::with_ready_gate_set(&[writer, &self.admission], |store| {
                    verify(store)?;
                    submit_once()
                })
            }
            _ => anyhow::bail!("selected Writer memory handoff absent"),
        }
    }
    pub(crate) fn finish(
        &self,
        writer: &SelectedAdmission,
        finish: crate::conversation::SelectedWriterFinish,
        memory: Option<&dyn crate::memory::Provider>,
        view: Option<&crate::memory::QueryResult>,
    ) -> Result<crate::conversation::SelectedWriterPublication> {
        self.require_writer(writer)?;
        ensure!(
            finish.outcome != crate::conversation::Outcome::Generated
                || self.submitted.load(Ordering::Acquire),
            "model result has no guarded submission"
        );
        let mut reads = self
            .reads
            .finish
            .lock()
            .map_err(|_| anyhow::anyhow!("preference completion allowance unavailable"))?;
        if let (Some(memory), Some(view)) = (memory, view) {
            memory.publish_selected_writer_joined(
                view,
                writer,
                finish,
                &[&self.admission],
                &mut |store| self.verify_locked(store, &mut reads),
            )
        } else {
            ensure!(
                memory.is_none() || finish.outcome != crate::conversation::Outcome::Generated,
                "selected Writer memory publication absent"
            );
            writer.finish_selected_writer_joined(finish, &[&self.admission], |store| {
                self.verify_locked(store, &mut reads)
            })
        }
    }
}

impl RequestPreferences {
    fn detach(self) -> DetachedPreferences {
        let Self {
            store,
            admission,
            mode,
            binding,
            capture,
            guard,
            catalog_root,
            profile,
            selected,
            absence,
            effective,
            admission_deadline,
            deadline,
            reads,
            handoff_used,
            submitted,
        } = self;
        let mode_token = mode.snapshot().token.clone();
        let scope = admission.scope().clone();
        drop(admission);
        drop(mode);
        drop(store);
        DetachedPreferences {
            binding,
            capture,
            guard,
            catalog_root,
            profile,
            selected,
            absence,
            effective,
            admission_deadline,
            deadline,
            reads,
            handoff_used,
            submitted,
            mode_token,
            scope,
        }
    }
}
impl DetachedPreferences {
    fn attach(self, store: Arc<Store>) -> Result<RequestPreferences> {
        let Self {
            binding,
            capture,
            guard,
            catalog_root,
            profile,
            selected,
            absence,
            effective,
            admission_deadline,
            deadline,
            reads,
            handoff_used,
            submitted,
            mode_token,
            scope,
        } = self;
        let mode = store.sync_mode_lease(&mode_token)?;
        mode.require_shared_scope(&scope)?;
        let admission = SelectedAdmission::new(store.clone(), scope)?;
        Ok(RequestPreferences {
            store,
            admission,
            mode,
            binding,
            capture,
            guard,
            catalog_root,
            profile,
            selected,
            absence,
            effective,
            admission_deadline,
            deadline,
            reads,
            handoff_used,
            submitted,
        })
    }
}

#[cfg(test)]
mod request_budget_tests {
    use super::*;
    #[test]
    fn progress_exhaustion_cannot_spend_or_replenish_completion_verification() {
        let mut reads = RequestReads::new().unwrap();
        let pulses = u64::try_from(
            (Duration::from_secs(120) + crate::llm::openai::DEFAULT_TIMEOUT)
                .as_nanos()
                .div_ceil(crate::workflow::indicator::CADENCE.as_nanos()),
        )
        .unwrap()
            + 4;
        let mut progress = reads.progress.lock().unwrap();
        progress.charge(0, pulses * 2, 0, 0).unwrap();
        assert!(progress.charge(0, 1, 0, 0).is_err());
        drop(progress);
        reads
            .finish
            .lock()
            .unwrap()
            .charge(
                64,
                MAX_ITEMS as u64,
                8 * (3 * MAX_METADATA as u64 + 64 * 1024),
                0,
            )
            .unwrap();
        assert!(reads._reservations.reserve(0, 1, 0, 0).is_err());
    }
}

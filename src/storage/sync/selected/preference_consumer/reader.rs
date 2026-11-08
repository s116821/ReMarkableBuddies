//! Reader retains one preference snapshot across multiple guarded stages.
use super::*;
mod target_startup;
use crate::memory::catalog::{Catalog, Identity, Owner as DocumentOwner, Registration};
use std::cell::Cell;

const RESULT_WINDOW: Duration = Duration::from_secs(60);

pub(crate) struct ReaderPreferences {
    inner: RequestPreferences,
    catalog: SelectedAdmission,
    identity: Identity,
    registration: Registration,
    entry: Entry,
    root: String,
    publication_elapsed: Cell<Duration>,
    response_at: Cell<Option<Instant>>,
}

#[derive(Clone, Copy)]
pub(crate) enum ReaderPhase {
    Acquisition,
    Submission,
    Result,
}

impl ReaderPreferences {
    #[cfg(test)]
    pub(crate) fn cap_fixture_windows(&mut self, acquisition: Duration, parent: Duration) {
        let now = Instant::now();
        self.inner.deadline = self.inner.deadline.min(now + parent);
        self.inner.admission_deadline = self
            .inner
            .admission_deadline
            .min(now + acquisition)
            .min(self.inner.deadline);
    }
    pub(crate) fn open(
        store: Arc<Store>,
        path: PathBuf,
        reader: &SelectedAdmission,
        started: Instant,
    ) -> Result<(Self, SelectionToken)> {
        ensure!(
            !store.ordinary_reader_family()?,
            "selected Reader requires format 5"
        );
        let current = store.sync_mode_snapshot()?;
        let binding =
            Binding::qualified(current.mode.as_ref().context("shared Reader mode absent")?)?;
        Self::from_binding(store, path, reader, started, binding, |store| {
            GoogleDrive::open(store.paths.credentials.join("drive.json"))
        })
    }

    pub(in crate::storage::sync::selected) fn from_binding(
        store: Arc<Store>,
        path: PathBuf,
        reader: &SelectedAdmission,
        started: Instant,
        binding: Binding,
        transport: impl FnOnce(&Store) -> Result<GoogleDrive>,
    ) -> Result<(Self, SelectionToken)> {
        let lifetime = Duration::from_secs(420);
        let inner = RequestPreferences::from_binding_at(
            store,
            path,
            binding,
            transport,
            (started + Duration::from_secs(90), started + lifetime),
            RequestReads::for_lifetime(lifetime, 16, MAX_MEDIA * 16, true)?,
        )?;
        inner.require_writer(reader)?;
        ensure!(
            inner.effective.reader_enabled()
                && inner.effective.reader.selection.as_ref() == Some(reader.scope()),
            "effective selected Reader disabled or document changed"
        );
        let mut reads = inner
            .reads
            .admission
            .lock()
            .map_err(|_| anyhow::anyhow!("Reader admission allowance unavailable"))?;
        let initial = command::checkpoint(
            &inner.store,
            &inner.binding,
            inner.guard.mode_epoch,
            &mut reads,
        )?
        .context("Reader accepted catalog absent")?;
        let initial =
            catalog_view(&inner.binding, &initial)?.context("Reader catalog root absent")?;
        let identity = Identity {
            group: inner.binding.group,
            profile: initial.root.profile,
            binding_sha256: inner.binding.digest.clone(),
        };
        let catalog = SelectedAdmission::new(inner.store.clone(), identity.scope()?)?;
        let (registration, entry, token) =
            SelectedAdmission::with_gates(&[reader, &catalog, &inner.admission], |store| {
                inner.verify_locked(store, &mut reads)?;
                let (registration, entry, token) = Self::registered(
                    &inner,
                    &identity,
                    reader,
                    store,
                    &initial.root_sha,
                    &mut reads,
                )?;
                Ok((registration, entry, token))
            })?;
        drop(reads);
        Ok((
            Self {
                inner,
                catalog,
                identity,
                registration,
                entry,
                root: initial.root_sha,
                publication_elapsed: Cell::new(Duration::ZERO),
                response_at: Cell::new(None),
            },
            token,
        ))
    }

    fn registered(
        inner: &RequestPreferences,
        identity: &Identity,
        reader: &SelectedAdmission,
        store: &Store,
        root: &str,
        reads: &mut SelectedReadBudget,
    ) -> Result<(Registration, Entry, SelectionToken)> {
        Self::registered_bound(
            &inner.binding,
            inner.guard.mode_epoch,
            identity,
            reader.scope(),
            store,
            root,
            reads,
        )
    }
    fn registered_bound(
        binding: &Binding,
        epoch: Uuid,
        identity: &Identity,
        scope: &SelectionScope,
        store: &Store,
        root: &str,
        reads: &mut SelectedReadBudget,
    ) -> Result<(Registration, Entry, SelectionToken)> {
        let key = &scope.key_sha256;
        let checkpoint = command::checkpoint(store, binding, epoch, reads)?
            .context("Reader accepted catalog absent")?;
        let view = catalog_view(binding, &checkpoint)?.context("Reader catalog root absent")?;
        ensure!(
            view.root_sha == root && view.root.profile == identity.profile,
            "Reader catalog root changed"
        );
        let entry = view
            .entries
            .get(key)
            .context("Reader document is not registered")?;
        let Owner::Memory(owner @ DocumentOwner::Document { .. }) = &entry.owner else {
            anyhow::bail!("selected Reader owner is not a document");
        };
        ensure!(
            identity.aggregate_scope(owner)? == *scope,
            "selected Reader owner differs"
        );
        let local = store
            .selected_snapshot_with_budget(&identity.scope()?, MAX_ITEMS, reads)?
            .context("Reader local registration absent")?;
        let catalog = Catalog::project(identity, &local.snapshot, &local.accepted_history)?;
        let registration = catalog
            .registrations
            .get(key)
            .context("Reader local document registration absent")?;
        ensure!(
            &registration.owner == owner,
            "Reader local registration owner differs"
        );
        let read = store
            .selected_snapshot_with_budget(scope, MAX_ITEMS, reads)?
            .context("Reader registered document is unreceived")?;
        crate::memory::aggregate::Aggregate::project(
            identity,
            registration,
            &read.snapshot,
            &read.accepted_history,
        )?;
        let genesis = read
            .accepted_history
            .last()
            .context("Reader local genesis absent")?;
        let token = read.snapshot.token;
        if let Some(applied) = checkpoint.scopes.get(key).and_then(|p| p.applied.as_ref()) {
            ensure!(
                token.accepted_base_sha256() == applied.descriptor
                    && read.accepted_history.iter().any(|original| {
                        let transaction = &original.transaction;
                        transaction.operation == applied.operation
                            && transaction.accepted_base_sha256 == applied.descriptor
                            && super::super::receiving::membership(&transaction.selected)
                                .is_ok_and(|membership| membership == applied.membership)
                            && matches!(
                                transaction.mutation,
                                crate::storage::selection::SelectionMutation::Initialize
                                    | crate::storage::selection::SelectionMutation::Activate
                            )
                    }),
                "Reader applied custody differs"
            );
        } else {
            ensure!(
                genesis.transaction.accepted_base_sha256 == genesis_base(root, key)
                    && token.accepted_base_sha256() == genesis.transaction.accepted_base_sha256,
                "Reader original genesis differs"
            );
        }
        Ok((registration.clone(), entry.clone(), token))
    }

    pub(crate) fn config(&self) -> &Config {
        self.inner.config()
    }

    pub(crate) fn deadline(&self) -> Instant {
        self.inner.deadline
    }

    pub(crate) fn mode_token(&self) -> &mode::SyncModeToken {
        &self.inner.mode.snapshot().token
    }

    pub(crate) fn gates(&self) -> [&SelectedAdmission; 2] {
        [&self.catalog, &self.inner.admission]
    }

    pub(crate) fn reads(
        &self,
        phase: ReaderPhase,
    ) -> Result<std::sync::MutexGuard<'_, SelectedReadBudget>> {
        self.require_phase_time(phase)?;
        let reads = match phase {
            ReaderPhase::Acquisition => &self.inner.reads.admission,
            ReaderPhase::Submission => &self.inner.reads.handoff,
            ReaderPhase::Result => &self.inner.reads.finish,
        };
        reads
            .lock()
            .map_err(|_| anyhow::anyhow!("Reader allowance unavailable"))
    }

    pub(crate) fn require_phase_time(&self, phase: ReaderPhase) -> Result<()> {
        match phase {
            ReaderPhase::Acquisition => self.require_acquisition_time(),
            ReaderPhase::Submission => Ok(()),
            ReaderPhase::Result => self.require_result_time(),
        }
    }

    pub(crate) fn verify_locked(
        &self,
        store: &Store,
        reader: &SelectedAdmission,
        expected: &SelectionToken,
        reads: &mut SelectedReadBudget,
    ) -> Result<()> {
        self.inner.require_writer(reader)?;
        self.inner.verify_locked(store, reads)?;
        let (registration, entry, token) = Self::registered(
            &self.inner,
            &self.identity,
            reader,
            store,
            &self.root,
            reads,
        )?;
        ensure!(
            registration == self.registration && entry == self.entry && token == *expected,
            "Reader registered document changed"
        );
        ensure!(Instant::now() < self.deadline(), "Reader request expired");
        Ok(())
    }

    pub(crate) fn require_acquisition_time(&self) -> Result<()> {
        ensure!(
            Instant::now() < self.acquisition_deadline(),
            "Reader acquisition expired"
        );
        Ok(())
    }

    pub(crate) fn acquisition_deadline(&self) -> Instant {
        self.inner.admission_deadline
    }

    fn result_elapsed(&self) -> Duration {
        self.publication_elapsed.get()
            + self
                .response_at
                .get()
                .map_or(Duration::ZERO, |at| at.elapsed())
    }

    pub(crate) fn require_result_time(&self) -> Result<()> {
        ensure!(
            self.result_elapsed() < RESULT_WINDOW,
            "Reader result time exhausted"
        );
        Ok(())
    }

    pub(crate) fn start_model_wait(&self) -> Result<()> {
        let elapsed = self.result_elapsed();
        ensure!(elapsed < RESULT_WINDOW, "Reader result time exhausted");
        self.publication_elapsed.set(elapsed);
        self.response_at.set(None);
        Ok(())
    }

    pub(crate) fn finish_model_wait(&self) {
        self.response_at.set(Some(Instant::now()));
    }

    pub(crate) fn check_progress(
        &self,
        reader: &SelectedAdmission,
        expected: &SelectionToken,
    ) -> Result<()> {
        self.inner.check_joined_progress(Some((reader, expected)))
    }
}

impl ReaderPreferences {
    /// Retain the original OS lease while temporary policy/mode/domain owners
    /// are removed. This consumes the qualified witness and every remaining
    /// request allowance; it never opens another ReaderPreferences or does GET.
    pub(crate) fn promote_targets(
        self,
        store: Arc<Store>,
        reader: Arc<SelectedAdmission>,
        token: SelectionToken,
    ) -> Result<(Arc<Store>, Arc<SelectedAdmission>, Self, SelectionToken)> {
        self.promote_targets_with_hook(store, reader, token, |_| Ok(()))
    }
    pub(crate) fn promote_targets_with_hook(
        self,
        store: Arc<Store>,
        reader: Arc<SelectedAdmission>,
        token: SelectionToken,
        mut hook: impl FnMut(crate::storage::target_generation::Stage) -> Result<()>,
    ) -> Result<(Arc<Store>, Arc<SelectedAdmission>, Self, SelectionToken)> {
        ensure!(reader.uses_store(&store), "target startup Store differs");
        if store.source_target_generation()?.is_some() {
            return Ok((store, reader, self, token));
        }
        let Self {
            inner,
            catalog,
            identity,
            registration,
            entry,
            root,
            publication_elapsed,
            response_at,
        } = self;
        ensure!(
            !inner.handoff_used.load(Ordering::Acquire) && !inner.submitted.load(Ordering::Acquire),
            "target startup already submitted"
        );
        let scope = reader.scope().clone();
        let detached = inner.detach();
        drop(catalog);
        drop(reader);
        let mut store = Arc::try_unwrap(store)
            .map_err(|_| anyhow::anyhow!("target startup requires unique Store ownership"))?;
        {
            let mut reads =
                detached.reads.admission.lock().map_err(|_| {
                    anyhow::anyhow!("original target startup allowance unavailable")
                })?;
            let verify = |reads: &mut SelectedReadBudget| -> Result<()> {
                detached.verify_locked(&store, reads)?;
                let (r, e, t) = Self::registered_bound(
                    &detached.binding,
                    detached.guard.mode_epoch,
                    &identity,
                    &scope,
                    &store,
                    &root,
                    reads,
                )?;
                ensure!(
                    r == registration && e == entry && t == token,
                    "target startup catalog/registration/token changed"
                );
                Ok(())
            };
            verify(&mut reads)?;
        }
        let (pin, operation) = target_startup::freeze(
            &store,
            &detached,
            &identity,
            &registration,
            &entry,
            &root,
            &token,
        )?;
        hook(crate::storage::target_generation::Stage::Policy)?;
        crate::storage::target_generation::prepare_selected_original(
            &mut store, &pin, operation, hook,
        )?;
        {
            let mut reads =
                detached.reads.admission.lock().map_err(|_| {
                    anyhow::anyhow!("original target startup allowance unavailable")
                })?;
            detached.verify_locked(&store, &mut reads)?;
            let (r, e, t) = Self::registered_bound(
                &detached.binding,
                detached.guard.mode_epoch,
                &identity,
                &scope,
                &store,
                &root,
                &mut reads,
            )?;
            ensure!(
                r == registration && e == entry && t == token,
                "target promotion changed original custody"
            );
        }
        let store = Arc::new(store);
        let reader = Arc::new(SelectedAdmission::new(store.clone(), scope)?);
        let inner = detached.attach(store.clone())?;
        let catalog = SelectedAdmission::new(store.clone(), identity.scope()?)?;
        Ok((
            store,
            reader,
            Self {
                inner,
                catalog,
                identity,
                registration,
                entry,
                root,
                publication_elapsed,
                response_at,
            },
            token,
        ))
    }
}

impl ReaderPreferences {
    pub(crate) fn resume_target_preparation(store: &mut Store, config: &Config) -> Result<bool> {
        target_startup::resume(store, config)
    }
}

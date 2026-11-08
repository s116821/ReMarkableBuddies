//! Explicit confirmed operations using the selected Worker and Store custody.
use super::*;
use crate::config::Config;
use crate::handwriting::{
    Availability, Command, Fact, FactState, Failure, MutationOutcome, Report, ValidatedInput,
};
use crate::storage::selection::{
    AcceptedSelectionTransaction, LocalOperationConflict, SelectedReadSnapshot, SelectionChange,
    SelectionPublication,
};

pub(crate) fn run(config: Config, input: ValidatedInput) -> Report {
    let mut report = Report::new(Some(input.request()));
    // A legacy sync declaration alone does not select or create a Store.
    if matches!(config.paths.data.try_exists(), Ok(false)) {
        report.fail(Failure::UnsupportedMode);
        return report;
    }
    let store = match Store::open_existing_for_mode(config.paths) {
        Ok(store) => Arc::new(store),
        Err(refusal) => {
            report.fail(match refusal {
                crate::storage::maintenance::Refusal::Busy => Failure::OwnershipBusy,
                crate::storage::maintenance::Refusal::Unsupported => Failure::UnsupportedMode,
                _ => Failure::StoreUnavailable,
            });
            return report;
        }
    };
    let binding = store.sync_mode_snapshot().ok().and_then(|current| {
        current
            .mode
            .as_ref()
            .and_then(|mode| Binding::qualified(mode).ok())
    });
    let Some(binding) = binding else {
        report.fail(Failure::UnsupportedMode);
        return report;
    };
    let credentials = store.paths.credentials.join("drive.json");
    admitted(store, config.sync, binding, input, || {
        GoogleDrive::open(credentials)
    })
}

pub(in crate::storage::sync::selected) fn admitted(
    store: Arc<Store>,
    policy: SyncPolicy,
    binding: Binding,
    input: ValidatedInput,
    transport: impl FnOnce() -> Result<GoogleDrive>,
) -> Report {
    let mut report = Report::new(Some(input.request()));
    let outcome = (|| -> handwriting::Result<_> {
        let profile = input.request().profile();
        let current = local(store.sync_mode_snapshot())?;
        require_mode(&current, &binding).map_err(|_| Failure::UnsupportedMode)?;
        let epoch = current.epoch.ok_or(Failure::UnsupportedMode)?;
        let lease = local(store.sync_mode_lease(&current.token))?;
        if let handwriting::Request::Query(query) = input.request() {
            super::query::execute(
                &store,
                policy,
                binding,
                &lease,
                query,
                &mut report,
                transport,
            )?;
            return Ok(lease);
        }
        let owner = HandwritingOwner::new(
            profile,
            input
                .request()
                .correction_id()
                .ok_or(Failure::UnsupportedMode)?,
        );
        let scope = owner
            .domain()
            .map_err(|_| Failure::InvalidIdentity)?
            .scope(binding.group, binding.digest.clone())?;
        local(lease.require_shared_scope(&scope))?;
        let catalog_gate = local(catalog_gate(&store, &scope, profile))?;
        let correction_gate = local(SelectedAdmission::new(store.clone(), scope.clone()))?;
        let context = Admission {
            store: &store,
            policy,
            binding,
            scope,
            owner,
            epoch,
        };
        local(SelectedAdmission::with_gates(
            &[&catalog_gate, &correction_gate],
            |_| Ok(context.execute(input, &mut report, transport)),
        ))??;
        Ok(lease)
    })();
    match outcome {
        Ok(lease) => report._guard = Some(Box::new(lease)),
        Err(failure) => report.fail(failure),
    }
    report
}

fn local<T>(result: Result<T>) -> handwriting::Result<T> {
    result.map_err(|_| Failure::StoreUnavailable)
}

struct Admission<'a> {
    store: &'a Arc<Store>,
    policy: SyncPolicy,
    binding: Binding,
    scope: SelectionScope,
    owner: HandwritingOwner,
    epoch: Uuid,
}

impl Admission<'_> {
    fn execute(
        self,
        input: ValidatedInput,
        report: &mut Report,
        transport: impl FnOnce() -> Result<GoogleDrive>,
    ) -> handwriting::Result<()> {
        let command = input.request().command();
        let generation = local(catalog::current_generation(self.store))?;
        crate::storage::compound_generation::validate_ready(
            &self.store.paths,
            self.store.actor_id,
            generation,
        )
        .map_err(|_| Failure::UnsupportedMode)?;
        // Share one allowance for exact local evidence and the bounded fresh
        // catalog observation needed by the first confirmation.
        let mut budget = SelectedReadBudget::new(
            64,
            MAX_ITEMS as u64,
            8 * (3 * MAX_METADATA as u64 + 64 * 1024),
            MAX_MEDIA,
        );
        let snapshot = local(self.store.selected_snapshot_with_budget(
            &self.scope,
            MAX_ITEMS,
            &mut budget,
        ))?;
        let checkpoint = local(preference_command::checkpoint(
            self.store,
            &self.binding,
            self.epoch,
            &mut budget,
        ))?;
        let view = checkpoint
            .as_ref()
            .map(|cp| catalog_view(&self.binding, cp))
            .transpose()
            .map_err(|_| Failure::StoreUnavailable)?
            .flatten();
        if let Some(view) = &view {
            self.require_owner(view)?;
        }
        let mutation = match command {
            Command::Confirm | Command::Update | Command::Forget => {
                Some(handwriting::selected::Mutation::prepare(
                    self.owner.domain().map_err(|_| Failure::InvalidIdentity)?,
                    self.store.actor_id,
                    input,
                )?)
            }
            _ => None,
        };
        if let Some(original) = snapshot.as_ref().and_then(|read| {
            read.accepted_history
                .iter()
                .find(|t| Some(t.transaction.operation) == report.operation)
        }) {
            let root = &view.as_ref().ok_or(Failure::StoreUnavailable)?.root_sha;
            let envelope = self.local_original(original, root)?;
            if mutation
                .as_ref()
                .is_some_and(|mutation| mutation.envelope() != envelope)
            {
                return Err(Failure::OperationConflict);
            }
            let projection = local(project(
                self.store,
                self.owner,
                &original.transaction.selected,
                &mut budget,
            ))?;
            set_fact(report, envelope, projection, true);
            report.transaction = Some(original.transaction.operation);
            return Ok(());
        }
        if command == Command::Status {
            return Ok(());
        }
        if let Some(snapshot) = snapshot {
            let view = view.as_ref().ok_or(Failure::StoreUnavailable)?;
            let projection = self.current(&snapshot, view, &mut budget)?;
            if let Some(mutation) = mutation {
                mutation.check_predecessor(Some(&projection))?;
                let operation = mutation.envelope().operation_id;
                let (envelope, media) = mutation.into_parts();
                let publication = publish(
                    report,
                    self.store.commit_selected(
                        &snapshot.snapshot.token,
                        operation,
                        vec![envelope.clone()],
                        media,
                    ),
                )?;
                self.observe(report, &publication, &envelope, &mut budget)?;
            } else {
                let envelope = snapshot
                    .snapshot
                    .selected_records
                    .iter()
                    .find(|record| record.revision_id == projection.head())
                    .ok_or(Failure::UnsupportedRecord)?;
                set_fact(report, envelope, projection, false);
            }
            return Ok(());
        }

        if command != Command::Confirm {
            return Err(if mutation.is_some() {
                Failure::HeadsConflict
            } else {
                Failure::StoreUnavailable
            });
        }
        if checkpoint.as_ref().is_some_and(|cp| {
            let key = &self.scope.key_sha256;
            cp.scopes.contains_key(key)
                || cp.retired_catalog.iter().any(|p| &p.key == key)
                || cp
                    .catalog_pending
                    .as_ref()
                    .is_some_and(|p| p.key().as_ref().map_or(true, |k| k == key))
        }) {
            return Err(Failure::StoreUnavailable);
        }
        let mutation = mutation.ok_or(Failure::InvalidJson)?;
        mutation.check_predecessor(None)?;
        let mut engine = local(Engine::new(
            self.store.clone(),
            self.policy.clone(),
            local(transport())?,
            self.binding.clone(),
            Arc::new(AtomicBool::new(false)),
        ))?;
        engine.transport.set_request_window(
            Some(Instant::now() + Duration::from_secs(60)),
            Some(engine.cancel.clone()),
        );
        let mut items = MAX_ITEMS;
        let view = local(engine.refresh_root_catalog_bounded(&mut items, Some(&mut budget)))?
            .ok_or(Failure::StoreUnavailable)?;
        self.require_owner(&view)?;
        if !view.complete
            || engine.checkpoint.catalog.is_empty()
            || view.entries.contains_key(&self.scope.key_sha256)
        {
            return Err(Failure::StoreUnavailable);
        }
        let (envelope, media) = mutation.into_parts();
        let publication = publish(report, self.initialize(&view, &envelope, media))?;
        self.observe(report, &publication, &envelope, &mut budget)
    }

    fn require_owner(&self, view: &CatalogView) -> handwriting::Result<()> {
        if view.root.profile != self.owner.profile
            || view
                .entries
                .get(&self.scope.key_sha256)
                .is_some_and(|entry| entry.owner != Owner::Handwriting(self.owner))
        {
            return Err(Failure::InvalidIdentity);
        }
        Ok(())
    }

    fn local_original<'a>(
        &self,
        original: &'a AcceptedSelectionTransaction,
        root: &str,
    ) -> handwriting::Result<&'a Envelope> {
        let transaction = &original.transaction;
        let is_local = transaction.mutation == SelectionMutation::Commit
            || (transaction.mutation == SelectionMutation::Initialize
                && transaction.accepted_base_sha256 == genesis_base(root, &self.scope.key_sha256));
        if !is_local {
            return Err(Failure::OperationConflict);
        }
        original
            .selected_records
            .iter()
            .find(|record| {
                record.actor_id == self.store.actor_id
                    && record.operation_id == transaction.operation
            })
            .ok_or(Failure::OperationConflict)
    }

    fn current(
        &self,
        read: &SelectedReadSnapshot,
        view: &CatalogView,
        budget: &mut SelectedReadBudget,
    ) -> handwriting::Result<Projection> {
        if !view.entries.contains_key(&self.scope.key_sha256) {
            local(require_initial_publication(
                self.store,
                &self.scope,
                self.owner,
                &view.root_sha,
                budget,
            ))?;
        }
        for retained in &read.snapshot.transaction.retained {
            local(project(self.store, self.owner, retained, budget))?;
        }
        local(project(
            self.store,
            self.owner,
            &read.snapshot.transaction.selected,
            budget,
        ))
    }

    fn initialize(
        &self,
        view: &CatalogView,
        envelope: &Envelope,
        mut objects: BTreeMap<String, Vec<u8>>,
    ) -> Result<SelectionPublication> {
        let operation = envelope.operation_id;
        let encoded = serde_json::to_vec(envelope)?;
        let sha256 = digest(&encoded);
        let record = ObjectRef {
            sha256: sha256.clone(),
            bytes: encoded.len() as u64,
        };
        objects.insert(sha256.clone(), encoded);
        let [sample] = envelope.media_descriptors.as_slice() else {
            anyhow::bail!("confirmed handwriting sample absent");
        };
        let manifest = Manifest {
            format: FORMAT,
            transaction_id: operation,
            scope: Scope::SelectedRecords,
            records: vec![record],
            record_namespaces: BTreeMap::from([(sha256, Namespace::Handwriting)]),
            media: vec![ObjectRef {
                sha256: sample.sha256.clone(),
                bytes: sample.bytes,
            }],
            media_coverage: vec![Coverage::Included {
                sha256: sample.sha256.clone(),
            }],
        };
        self.store.initialize_selected(
            &self.scope,
            SelectionChange {
                operation,
                accepted_base_sha256: genesis_base(&view.root_sha, &self.scope.key_sha256),
                selected: manifest,
                retained: vec![],
            },
            objects,
        )
    }

    fn observe(
        &self,
        report: &mut Report,
        publication: &SelectionPublication,
        envelope: &Envelope,
        budget: &mut SelectedReadBudget,
    ) -> handwriting::Result<()> {
        report.transaction = Some(publication.transaction.operation);
        report.diagnostics.mutation_outcome = MutationOutcome::Observed;
        let projection = local(project(
            self.store,
            self.owner,
            &publication.transaction.selected,
            budget,
        ))?;
        set_fact(report, envelope, projection, publication.replayed);
        Ok(())
    }
}

fn publish(
    report: &mut Report,
    result: Result<SelectionPublication>,
) -> handwriting::Result<SelectionPublication> {
    match result {
        Ok(publication) => Ok(publication),
        Err(error) if error.is::<LocalOperationConflict>() => Err(Failure::OperationConflict),
        Err(error) if error.is::<Conflict>() => Err(Failure::HeadsConflict),
        Err(_) => {
            report.diagnostics.mutation_outcome = MutationOutcome::Unknown;
            Err(Failure::CommitUnknown)
        }
    }
}

fn set_fact(report: &mut Report, envelope: &Envelope, projection: Projection, historical: bool) {
    let value = projection.current().cloned();
    let availability = match (&value, projection.sample()) {
        (None, _) => Availability::NotApplicable,
        (Some(_), Some(_)) => Availability::Available,
        (Some(_), None) => Availability::Unavailable,
    };
    report.historical = historical;
    report.diagnostics.scanned = 1;
    report.diagnostics.matched = 1;
    report.diagnostics.missing_media = u64::from(availability == Availability::Unavailable);
    if report.operation.is_some() {
        report.diagnostics.mutation_outcome = MutationOutcome::Observed;
    }
    report.status = if availability == Availability::Unavailable {
        handwriting::Status::Partial
    } else {
        handwriting::Status::Complete
    };
    report.facts.push(Fact {
        record: envelope.record_id,
        revision: envelope.revision_id,
        parents: envelope.parents.clone(),
        head: (!historical).then_some(true),
        conflicted: (!historical).then_some(false),
        state: if value.is_some() {
            FactState::Value
        } else {
            FactState::Tombstone
        },
        value,
        sample_availability: availability,
    });
}

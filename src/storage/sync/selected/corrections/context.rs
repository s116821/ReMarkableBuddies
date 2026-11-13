//! Explicit example capture and guarded revalidation for shared corrections.
use super::*;
use crate::handwriting::{runtime::Selection, Correction, SelectedCapture};
use crate::handwriting::{
    ConfirmedContext, ConfirmedExample, ContextFailure, ContextOwner, ExampleSelection,
};

pub(crate) struct SelectedContextOwner {
    mode: mode::SyncModeLease,
    binding: Binding,
    catalog: SelectedAdmission,
    root: String,
    tail: String,
    corrections: Vec<CorrectionPin>,
    inventory: Option<Vec<String>>,
}

struct CorrectionPin {
    owner: HandwritingOwner,
    admission: SelectedAdmission,
    token: Option<SelectionToken>,
    included: bool,
    progress: (bool, Option<Applied>),
}

impl SelectedContextOwner {
    pub(in crate::storage::sync::selected) fn capture_profile(
        store: &Arc<Store>,
        binding: &Binding,
        profile: Uuid,
        targets: &BTreeSet<String>,
        additional: &[&SelectedAdmission],
        reads: &mut SelectedReadBudget,
        verify: impl FnOnce(&Store, &mut SelectedReadBudget) -> Result<()>,
    ) -> Result<SelectedCapture> {
        let mode = store.sync_mode_snapshot()?;
        require_mode(&mode, binding)?;
        let lease = store.sync_mode_lease(&mode.token)?;
        let catalog = SelectedAdmission::new(
            store.clone(),
            Identity {
                group: binding.group,
                profile,
                binding_sha256: binding.digest.clone(),
            }
            .scope()?,
        )?;
        let (_, expected) = catalog_state(store, &lease, binding, reads)?;
        ensure!(
            expected.root.profile == profile,
            "selected handwriting profile differs"
        );
        let mut corrections = expected
            .entries
            .values()
            .filter_map(|entry| match entry.owner {
                Owner::Handwriting(owner) => Some(owner),
                _ => None,
            })
            .map(|owner| {
                let scope = owner
                    .domain()?
                    .scope(binding.group, binding.digest.clone())
                    .map_err(|error| anyhow::anyhow!("invalid handwriting scope: {error:?}"))?;
                let admission = SelectedAdmission::new(store.clone(), scope)?;
                Ok(CorrectionPin {
                    owner,
                    admission,
                    token: None,
                    included: false,
                    progress: (false, None),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut gates = additional.to_vec();
        gates.push(&catalog);
        gates.extend(corrections.iter().map(|pin| &pin.admission));
        let (inventory, examples, selection, pins) =
            SelectedAdmission::with_gates(&gates, |store| {
                let _checkpoint = store.selected_checkpoint_read()?;
                verify(store, reads)?;
                let (checkpoint, view) = catalog_state(store, &lease, binding, reads)?;
                ensure!(
                    view.root_sha == expected.root_sha
                        && view.base == expected.base
                        && checkpoint.observed_catalog_tail.as_ref() == Some(&view.base),
                    "selected handwriting catalog tail unobserved or changed"
                );
                let inventory = inventory(store, binding, reads)?;
                let unknown_local = inventory
                    .iter()
                    .filter(|key| !view.entries.contains_key(*key))
                    .count();
                let mut selection = Selection {
                    selected: Vec::new(),
                    scanned: corrections.len() as u64,
                    eligible: 0,
                    checked: 0,
                    skipped_media: 0,
                    conflicted: 0,
                    unsupported: unknown_local as u64,
                    unexamined: 0,
                    tombstones: 0,
                    excluded_targets: 0,
                };
                let (examples, pins) = rank(
                    store,
                    &checkpoint,
                    &view,
                    &corrections,
                    targets,
                    reads,
                    &mut selection,
                )?;
                Ok((inventory, examples, selection, pins))
            })?;
        for (pin, (token, included, progress)) in corrections.iter_mut().zip(pins) {
            pin.token = token;
            pin.included = included;
            pin.progress = progress;
        }
        let owner = Self {
            mode: lease,
            binding: binding.clone(),
            catalog,
            root: expected.root_sha,
            tail: expected.base,
            corrections,
            inventory: Some(inventory),
        };
        Ok(SelectedCapture {
            context: ConfirmedContext::from_examples(
                profile,
                examples,
                ContextOwner::Selected(owner),
            ),
            selection,
        })
    }

    pub(crate) fn capture(
        store: Arc<Store>,
        profile: Uuid,
        requested: &[ExampleSelection],
        targets: &BTreeSet<String>,
        reads: &mut SelectedReadBudget,
    ) -> std::result::Result<ConfirmedContext, ContextFailure> {
        if profile.is_nil()
            || !(1..=3).contains(&requested.len())
            || requested
                .iter()
                .any(|example| example.correction.is_nil() || example.revision.is_nil())
            || requested
                .iter()
                .map(|example| example.correction)
                .collect::<BTreeSet<_>>()
                .len()
                != requested.len()
        {
            return Err(ContextFailure::Invalid);
        }
        let mode = store
            .sync_mode_snapshot()
            .map_err(|_| ContextFailure::Storage)?;
        let Some(mode::SyncMode::SharedGroup {
            group,
            root_id,
            binding_sha256,
        }) = &mode.mode
        else {
            return Err(ContextFailure::Unavailable);
        };
        let binding = Binding {
            group: *group,
            root_id: root_id.clone(),
            digest: binding_sha256.clone(),
        };
        let lease = store
            .sync_mode_lease(&mode.token)
            .map_err(|_| ContextFailure::Storage)?;
        let scope = Identity {
            group: *group,
            profile,
            binding_sha256: binding_sha256.clone(),
        }
        .scope()
        .map_err(|_| ContextFailure::Invalid)?;
        lease
            .require_shared_scope(&scope)
            .map_err(|_| ContextFailure::Unavailable)?;
        let catalog =
            SelectedAdmission::new(store.clone(), scope).map_err(|_| ContextFailure::Storage)?;
        let admissions = requested
            .iter()
            .map(|example| {
                let owner = HandwritingOwner::new(profile, example.correction);
                let scope = owner
                    .domain()?
                    .scope(binding.group, binding.digest.clone())
                    .map_err(|error| anyhow::anyhow!("invalid handwriting scope: {error:?}"))?;
                Ok((owner, SelectedAdmission::new(store.clone(), scope)?))
            })
            .collect::<Result<Vec<_>>>()
            .map_err(|_| ContextFailure::Invalid)?;
        let mut gates = vec![&catalog];
        gates.extend(admissions.iter().map(|(_, admission)| admission));
        let result = SelectedAdmission::with_gates(&gates, |store| {
            let _checkpoint = store.selected_checkpoint_read()?;
            let (checkpoint, view) = catalog_state(store, &lease, &binding, reads)?;
            ensure!(
                view.root.profile == profile,
                "selected handwriting profile differs"
            );
            let mut examples = Vec::new();
            let mut tokens = Vec::new();
            for ((owner, admission), requested) in admissions.iter().zip(requested) {
                let (token, projection) =
                    correction(store, *owner, admission.scope(), &checkpoint, &view, reads)?;
                if projection.head() != requested.revision {
                    return Ok(Err(ContextFailure::Stale));
                }
                let value = projection
                    .current()
                    .context("selected handwriting head is a tombstone")?;
                let sample = projection
                    .sample()
                    .context("selected handwriting sample omitted")?;
                ensure!(
                    !targets.contains(&value.sample.sha256),
                    "target image cannot be a handwriting example"
                );
                crate::retrieval::notebook::static_png(sample)?;
                examples.push(
                    ConfirmedExample::from_verified(
                        value.clone(),
                        requested.revision,
                        sample.to_vec(),
                    )
                    .map_err(|error| anyhow::anyhow!("invalid handwriting example: {error:?}"))?,
                );
                tokens.push(token);
            }
            Ok(Ok((view, examples, tokens)))
        })
        .map_err(|_| ContextFailure::Unavailable)??;
        let (view, examples, tokens) = result;
        let corrections = admissions
            .into_iter()
            .zip(tokens)
            .map(|((owner, admission), token)| CorrectionPin {
                owner,
                admission,
                token: Some(token),
                included: true,
                progress: (false, None),
            })
            .collect();
        Ok(ConfirmedContext::from_examples(
            profile,
            examples,
            ContextOwner::Selected(Self {
                mode: lease,
                binding,
                catalog,
                root: view.root_sha,
                tail: view.base,
                corrections,
                inventory: None,
            }),
        ))
    }

    pub(crate) fn with_phase<T>(
        &self,
        gates: &[&SelectedAdmission],
        reads: &mut SelectedReadBudget,
        run: impl FnOnce(&Store) -> Result<T>,
    ) -> Result<T> {
        let mut ordered = gates.to_vec();
        self.join_gates(&mut ordered);
        SelectedAdmission::with_gates(&ordered, |store| {
            let _checkpoint = self.verify_locked(store, reads)?;
            run(store)
        })
    }

    pub(crate) fn join_gates<'a>(&'a self, gates: &mut Vec<&'a SelectedAdmission>) {
        gates.push(&self.catalog);
        gates.extend(self.corrections.iter().map(|pin| &pin.admission));
    }

    pub(crate) fn verify_locked(
        &self,
        store: &Store,
        reads: &mut SelectedReadBudget,
    ) -> Result<std::sync::RwLockReadGuard<'_, ()>> {
        let owner_store = self.catalog.writer_store();
        ensure!(
            std::ptr::eq(store, owner_store.as_ref()),
            "foreign handwriting Store"
        );
        let guard = owner_store.selected_checkpoint_read()?;
        self.mode.require_shared_scope(self.catalog.scope())?;
        let (checkpoint, view) = catalog_state(store, &self.mode, &self.binding, reads)?;
        ensure!(
            view.root_sha == self.root && view.base == self.tail,
            "selected handwriting catalog changed"
        );
        if let Some(expected) = &self.inventory {
            ensure!(
                checkpoint.observed_catalog_tail.as_ref() == Some(&view.base)
                    && inventory(store, &self.binding, reads)? == *expected,
                "selected handwriting inventory changed"
            );
        }
        for pin in &self.corrections {
            if pin.included {
                let (token, projection) = correction(
                    store,
                    pin.owner,
                    pin.admission.scope(),
                    &checkpoint,
                    &view,
                    reads,
                )?;
                ensure!(
                    pin.token.as_ref() == Some(&token) && projection.sample().is_some(),
                    "selected handwriting example changed"
                );
            } else {
                ensure!(
                    pin.progress == correction_progress(&checkpoint, pin.admission.scope())
                        && store.selection_unchanged_for_progress(
                            pin.admission.scope(),
                            pin.token.as_ref(),
                            reads
                        )?,
                    "selected handwriting inventory changed"
                );
            }
        }
        Ok(guard)
    }
}

fn correction_progress(checkpoint: &Checkpoint, scope: &SelectionScope) -> (bool, Option<Applied>) {
    checkpoint
        .scopes
        .get(&scope.key_sha256)
        .map_or((false, None), |progress| {
            (progress.receive.is_some(), progress.applied.clone())
        })
}

type RankedPin = (Option<SelectionToken>, bool, (bool, Option<Applied>));
type RankedCapture = (Vec<ConfirmedExample>, Vec<RankedPin>);

fn rank(
    store: &Store,
    checkpoint: &Checkpoint,
    view: &CatalogView,
    corrections: &[CorrectionPin],
    targets: &BTreeSet<String>,
    reads: &mut SelectedReadBudget,
    selection: &mut Selection,
) -> Result<RankedCapture> {
    let mut candidates = Vec::<(usize, Correction, Uuid, Uuid, bool)>::new();
    let mut pins = Vec::new();
    for (position, item) in corrections.iter().enumerate() {
        let scope = item.admission.scope();
        let read = store.selected_handwriting_metadata(scope, reads)?;
        let token = read.as_ref().map(|read| read.snapshot.token.clone());
        pins.push((token, false, correction_progress(checkpoint, scope)));
        let progress = checkpoint.scopes.get(&scope.key_sha256);
        let Some(read) =
            read.filter(|_| progress.is_none_or(|progress| progress.receive.is_none()))
        else {
            selection.unsupported += 1;
            continue;
        };
        let projection = match current_projection_with_media(
            store,
            item.owner,
            &read,
            &view.root_sha,
            progress,
            reads,
            false,
        ) {
            Ok(projection) => projection,
            Err(_) if !reads.exhausted() => {
                selection.unsupported += 1;
                continue;
            }
            Err(error) => return Err(error),
        };
        let Some(value) = projection.current() else {
            selection.tombstones += 1;
            continue;
        };
        if targets.contains(&value.sample.sha256) {
            selection.excluded_targets += 1;
            continue;
        }
        candidates.push((
            position,
            value.clone(),
            item.owner.domain()?.record_id(),
            projection.head(),
            matches!(projection.coverage(), Coverage::Included { .. }),
        ));
    }
    candidates.sort_by(|a, b| {
        b.1.confirmed_ms
            .cmp(&a.1.confirmed_ms)
            .then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.3.cmp(&b.3))
    });
    selection.eligible = candidates.len();
    let mut examples = Vec::new();
    for (position, value, _, revision, included) in candidates.into_iter().take(16) {
        if examples.len() == 3 {
            break;
        }
        selection.checked += 1;
        if !included {
            selection.skipped_media += 1;
            continue;
        }
        let pin = &corrections[position];
        let verified = correction(
            store,
            pin.owner,
            pin.admission.scope(),
            checkpoint,
            view,
            reads,
        );
        let (token, projection) = match verified {
            Ok(verified) => verified,
            Err(_) if !reads.exhausted() => {
                selection.skipped_media += 1;
                continue;
            }
            Err(error) => return Err(error),
        };
        ensure!(
            pins[position].0.as_ref() == Some(&token) && revision == projection.head(),
            "selected handwriting correction changed"
        );
        let Some(sample) = projection
            .sample()
            .filter(|png| crate::retrieval::notebook::static_png(png).is_ok())
        else {
            selection.skipped_media += 1;
            continue;
        };
        selection.selected.push(ExampleSelection {
            correction: value.correction,
            revision,
        });
        examples.push(
            ConfirmedExample::from_verified(value, revision, sample.to_vec())
                .map_err(|error| anyhow::anyhow!("invalid handwriting example: {error:?}"))?,
        );
        pins[position].1 = true;
    }
    selection.unexamined = selection.eligible - selection.checked;
    Ok((examples, pins))
}

fn inventory(
    store: &Store,
    binding: &Binding,
    reads: &mut SelectedReadBudget,
) -> Result<Vec<String>> {
    Ok(store
        .selected_scope_candidates(
            binding.group,
            &binding.digest,
            Namespace::Handwriting,
            reads,
        )?
        .into_iter()
        .map(|scope| scope.key_sha256)
        .collect())
}

fn catalog_state(
    store: &Store,
    lease: &mode::SyncModeLease,
    binding: &Binding,
    reads: &mut SelectedReadBudget,
) -> Result<(Checkpoint, CatalogView)> {
    let epoch = lease
        .snapshot()
        .epoch
        .context("selected handwriting mode epoch absent")?;
    let generation = catalog::current_generation(store)?;
    crate::storage::compound_generation::validate_ready(&store.paths, store.actor_id, generation)?;
    let checkpoint = preference_command::checkpoint(store, binding, epoch, reads)?
        .context("selected handwriting catalog checkpoint absent")?;
    ensure!(
        !checkpoint.catalog.is_empty(),
        "selected handwriting catalog bootstrap absent"
    );
    let view =
        catalog_view(binding, &checkpoint)?.context("selected handwriting catalog root absent")?;
    Ok((checkpoint, view))
}

fn correction(
    store: &Store,
    owner: HandwritingOwner,
    scope: &SelectionScope,
    checkpoint: &Checkpoint,
    view: &CatalogView,
    reads: &mut SelectedReadBudget,
) -> Result<(SelectionToken, Projection)> {
    let entry = view
        .entries
        .get(&scope.key_sha256)
        .context("selected handwriting correction absent")?;
    ensure!(
        entry.owner == Owner::Handwriting(owner),
        "selected handwriting owner differs"
    );
    let progress = checkpoint.scopes.get(&scope.key_sha256);
    ensure!(
        progress.is_none_or(|progress| progress.receive.is_none()),
        "selected handwriting receive incomplete"
    );
    let read = store
        .selected_snapshot_with_budget(scope, MAX_ITEMS, reads)?
        .context("selected handwriting correction unreceived")?;
    let projection = current_projection(store, owner, &read, &view.root_sha, progress, reads)?;
    Ok((read.snapshot.token, projection))
}

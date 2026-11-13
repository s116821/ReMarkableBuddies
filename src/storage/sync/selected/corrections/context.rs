//! Explicit example capture and guarded revalidation for shared corrections.
use super::*;
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
}

struct CorrectionPin {
    owner: HandwritingOwner,
    admission: SelectedAdmission,
    token: SelectionToken,
}

impl SelectedContextOwner {
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
                token,
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
        ordered.push(&self.catalog);
        ordered.extend(self.corrections.iter().map(|pin| &pin.admission));
        SelectedAdmission::with_gates(&ordered, |store| {
            self.mode.require_shared_scope(self.catalog.scope())?;
            let (checkpoint, view) = catalog_state(store, &self.mode, &self.binding, reads)?;
            ensure!(
                view.root_sha == self.root && view.base == self.tail,
                "selected handwriting catalog changed"
            );
            for pin in &self.corrections {
                let (token, projection) = correction(
                    store,
                    pin.owner,
                    pin.admission.scope(),
                    &checkpoint,
                    &view,
                    reads,
                )?;
                ensure!(
                    token == pin.token && projection.sample().is_some(),
                    "selected handwriting example changed"
                );
            }
            run(store)
        })
    }
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

//! Bounded profile reads from the existing selected catalog and Store.
use super::*;
use crate::handwriting::{Availability, Command, Fact, FactState, Failure, QueryInput, Report};

pub(super) fn execute(
    store: &Arc<Store>,
    policy: SyncPolicy,
    binding: Binding,
    lease: &mode::SyncModeLease,
    query: &QueryInput,
    report: &mut Report,
    transport: impl FnOnce() -> Result<GoogleDrive>,
) -> handwriting::Result<()> {
    let identity = Identity {
        group: binding.group,
        binding_sha256: binding.digest.clone(),
        profile: query.profile,
    };
    let scope = identity.scope().map_err(|_| Failure::InvalidIdentity)?;
    lease
        .require_shared_scope(&scope)
        .map_err(|_| Failure::UnsupportedMode)?;
    let epoch = lease.snapshot().epoch.ok_or(Failure::UnsupportedMode)?;
    let generation = catalog::current_generation(store).map_err(|_| Failure::StoreUnavailable)?;
    crate::storage::compound_generation::validate_ready(&store.paths, store.actor_id, generation)
        .map_err(|_| Failure::UnsupportedMode)?;
    let catalog =
        SelectedAdmission::new(store.clone(), scope).map_err(|_| Failure::StoreUnavailable)?;
    let mut reads = SelectedReadBudget::new(
        MAX_ITEMS as u64,
        MAX_ITEMS as u64,
        8 * (3 * MAX_METADATA as u64 + 64 * 1024),
        MAX_MEDIA,
    );
    let observed = SelectedAdmission::with_gates(&[&catalog], |_| {
        reads.charge(0, 1, MAX_METADATA as u64, 0)?;
        let mut engine = Engine::new(
            store.clone(),
            policy,
            transport()?,
            binding.clone(),
            Arc::new(AtomicBool::new(false)),
        )?;
        engine.transport.set_request_window(
            Some(Instant::now() + Duration::from_secs(60)),
            Some(engine.cancel.clone()),
        );
        let mut items = MAX_ITEMS;
        let view = engine
            .refresh_root_catalog_bounded(&mut items, Some(&mut reads))?
            .context("correction catalog absent")?;
        ensure!(
            view.complete && !engine.checkpoint.catalog.is_empty(),
            "correction catalog tail unverified"
        );
        Ok(view)
    })
    .map_err(|_| Failure::StoreUnavailable)?;
    if observed.root.profile != query.profile {
        return Err(Failure::InvalidIdentity);
    }
    let corrections = observed
        .entries
        .values()
        .filter_map(|entry| {
            if let Owner::Handwriting(owner) = entry.owner {
                Some(owner)
            } else {
                None
            }
        })
        .map(|owner| {
            let scope = owner
                .domain()?
                .scope(binding.group, binding.digest.clone())
                .map_err(|error| anyhow::anyhow!("invalid correction scope: {error:?}"))?;
            Ok((owner, SelectedAdmission::new(store.clone(), scope)?))
        })
        .collect::<Result<Vec<_>>>()
        .map_err(|_| Failure::StoreUnavailable)?;
    let mut gates = vec![&catalog];
    gates.extend(corrections.iter().map(|(_, gate)| gate));
    let facts = SelectedAdmission::with_gates(&gates, |store| {
        let checkpoint = preference_command::checkpoint(store, &binding, epoch, &mut reads)?
            .context("correction catalog checkpoint absent")?;
        let current =
            catalog_view(&binding, &checkpoint)?.context("correction catalog root absent")?;
        ensure!(
            current.root_sha == observed.root_sha
                && current.base == observed.base
                && current.slot == observed.slot
                && current.entries == observed.entries,
            "correction catalog changed during query"
        );
        let local = store.selected_scope_candidates(
            binding.group,
            &binding.digest,
            Namespace::Handwriting,
            &mut reads,
        )?;
        let mut pending: BTreeSet<_> = local
            .into_iter()
            .map(|scope| scope.key_sha256)
            .filter(|key| !observed.entries.contains_key(key))
            .collect();
        if let Some(original) = &checkpoint.catalog_pending {
            if original.handwriting.is_some() {
                let key = original.key()?;
                if !observed.entries.contains_key(&key) {
                    pending.insert(key);
                }
            }
        }
        report.diagnostics.omitted += pending.len() as u64;
        let mut facts = Vec::new();
        for (owner, gate) in &corrections {
            report.diagnostics.scanned += 1;
            let progress = checkpoint.scopes.get(&gate.scope().key_sha256);
            if progress.is_some_and(|progress| progress.receive.is_some()) {
                report.diagnostics.omitted += 1;
                continue;
            }
            match current_fact(
                store,
                *owner,
                gate.scope(),
                &current.root_sha,
                progress,
                &mut reads,
            ) {
                Ok(Some(fact)) => facts.push(fact),
                Ok(None) => {}
                Err(_) => {
                    report.diagnostics.unsupported += 1;
                    if reads.exhausted() {
                        break;
                    }
                }
            }
        }
        Ok(facts)
    })
    .map_err(|_| Failure::StoreUnavailable)?;
    fit(query, report, facts)
}

fn current_fact(
    store: &Store,
    owner: HandwritingOwner,
    scope: &SelectionScope,
    root: &str,
    progress: Option<&Progress>,
    reads: &mut SelectedReadBudget,
) -> Result<Option<Fact>> {
    let read = store
        .selected_snapshot_with_budget(scope, MAX_ITEMS, reads)?
        .context("registered correction unreceived")?;
    let projection = current_projection(store, owner, &read, root, progress, reads)?;
    let Some(value) = projection.current().cloned() else {
        return Ok(None);
    };
    let envelope = read
        .snapshot
        .selected_records
        .iter()
        .find(|record| record.revision_id == projection.head())
        .context("correction head absent")?;
    Ok(Some(Fact {
        record: envelope.record_id,
        revision: envelope.revision_id,
        parents: envelope.parents.clone(),
        head: Some(true),
        conflicted: Some(false),
        state: FactState::Value,
        value: Some(value),
        sample_availability: if projection.sample().is_some() {
            Availability::Available
        } else {
            Availability::Unavailable
        },
    }))
}

fn fit(query: &QueryInput, report: &mut Report, facts: Vec<Fact>) -> handwriting::Result<()> {
    let words = query
        .query_text
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    let mut candidate = Report::new(None);
    candidate.command = Some(Command::Query);
    candidate.status = handwriting::Status::Partial;
    candidate.diagnostics.scanned = u64::MAX;
    candidate.diagnostics.matched = u64::MAX;
    candidate.diagnostics.omitted = u64::MAX;
    candidate.diagnostics.conflicted = u64::MAX;
    candidate.diagnostics.unsupported = u64::MAX;
    candidate.diagnostics.missing_media = u64::MAX;
    for fact in facts {
        let value = fact.value.as_ref().ok_or(Failure::UnsupportedRecord)?;
        let text = format!(
            "{}\n{}",
            value.original_text.as_deref().unwrap_or_default(),
            value.corrected_text
        )
        .to_lowercase();
        if !words.iter().all(|word| text.contains(word)) {
            continue;
        }
        report.diagnostics.matched += 1;
        if fact.sample_availability != Availability::Available {
            report.diagnostics.missing_media += 1;
        } else if candidate.facts.len() == query.max_matches {
            report.diagnostics.omitted += 1;
        } else {
            candidate.facts.push(fact);
            if candidate.size() > query.max_encoded_bytes {
                candidate.facts.pop();
                report.diagnostics.omitted += 1;
            }
        }
    }
    report.facts = candidate.facts;
    report.status = if report.diagnostics.omitted
        + report.diagnostics.unsupported
        + report.diagnostics.missing_media
        > 0
    {
        handwriting::Status::Partial
    } else if report.facts.is_empty() {
        handwriting::Status::CompleteEmpty
    } else {
        handwriting::Status::Complete
    };
    if report.size() > query.max_encoded_bytes {
        return Err(Failure::OutputLimit);
    }
    Ok(())
}

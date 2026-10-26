//! Ordinary local domain operations; immutable returned facts are historical data.
use super::*;
use crate::{
    config::Config,
    storage::{self, ObjectRef, Store, MAX_ITEMS},
};
use std::{collections::BTreeMap, sync::Arc};

pub trait Provider {
    fn execute(self, input: ValidatedInput) -> Report;
}
pub struct LocalCorrections {
    store: Arc<Store>,
    _ordinary: storage::sync::mode::OrdinaryModeLease,
    _local: storage::LocalDomainLease,
}
impl LocalCorrections {
    fn lineage(envelope: &Envelope, history: &[Envelope]) -> Result<Option<Correction>> {
        let original = decode(envelope)?;
        let mut anchor = original.clone();
        let mut pending = vec![(envelope.revision_id, false)];
        let mut seen = BTreeSet::new();
        let mut active = BTreeSet::new();
        let records = history
            .iter()
            .map(|e| (e.revision_id, e))
            .collect::<BTreeMap<_, _>>();
        while let Some((revision, exit)) = pending.pop() {
            if exit {
                active.remove(&revision);
                seen.insert(revision);
                continue;
            }
            if seen.contains(&revision) {
                continue;
            }
            if !active.insert(revision) {
                return Err(Failure::UnsupportedRecord);
            }
            pending.push((revision, true));
            let e = records.get(&revision).ok_or(Failure::UnsupportedRecord)?;
            if e.record_id != envelope.record_id {
                return Err(Failure::UnsupportedRecord);
            }
            match decode(e)? {
                Some(value) => {
                    if anchor.as_ref().is_some_and(|a| !a.same_original(&value)) {
                        return Err(Failure::UnsupportedRecord);
                    }
                    if anchor.is_none() {
                        anchor = Some(value);
                    }
                }
                None if e.parents.is_empty() => return Err(Failure::UnsupportedRecord),
                None => {}
            }
            pending.extend(e.parents.iter().copied().map(|id| (id, false)));
        }
        if anchor.is_none() {
            return Err(Failure::UnsupportedRecord);
        }
        Ok(original)
    }
    fn heads(history: &[Envelope]) -> Vec<&Envelope> {
        let parents = history
            .iter()
            .flat_map(|e| e.parents.iter().copied())
            .collect::<BTreeSet<_>>();
        history
            .iter()
            .filter(|e| !parents.contains(&e.revision_id))
            .collect()
    }
    pub fn new(store: Arc<Store>, config: &Config) -> Result<Self> {
        if config.sync.enabled
            || config.paths.data != store.paths.data
            || config.paths.cache != store.paths.cache
            || config.paths.credentials != store.paths.credentials
        {
            return Err(Failure::UnsupportedMode);
        }
        let current = store
            .sync_mode_snapshot()
            .map_err(|_| Failure::StoreUnavailable)?;
        let ordinary = store
            .ordinary_mode_lease(&current.token)
            .map_err(|_| Failure::UnsupportedMode)?;
        let local = store
            .local_domain_lease()
            .map_err(|_| Failure::UnsupportedMode)?;
        Ok(Self {
            store,
            _ordinary: ordinary,
            _local: local,
        })
    }
    fn available(&self) -> Result<()> {
        if self
            .store
            .unavailable_commits()
            .map_err(|_| Failure::StoreUnavailable)?
            != 0
        {
            return Err(Failure::StoreUnavailable);
        }
        Ok(())
    }
    fn retained(&self, filter: impl Fn(&Envelope) -> bool) -> Result<Vec<Envelope>> {
        self.store
            .snapshot_revisions_matching(&[Namespace::Handwriting], MAX_ITEMS, filter)
            .map_err(|_| Failure::StoreUnavailable)
    }
    #[cfg(test)]
    pub(crate) fn publisher_excluded_for_test(&self) -> bool {
        self.store.legacy_publisher_lease().is_err()
    }
    /// Capture all selected current heads from one bounded retained snapshot.
    /// Stricter PNG eligibility belongs to this consumer, not stored validity.
    pub fn capture_context(
        self,
        profile: Uuid,
        selected: &[ExampleSelection],
        progress: &mut dyn FnMut() -> std::result::Result<(), ContextFailure>,
    ) -> std::result::Result<ConfirmedContext, (ContextFailure, LocalCorrections)> {
        let admitted = (|| -> std::result::Result<Vec<ConfirmedExample>, ContextFailure> {
            progress()?;
            if profile.is_nil()
                || selected.is_empty()
                || selected.len() > 3
                || selected
                    .iter()
                    .any(|s| s.correction.is_nil() || s.revision.is_nil())
                || selected
                    .iter()
                    .map(|s| s.correction)
                    .collect::<BTreeSet<_>>()
                    .len()
                    != selected.len()
            {
                return Err(ContextFailure::Invalid);
            }
            self.available().map_err(|_| ContextFailure::Storage)?;
            let keys = selected
                .iter()
                .map(|s| record_id(profile, s.correction))
                .collect::<Result<BTreeSet<_>>>()
                .map_err(|_| ContextFailure::Invalid)?;
            let history = self
                .retained(|e| keys.contains(&e.record_id))
                .map_err(|_| ContextFailure::Storage)?;
            let mut examples = Vec::new();
            for selection in selected {
                progress()?;
                let record = record_id(profile, selection.correction)
                    .map_err(|_| ContextFailure::Invalid)?;
                let retained = history
                    .iter()
                    .filter(|e| e.record_id == record)
                    .cloned()
                    .collect::<Vec<_>>();
                let heads = Self::heads(&retained);
                if heads.len() > 1 {
                    return Err(ContextFailure::Conflict);
                }
                let head = heads.first().ok_or(ContextFailure::Unavailable)?;
                if head.revision_id != selection.revision {
                    return Err(ContextFailure::Stale);
                }
                let value = Self::lineage(head, &retained)
                    .map_err(|_| ContextFailure::Unavailable)?
                    .ok_or(ContextFailure::Unavailable)?;
                if value.profile != profile || value.correction != selection.correction {
                    return Err(ContextFailure::Unavailable);
                }
                let png = self
                    .store
                    .read_object(&ObjectRef {
                        sha256: value.sample.sha256.clone(),
                        bytes: value.sample.bytes,
                    })
                    .map_err(|_| ContextFailure::Unavailable)?;
                // Container framing/CRC/static checks do not decode another image.
                crate::retrieval::notebook::static_png(&png)
                    .map_err(|_| ContextFailure::Unavailable)?;
                value
                    .verify_sample(&png)
                    .map_err(|_| ContextFailure::Unavailable)?;
                examples.push(ConfirmedExample {
                    identity: ExampleIdentity {
                        correction: selection.correction,
                        record,
                        revision: selection.revision,
                        sample_sha256: value.sample.sha256.clone(),
                        original_text_sha256: digest(value.original_text.as_bytes()),
                        corrected_text_sha256: digest(value.corrected_text.as_bytes()),
                    },
                    png,
                    original_text: value.original_text,
                    corrected_text: value.corrected_text,
                });
            }
            progress()?;
            Ok(examples)
        })();
        let examples = match admitted {
            Ok(values) => values,
            Err(failure) => return Err((failure, self)),
        };
        let identity = ContextIdentity {
            profile,
            examples: examples.iter().map(|e| e.identity.clone()).collect(),
        };
        Ok(ConfirmedContext {
            identity,
            examples,
            _owner: self,
        })
    }
    fn operations(&self, operation: Uuid) -> Result<Vec<Envelope>> {
        self.store
            .snapshot_revisions_matching(
                &[
                    Namespace::Conversation,
                    Namespace::Source,
                    Namespace::ExportAssociation,
                    Namespace::SubjectMemory,
                    Namespace::Handwriting,
                ],
                MAX_ITEMS,
                |e| e.operation_id == operation,
            )
            .map_err(|_| Failure::StoreUnavailable)
    }
    fn media(&self, value: &Correction) -> bool {
        if value.validate().is_err() {
            return false;
        }
        self.store
            .read_object(&ObjectRef {
                sha256: value.sample.sha256.clone(),
                bytes: value.sample.bytes,
            })
            .is_ok_and(|bytes| value.verify_sample(&bytes).is_ok())
    }
    fn fact(
        &self,
        envelope: &Envelope,
        head: Option<bool>,
        conflicted: Option<bool>,
        report: &mut Report,
        history: &[Envelope],
    ) -> Fact {
        let (state, value, sample_availability) = match Self::lineage(envelope, history) {
            Ok(Some(value)) => {
                let available = self.media(&value);
                if !available {
                    report.diagnostics.missing_media += 1;
                }
                (
                    FactState::Value,
                    Some(value),
                    if available {
                        Availability::Available
                    } else {
                        Availability::Unavailable
                    },
                )
            }
            Ok(None) => (FactState::Tombstone, None, Availability::NotApplicable),
            Err(_) => {
                report.diagnostics.unsupported += 1;
                (FactState::Unsupported, None, Availability::NotExamined)
            }
        };
        Fact {
            record: envelope.record_id,
            revision: envelope.revision_id,
            parents: envelope.parents.clone(),
            head,
            conflicted,
            state,
            value,
            sample_availability,
        }
    }
    fn status(&self, input: &StatusInput, report: &mut Report) -> Result<()> {
        let values = self.operations(input.operation)?;
        report.diagnostics.scanned += values.len() as u64;
        if values.is_empty() {
            return Ok(());
        }
        if values.len() != 1
            || values[0].namespace != Namespace::Handwriting
            || Some(values[0].record_id) != report.record
        {
            return Err(Failure::OperationConflict);
        }
        let history = self.retained(|e| e.record_id == values[0].record_id)?;
        report.diagnostics.scanned += history.len() as u64;
        Self::lineage(&values[0], &history)?;
        report.historical = true;
        report.diagnostics.mutation_outcome = MutationOutcome::Observed;
        let fact = self.fact(&values[0], None, None, report, &history);
        report.status = if fact.sample_availability == Availability::Unavailable {
            Status::Partial
        } else {
            Status::Complete
        };
        report.facts.push(fact);
        Ok(())
    }
    fn mutate(&self, input: &ValidatedInput, report: &mut Report) -> Result<()> {
        let (action, operation, expected) = match &input.request {
            Request::Confirm(v) => (
                Action::Confirm,
                v.operation,
                v.expected_heads.iter().copied().collect(),
            ),
            Request::Update(v) => (
                Action::Update,
                v.operation,
                v.expected_heads.iter().copied().collect(),
            ),
            Request::Forget(v) => (
                Action::Forget,
                v.operation,
                v.expected_heads.iter().copied().collect(),
            ),
            _ => return Err(Failure::InvalidJson),
        };
        let record = report.record.ok_or(Failure::InvalidIdentity)?;
        let value_hash = input
            .correction
            .as_ref()
            .map(Correction::value_digest)
            .transpose()?;
        let (_, revision) =
            mutation_identity(action, operation, record, &expected, value_hash.as_deref())?;
        let mut envelope = Envelope {
            envelope_version: 1,
            namespace: Namespace::Handwriting,
            domain_schema_version: 1,
            record_id: record,
            revision_id: revision,
            parents: expected,
            operation_id: operation,
            actor_id: self.store.actor_id,
            kind: if action == Action::Forget {
                Kind::Tombstone
            } else {
                Kind::Value
            },
            payload: input
                .correction
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(|_| Failure::StorageFailure)?
                .unwrap_or(serde_json::Value::Null),
            media_descriptors: input
                .correction
                .as_ref()
                .map(|v| vec![v.sample.clone()])
                .unwrap_or_default(),
        };
        let prior = self.operations(operation)?;
        report.diagnostics.scanned += prior.len() as u64;
        if !prior.is_empty() {
            if prior.len() != 1 {
                return Err(Failure::OperationConflict);
            }
            // A restore can change the current actor; historical bytes stay original.
            envelope.actor_id = prior[0].actor_id;
            if prior[0] != envelope {
                return Err(Failure::OperationConflict);
            }
            let history = self.retained(|e| e.record_id == record)?;
            report.diagnostics.scanned += history.len() as u64;
            Self::lineage(&prior[0], &history)?;
            report.historical = true;
            report.diagnostics.mutation_outcome = MutationOutcome::Observed;
            let fact = self.fact(&prior[0], None, None, report, &history);
            if fact.sample_availability == Availability::Unavailable {
                return Err(Failure::MediaUnavailable);
            }
            report.facts.push(fact);
            report.status = Status::Complete;
            return Ok(());
        }
        let mut history = self.retained(|e| e.record_id == record)?;
        report.diagnostics.scanned += history.len() as u64;
        if action != Action::Confirm && history.is_empty() {
            return Err(Failure::HeadsConflict);
        }
        for old in &history {
            if Self::lineage(old, &history)?
                .as_ref()
                .zip(input.correction.as_ref())
                .is_some_and(|(old, value)| !old.same_original(value))
            {
                return Err(Failure::ImmutableChange);
            }
        }
        let media = input
            .correction
            .as_ref()
            .zip(input.sample.as_ref())
            .map(|(v, b)| BTreeMap::from([(v.sample.sha256.clone(), b.clone())]))
            .unwrap_or_default();
        match self
            .store
            .commit_with_disposition(vec![envelope.clone()], media)
        {
            Ok((transaction, fresh)) => {
                report.historical = !fresh;
                report.transaction = if fresh { Some(transaction) } else { None };
                report.diagnostics.mutation_outcome = MutationOutcome::Observed;
                history.push(envelope.clone());
                let fact = self.fact(
                    &envelope,
                    if fresh { Some(true) } else { None },
                    if fresh { Some(false) } else { None },
                    report,
                    &history,
                );
                if fact.sample_availability == Availability::Unavailable {
                    return Err(Failure::MediaUnavailable);
                }
                report.facts.push(fact);
                report.status = Status::Complete;
                Ok(())
            }
            Err(error) if error.downcast_ref::<storage::Conflict>().is_some() => {
                Err(Failure::HeadsConflict)
            }
            Err(_) => {
                report.diagnostics.mutation_outcome = MutationOutcome::Unknown;
                Err(Failure::CommitUnknown)
            }
        }
    }
    fn inspect(&self, record: Uuid, report: &mut Report) -> Result<()> {
        let history = self.retained(|e| e.record_id == record)?;
        report.diagnostics.scanned += history.len() as u64;
        let heads = Self::heads(&history);
        if heads.is_empty() && !history.is_empty() {
            return Err(Failure::UnsupportedRecord);
        }
        if heads.len() > 16 {
            return Err(Failure::OutputLimit);
        }
        if heads.is_empty() {
            return Ok(());
        }
        if heads.len() > 1 {
            report.diagnostics.conflicted = 1;
        }
        for envelope in &heads {
            let fact = self.fact(
                envelope,
                Some(true),
                Some(heads.len() > 1),
                report,
                &history,
            );
            report.facts.push(fact);
        }
        report.status = if report.diagnostics.unsupported == heads.len() as u64 {
            report.failure = Some(Failure::UnsupportedRecord);
            Status::Unsupported
        } else if report.diagnostics.conflicted
            + report.diagnostics.unsupported
            + report.diagnostics.missing_media
            > 0
        {
            Status::Partial
        } else {
            Status::Complete
        };
        Ok(())
    }
    fn query(&self, input: &QueryInput, report: &mut Report) -> Result<()> {
        // One complete retained metadata snapshot, including causal closure.
        // Store caps this at4096 envelopes/8MiB; no per-key repeated disk scans.
        let history = self.retained(|_| true)?;
        report.diagnostics.scanned += history.len() as u64;
        let mut groups = BTreeMap::<Uuid, Vec<Envelope>>::new();
        for envelope in history {
            groups.entry(envelope.record_id).or_default().push(envelope);
        }
        let words = input
            .query_text
            .as_deref()
            .unwrap_or("")
            .split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>();
        let mut selected = Vec::new();
        for group in groups.values() {
            let heads = Self::heads(group);
            if heads.is_empty() {
                report.diagnostics.unsupported += 1;
                continue;
            }
            let mut values = Vec::new();
            for envelope in heads.iter().copied() {
                match Self::lineage(envelope, group) {
                    Ok(Some(value)) if value.profile == input.profile => {
                        values.push((envelope, value))
                    }
                    Ok(_) => {}
                    Err(_) => report.diagnostics.unsupported += 1,
                }
            }
            if heads.len() > 1 {
                if !values.is_empty()
                    || group.iter().any(|e| {
                        decode(e)
                            .ok()
                            .flatten()
                            .is_some_and(|v| v.profile == input.profile)
                    })
                {
                    report.diagnostics.conflicted += 1;
                }
                continue;
            }
            if let Some((envelope, value)) = values.pop() {
                let haystack =
                    format!("{}\n{}", value.original_text, value.corrected_text).to_lowercase();
                if !words.iter().all(|w| haystack.contains(w)) {
                    continue;
                }
                report.diagnostics.matched += 1;
                if selected.len() >= input.max_matches {
                    report.diagnostics.omitted += 1;
                    continue;
                }
                selected.push((envelope.clone(), value));
            }
        }
        // Reserve maximum diagnostics/status/failure/header before media reads.
        let mut candidate = Report::new(Some(&Request::Query(QueryInput {
            handwriting_version: 1,
            profile: input.profile,
            query_text: input.query_text.clone(),
            max_matches: input.max_matches,
            max_encoded_bytes: input.max_encoded_bytes,
        })));
        candidate.status = Status::Partial;
        candidate.diagnostics.scanned = u64::MAX;
        candidate.diagnostics.matched = u64::MAX;
        candidate.diagnostics.omitted = u64::MAX;
        candidate.diagnostics.conflicted = u64::MAX;
        candidate.diagnostics.unsupported = u64::MAX;
        candidate.diagnostics.missing_media = u64::MAX;
        for (envelope, value) in selected {
            candidate.facts.push(Fact {
                record: envelope.record_id,
                revision: envelope.revision_id,
                parents: envelope.parents.clone(),
                head: Some(true),
                conflicted: Some(false),
                state: FactState::Value,
                value: Some(value),
                sample_availability: Availability::Unavailable,
            });
            if candidate.size() > input.max_encoded_bytes {
                candidate.facts.pop();
                report.diagnostics.omitted += 1;
            }
        }
        if candidate.size() > input.max_encoded_bytes {
            return Err(Failure::OutputLimit);
        }
        // Only the budgeted subset is image-verified: at most16*512KiB.
        for mut fact in candidate.facts {
            if self.media(fact.value.as_ref().ok_or(Failure::UnsupportedRecord)?) {
                fact.sample_availability = Availability::Available;
                report.facts.push(fact);
            } else {
                report.diagnostics.missing_media += 1;
            }
        }
        report.status = if report.diagnostics.omitted
            + report.diagnostics.conflicted
            + report.diagnostics.unsupported
            + report.diagnostics.missing_media
            > 0
        {
            Status::Partial
        } else if report.facts.is_empty() {
            Status::CompleteEmpty
        } else {
            Status::Complete
        };
        if report.size() > input.max_encoded_bytes {
            return Err(Failure::OutputLimit);
        }
        Ok(())
    }
}
impl Provider for LocalCorrections {
    fn execute(self, input: ValidatedInput) -> Report {
        let mut report = Report::new(Some(&input.request));
        let outcome = self.available().and_then(|()| match &input.request {
            Request::Confirm(_) | Request::Update(_) | Request::Forget(_) => {
                self.mutate(&input, &mut report)
            }
            Request::Status(v) => self.status(v, &mut report),
            Request::Inspect(_) => {
                self.inspect(report.record.ok_or(Failure::InvalidIdentity)?, &mut report)
            }
            Request::Query(v) => self.query(v, &mut report),
        });
        if let Err(error) = outcome {
            report.fail(error);
        }
        if report.size() > report::MAX_REPORT {
            report.fail(Failure::OutputLimit);
        }
        report._guard = Some(Box::new(self));
        report
    }
}

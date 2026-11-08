//! Shared Handwriting1 validation and mutation preparation.
//! Callers own mode, scope, replay and Store publication admission.
use super::*;
use crate::storage::{selection::SelectionScope, Coverage, Manifest, ObjectRef, Scope};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owner {
    profile: Uuid,
    correction: Uuid,
    record: Uuid,
}
impl Owner {
    pub fn new(profile: Uuid, correction: Uuid) -> Result<Self> {
        Ok(Self {
            profile,
            correction,
            record: record_id(profile, correction)?,
        })
    }

    pub fn record_id(self) -> Uuid {
        self.record
    }

    pub fn key_sha256(self) -> String {
        digest(
            format!(
                "urn:remarkable-buddies:global-record:v1:handwriting:{}",
                self.record
            )
            .as_bytes(),
        )
    }

    pub fn scope(self, group: Uuid, binding_sha256: String) -> Result<SelectionScope> {
        let scope = SelectionScope {
            group,
            key_sha256: self.key_sha256(),
            binding_sha256,
        };
        scope.validate().map_err(|_| Failure::InvalidIdentity)?;
        Ok(scope)
    }
}

/// One verified manifest. Apply independently to the active and each retained
/// closure. A retained projection is historical evidence and grants no admission.
pub struct Projection {
    owner: Owner,
    head: Uuid,
    original: Correction,
    current: Option<Correction>,
    coverage: Coverage,
    sample: Option<Vec<u8>>,
}
impl Projection {
    /// The loader is bound to the exact manifest objects and charged to the
    /// caller's shared receive/read budget before reading. This method validates
    /// their original bytes without requiring canonical JSON formatting.
    pub fn read(
        owner: Owner,
        manifest: &Manifest,
        mut read: impl FnMut(&ObjectRef) -> anyhow::Result<Vec<u8>>,
    ) -> Result<Self> {
        manifest
            .validate()
            .map_err(|_| Failure::UnsupportedRecord)?;
        if manifest.scope != Scope::SelectedRecords {
            return Err(Failure::UnsupportedRecord);
        }
        let mut history = Vec::with_capacity(manifest.records.len());
        for reference in &manifest.records {
            if manifest.record_namespaces.get(&reference.sha256) != Some(&Namespace::Handwriting) {
                return Err(Failure::UnsupportedRecord);
            }
            let bytes = read_exact(&mut read, reference).ok_or(Failure::UnsupportedRecord)?;
            let raw = crate::config::inspection::strict_json(&bytes)
                .map_err(|_| Failure::UnsupportedRecord)?;
            let envelope: Envelope =
                serde_json::from_value(raw).map_err(|_| Failure::UnsupportedRecord)?;
            if envelope.namespace != Namespace::Handwriting
                || envelope.record_id != owner.record
                || envelope.parents.len() > 1
            {
                return Err(Failure::UnsupportedRecord);
            }
            history.push(envelope);
        }
        let (head, genesis) = single_chain(&history)?;
        let current = LocalCorrections::lineage(head, &history)?;
        let original = decode(genesis)?.ok_or(Failure::UnsupportedRecord)?;
        if original.profile != owner.profile || original.correction != owner.correction {
            return Err(Failure::UnsupportedRecord);
        }

        let [coverage] = manifest.media_coverage.as_slice() else {
            return Err(Failure::UnsupportedRecord);
        };
        if coverage.hash() != original.sample.sha256 {
            return Err(Failure::UnsupportedRecord);
        }
        let sample = if matches!(coverage, Coverage::Included { .. }) {
            let reference = ObjectRef {
                sha256: original.sample.sha256.clone(),
                bytes: original.sample.bytes,
            };
            if manifest.media.as_slice() != std::slice::from_ref(&reference) {
                return Err(Failure::UnsupportedRecord);
            }
            let bytes = read_exact(&mut read, &reference).ok_or(Failure::MediaUnavailable)?;
            original.verify_sample(&bytes)?;
            current.as_ref().map(|_| bytes)
        } else {
            None
        };
        Ok(Self {
            owner,
            head: head.revision_id,
            original,
            current,
            coverage: coverage.clone(),
            sample,
        })
    }

    pub fn head(&self) -> Uuid {
        self.head
    }

    /// None denotes an explicit tombstone with a verified causal original.
    pub fn current(&self) -> Option<&Correction> {
        self.current.as_ref()
    }

    pub fn coverage(&self) -> &Coverage {
        &self.coverage
    }

    /// Only an active Value with Included coverage exposes verified sample bytes.
    /// Recognition may impose additional eligibility checks on these bytes.
    pub fn sample(&self) -> Option<&[u8]> {
        self.sample.as_deref()
    }
}

fn read_exact(
    read: &mut impl FnMut(&ObjectRef) -> anyhow::Result<Vec<u8>>,
    reference: &ObjectRef,
) -> Option<Vec<u8>> {
    let bytes = read(reference).ok()?;
    (bytes.len() as u64 == reference.bytes && digest(&bytes) == reference.sha256).then_some(bytes)
}

fn single_chain(history: &[Envelope]) -> Result<(&Envelope, &Envelope)> {
    let records: BTreeMap<_, _> = history.iter().map(|e| (e.revision_id, e)).collect();
    if records.len() != history.len() {
        return Err(Failure::UnsupportedRecord);
    }
    let parents: BTreeSet<_> = history
        .iter()
        .flat_map(|e| e.parents.iter().copied())
        .collect();
    let mut heads = history.iter().filter(|e| !parents.contains(&e.revision_id));
    let head = heads.next().ok_or(Failure::UnsupportedRecord)?;
    if heads.next().is_some() {
        return Err(Failure::UnsupportedRecord);
    }
    let mut current = head;
    for depth in 1..=history.len() {
        let Some(parent) = current.parents.first() else {
            return if depth == history.len() {
                Ok((head, current))
            } else {
                Err(Failure::UnsupportedRecord)
            };
        };
        current = records.get(parent).ok_or(Failure::UnsupportedRecord)?;
    }
    Err(Failure::UnsupportedRecord)
}

/// A typed request's exact candidate. It is neither a Store receipt nor a permit.
pub struct Mutation {
    owner: Owner,
    envelope: Envelope,
    correction: Option<Correction>,
    sample: Option<Vec<u8>>,
    fingerprint: String,
}
impl Mutation {
    pub fn prepare(owner: Owner, actor: Uuid, input: ValidatedInput) -> Result<Self> {
        let (action, operation, expected) = input.mutation()?;
        if actor.is_nil()
            || input.request.profile() != owner.profile
            || input.request.record() != Some(owner.record)
        {
            return Err(Failure::InvalidIdentity);
        }
        if expected.len() > 1 {
            return Err(Failure::HeadsConflict);
        }
        let (fingerprint, envelope) = mutation_envelope(
            action,
            operation,
            owner.record,
            expected,
            input.correction.as_ref(),
            actor,
        )?;
        Ok(Self {
            owner,
            envelope,
            correction: input.correction,
            sample: input.sample,
            fingerprint,
        })
    }

    /// Check only after original-operation reconciliation. The caller holds the
    /// shared admission and must atomically recheck it through publication.
    /// None means independently established current scope absence.
    pub fn check_predecessor(&self, current: Option<&Projection>) -> Result<()> {
        let Some(current) = current else {
            return if self.envelope.parents.is_empty() {
                Ok(())
            } else {
                Err(Failure::HeadsConflict)
            };
        };
        if self.owner != current.owner {
            return Err(Failure::InvalidIdentity);
        }
        if self.envelope.parents != BTreeSet::from([current.head]) {
            return Err(Failure::HeadsConflict);
        }
        if self
            .correction
            .as_ref()
            .is_some_and(|value| !current.original.same_original(value))
        {
            return Err(Failure::ImmutableChange);
        }
        Ok(())
    }

    pub fn envelope(&self) -> &Envelope {
        &self.envelope
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn into_parts(self) -> (Envelope, BTreeMap<String, Vec<u8>>) {
        let mut media = BTreeMap::new();
        if let Some((value, bytes)) = self.correction.zip(self.sample) {
            media.insert(value.sample.sha256, bytes);
        }
        (self.envelope, media)
    }
}

#[cfg(test)]
mod tests;

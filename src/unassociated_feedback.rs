//! Immutable local observations, never a display queue or operation receipt.
use crate::{
    conversation::AttemptReason,
    conversation_feedback::{describe_reason, FeedbackKind},
    storage::{self, Envelope, Kind, LocalDomainLease, Namespace, Store, StorePaths, Uuid, FORMAT},
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path, sync::Arc};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub schema: u32,
    pub actor: Uuid,
    pub operation: Uuid,
    /// Caller-supplied immutable observation time, not a current-state claim.
    pub observed_at: u64,
    pub reason: AttemptReason,
    pub reconciliation: bool,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Description {
    pub record: Uuid,
    pub actor: Uuid,
    pub operation: Uuid,
    pub observed_at: u64,
    pub kind: FeedbackKind,
    pub code: &'static str,
    pub text: &'static str,
}
impl Observation {
    /// Serialize an immutable observation; this grants no Store admission.
    pub fn envelope(&self) -> Result<Envelope> {
        ensure!(
            self.schema == 1 && !self.actor.is_nil() && !self.operation.is_nil(),
            "invalid feedback identity/schema"
        );
        let record = Uuid::new_v5(
            &self.actor,
            format!("unassociated-feedback-v1:{}", self.operation).as_bytes(),
        );
        Ok(Envelope {
            envelope_version: FORMAT,
            namespace: Namespace::UnassociatedFeedback,
            domain_schema_version: 1,
            record_id: record,
            revision_id: Uuid::new_v5(&record, b"revision-v1"),
            parents: BTreeSet::new(),
            operation_id: Uuid::new_v5(&record, b"publication-v1"),
            actor_id: self.actor,
            kind: Kind::Value,
            payload: serde_json::to_value(self)?,
            media_descriptors: Vec::new(),
        })
    }
    pub(crate) fn description(&self) -> Result<Description> {
        let envelope = self.envelope()?;
        let (kind, code, text) = describe_reason(&self.reason, self.reconciliation);
        Ok(Description {
            record: envelope.record_id,
            actor: self.actor,
            operation: self.operation,
            observed_at: self.observed_at,
            kind,
            code,
            text,
        })
    }
}
pub(crate) fn validate_envelope(envelope: &Envelope, actor: Uuid) -> Result<Observation> {
    let observation: Observation = serde_json::from_value(envelope.payload.clone())
        .map_err(|_| anyhow::anyhow!("unsupported feedback payload"))?;
    ensure!(
        observation.actor == actor && envelope == &observation.envelope()?,
        "unsupported feedback identity/content"
    );
    Ok(observation)
}
/// Preparation owns a fresh Store and its exclusive lease before Arc handles escape.
/// Existing Store::open never activates this feature. No runtime hook is installed.
pub struct Feedback {
    store: Arc<Store>,
    _local: LocalDomainLease,
}
impl Feedback {
    pub fn prepare(paths: StorePaths) -> Result<Self> {
        let mut store = Store::open_for_feedback(paths)?;
        storage::feedback_generation::prepare(&mut store, |_| Ok(()))?;
        let store = Arc::new(store);
        let local = store.local_domain_lease()?;
        Ok(Self {
            store,
            _local: local,
        })
    }
    pub fn actor(&self) -> Uuid {
        self.store.actor_id
    }
    /// Generic Store access grants no bypass of feature, identity or capacity checks.
    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }
    pub fn retain(
        &self,
        operation: Uuid,
        observed_at: u64,
        reason: AttemptReason,
        reconciliation: bool,
    ) -> Result<Description> {
        self.store.retain_feedback(Observation {
            schema: 1,
            actor: self.actor(),
            operation,
            observed_at,
            reason,
            reconciliation,
        })
    }
    pub fn snapshot(&self) -> Result<Vec<Description>> {
        self.store.feedback_snapshot()
    }
    pub fn export(&self, destination: &Path) -> Result<bool> {
        self.store.export_feedback(destination)
    }
    pub fn restore_additive(&self, source: &Path) -> Result<()> {
        self.store.restore_feedback(source)
    }
}

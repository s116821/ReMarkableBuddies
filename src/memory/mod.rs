//! Explicit associative facts over the existing Store. No automatic extraction or sync engine.
pub mod context;

use crate::storage::{Envelope, Kind, Namespace, Store, Uuid, MAX_ITEMS};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub const MAX_CONTEXT: usize = 16_384;
pub const MAX_MATCHES: usize = 16;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Configuration {
    pub enabled: bool,
    pub profile: Option<Uuid>,
}
impl Configuration {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.profile.is_none_or(|id| !id.is_nil()),
            "invalid memory profile"
        );
        ensure!(
            !self.enabled || self.profile.is_some(),
            "enabled memory requires an explicit profile"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Scope {
    Global,
    Document { document: Uuid },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Key {
    pub profile: Uuid,
    pub scope: Scope,
    pub fact: Uuid,
}
impl Key {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.profile.is_nil() && !self.fact.is_nil(),
            "invalid memory identity"
        );
        if let Scope::Document { document } = self.scope {
            ensure!(!document.is_nil(), "invalid memory document");
        }
        Ok(())
    }
    pub fn record_id(&self) -> Result<Uuid> {
        self.validate()?;
        let identity = format!(
            "urn:remarkable-buddies:subject-memory:v1:{}",
            serde_json::to_string(self)?
        );
        Ok(Uuid::new_v5(&Uuid::NAMESPACE_URL, identity.as_bytes()))
    }
}

/// An explicit assertion by the author; UUID links are not resolved source authority.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub basis: Basis,
    pub description: String,
    pub conversation: Option<Uuid>,
    pub turn: Option<Uuid>,
    pub source: Option<Uuid>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Basis {
    UserConfirmed,
    SourceAssertion,
    Inferred,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Fact {
    pub key: Key,
    pub claim: String,
    pub value: String,
    pub subject: String,
    pub tags: Vec<String>,
    pub provenance: Provenance,
    pub confidence_basis_points: u16,
    pub observed_ms: u64,
    pub expires_ms: Option<u64>,
}
fn text(value: &str, bound: usize) -> bool {
    !value.trim().is_empty() && value.len() <= bound && !value.contains('\0')
}
pub fn canonical_claim(value: &str) -> Result<String> {
    ensure!(text(value, 128), "invalid memory claim");
    let claim = value
        .split_ascii_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    ensure!(
        !claim.is_empty() && !claim.chars().any(char::is_control),
        "invalid memory claim"
    );
    Ok(claim)
}
impl Fact {
    pub fn validate(&self) -> Result<()> {
        self.key.validate()?;
        ensure!(
            canonical_claim(&self.claim)? == self.claim,
            "memory claim must be canonical"
        );
        ensure!(
            text(&self.value, 4096) && text(&self.subject, 128),
            "invalid memory text"
        );
        ensure!(
            self.tags.len() <= 16 && self.tags.iter().all(|s| text(s, 64)),
            "invalid memory tags"
        );
        ensure!(
            text(&self.provenance.description, 512),
            "invalid memory provenance"
        );
        ensure!(
            [
                self.provenance.conversation,
                self.provenance.turn,
                self.provenance.source
            ]
            .iter()
            .flatten()
            .all(|id| !id.is_nil()),
            "invalid memory provenance reference"
        );
        ensure!(
            self.provenance.turn.is_none() || self.provenance.conversation.is_some(),
            "turn provenance requires conversation"
        );
        ensure!(
            self.confidence_basis_points <= 10_000,
            "invalid memory confidence"
        );
        ensure!(
            self.expires_ms.is_none_or(|t| t > self.observed_ms),
            "invalid memory expiry"
        );
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Mutation {
    pub operation: Uuid,
    pub expected: BTreeSet<Uuid>,
    pub fact: Fact,
}
#[derive(Clone, Debug)]
pub struct Forget {
    pub operation: Uuid,
    pub expected: BTreeSet<Uuid>,
    pub key: Key,
}
#[derive(Clone, Debug)]
pub struct Query {
    pub document: Option<Uuid>,
    pub text: String,
    pub now_ms: u64,
    pub current_claims: Vec<String>,
    pub max_matches: usize,
    pub max_context_bytes: usize,
}
impl Query {
    pub fn new(text: String, now_ms: u64) -> Self {
        Self {
            document: None,
            text,
            now_ms,
            current_claims: vec![],
            max_matches: 8,
            max_context_bytes: 8192,
        }
    }
    fn validate(&self) -> Result<()> {
        ensure!(text(&self.text, 2048), "invalid memory query");
        ensure!(
            self.document.is_none_or(|id| !id.is_nil()),
            "invalid query document"
        );
        ensure!(
            (1..=MAX_MATCHES).contains(&self.max_matches)
                && (256..=MAX_CONTEXT).contains(&self.max_context_bytes),
            "invalid memory query budget"
        );
        ensure!(
            self.current_claims.len() <= 64,
            "too many current memory claims"
        );
        for claim in &self.current_claims {
            ensure!(
                canonical_claim(claim)? == *claim,
                "current claim must be canonical"
            );
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Disabled,
    CompleteEmpty,
    Complete,
    Partial,
    Unavailable,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Diagnostics {
    pub conflicted_keys: usize,
    pub contradicted_claims: usize,
    pub unsupported: usize,
    pub expired: usize,
    pub suppressed: usize,
    pub budget_omitted: usize,
    pub snapshot_unavailable: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Match {
    pub record: Uuid,
    pub revision: Uuid,
    pub fact: Fact,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextData {
    pub kind: String,
    pub provenance_authority: String,
    pub facts: Vec<Match>,
}
impl ContextData {
    fn empty() -> Self {
        Self {
            kind: "associative-memory-data".into(),
            provenance_authority: "author-asserted-not-source-verified".into(),
            facts: vec![],
        }
    }
}
#[derive(Clone, Debug)]
pub struct QueryResult {
    pub status: Status,
    pub diagnostics: Diagnostics,
    pub context: ContextData,
}
impl QueryResult {
    pub fn encoded(&self) -> Result<String> {
        Ok(format!(
            "ASSOCIATIVE_MEMORY: {}",
            serde_json::to_string(&self.context)?
        ))
    }
    fn empty(status: Status) -> Self {
        Self {
            status,
            diagnostics: Diagnostics::default(),
            context: ContextData::empty(),
        }
    }
}

/// Domain boundary shared by receiving buddies; cloud/selected adapters must preserve this contract.
pub trait Provider: Send + Sync {
    fn put(&self, mutation: Mutation) -> Result<Uuid>;
    fn forget(&self, mutation: Forget) -> Result<Uuid>;
    fn inspect(&self, key: &Key) -> Result<Vec<Envelope>>;
    fn query(&self, query: &Query) -> Result<QueryResult>;
}
pub struct LocalMemory {
    store: Arc<Store>,
    profile: Uuid,
    _local_only_lease: crate::storage::LegacySyncLease,
}
impl LocalMemory {
    /// Explicit local-only mode. Never substitutes for accepted selected-sync membership.
    pub fn new_local(store: Arc<Store>, profile: Uuid) -> Result<Self> {
        ensure!(!profile.is_nil(), "invalid memory profile");
        let lease = store.legacy_sync_lease()?;
        Ok(Self {
            store,
            profile,
            _local_only_lease: lease,
        })
    }
    fn own(&self, key: &Key) -> Result<()> {
        key.validate()?;
        ensure!(key.profile == self.profile, "foreign memory profile");
        Ok(())
    }
    fn available(&self) -> Result<()> {
        ensure!(
            self.store.unavailable_commits()? == 0,
            "subject memory store contains unavailable transactions"
        );
        Ok(())
    }
    fn decode(&self, envelope: &Envelope) -> Result<Fact> {
        envelope.validate()?;
        ensure!(
            envelope.namespace == Namespace::SubjectMemory
                && envelope.domain_schema_version == 1
                && envelope.kind == Kind::Value
                && envelope.media_descriptors.is_empty(),
            "unsupported memory envelope"
        );
        let fact: Fact = serde_json::from_value(envelope.payload.clone())
            .map_err(|_| anyhow::anyhow!("unsupported memory payload"))?;
        fact.validate()?;
        self.own(&fact.key)?;
        ensure!(
            fact.key.record_id()? == envelope.record_id,
            "memory identity mismatch"
        );
        Ok(fact)
    }
    fn envelope(
        &self,
        key: &Key,
        operation: Uuid,
        expected: BTreeSet<Uuid>,
        fact: Option<Fact>,
    ) -> Result<Envelope> {
        self.own(key)?;
        ensure!(
            !operation.is_nil()
                && expected.len() <= MAX_ITEMS
                && expected.iter().all(|id| !id.is_nil()),
            "invalid memory mutation"
        );
        let envelope = Envelope {
            envelope_version: 1,
            namespace: Namespace::SubjectMemory,
            domain_schema_version: 1,
            record_id: key.record_id()?,
            revision_id: Uuid::new_v5(&operation, b"remarkable-buddies:subject-memory:revision:v1"),
            parents: expected,
            operation_id: operation,
            actor_id: self.store.actor_id,
            kind: if fact.is_some() {
                Kind::Value
            } else {
                Kind::Tombstone
            },
            payload: if let Some(fact) = fact {
                serde_json::to_value(fact)?
            } else {
                serde_json::Value::Null
            },
            media_descriptors: vec![],
        };
        envelope.validate()?;
        Ok(envelope)
    }
    fn replay(&self, envelope: &Envelope) -> Result<Option<Uuid>> {
        let prior = self.store.snapshot_revisions_matching(
            &[Namespace::SubjectMemory],
            MAX_ITEMS,
            |e| e.operation_id == envelope.operation_id,
        )?;
        if let Some(prior) = prior.first() {
            ensure!(prior == envelope, "memory operation identity collision");
            return Ok(Some(
                self.store.commit(vec![envelope.clone()], BTreeMap::new())?,
            ));
        }
        Ok(None)
    }
}
fn tokens(value: &str) -> BTreeSet<String> {
    value
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect()
}
impl Provider for LocalMemory {
    fn put(&self, mutation: Mutation) -> Result<Uuid> {
        self.available()?;
        mutation.fact.validate()?;
        let key = mutation.fact.key.clone();
        let envelope = self.envelope(
            &key,
            mutation.operation,
            mutation.expected,
            Some(mutation.fact.clone()),
        )?;
        if let Some(receipt) = self.replay(&envelope)? {
            return Ok(receipt);
        }
        let history = self.store.snapshot_revisions_matching(
            &[Namespace::SubjectMemory],
            MAX_ITEMS,
            |e| e.record_id == envelope.record_id,
        )?;
        for prior in history.iter().filter(|e| e.kind == Kind::Value) {
            let prior = self.decode(prior)?;
            ensure!(
                prior.key == key && prior.claim == mutation.fact.claim,
                "memory immutable identity or claim changed"
            );
        }
        self.store.commit(vec![envelope], BTreeMap::new())
    }
    fn forget(&self, mutation: Forget) -> Result<Uuid> {
        self.available()?;
        let envelope = self.envelope(&mutation.key, mutation.operation, mutation.expected, None)?;
        if let Some(receipt) = self.replay(&envelope)? {
            return Ok(receipt);
        }
        ensure!(
            !envelope.parents.is_empty(),
            "forget requires explicit current heads"
        );
        self.store.commit(vec![envelope], BTreeMap::new())
    }
    fn inspect(&self, key: &Key) -> Result<Vec<Envelope>> {
        self.available()?;
        self.own(key)?;
        let record = key.record_id()?;
        self.store
            .snapshot_heads_matching(&[Namespace::SubjectMemory], MAX_ITEMS, |e| {
                e.record_id == record
            })
    }
    fn query(&self, query: &Query) -> Result<QueryResult> {
        query.validate()?;
        let mut result = QueryResult::empty(Status::CompleteEmpty);
        if self.available().is_err() {
            result.status = Status::Unavailable;
            result.diagnostics.snapshot_unavailable = true;
            return Ok(result);
        }
        // Include every competing head/tombstone in the global bound before scope filtering.
        let heads =
            match self
                .store
                .snapshot_heads_matching(&[Namespace::SubjectMemory], MAX_ITEMS, |_| true)
            {
                Ok(heads) => heads,
                Err(_) => {
                    result.status = Status::Unavailable;
                    result.diagnostics.snapshot_unavailable = true;
                    return Ok(result);
                }
            };
        let mut by_key: BTreeMap<Uuid, Vec<Envelope>> = BTreeMap::new();
        for envelope in heads {
            by_key.entry(envelope.record_id).or_default().push(envelope);
        }
        let mut by_claim: BTreeMap<String, Vec<Match>> = BTreeMap::new();
        let mut blocked_claims = BTreeSet::new();
        for (record, envelopes) in by_key {
            let values: Vec<_> = envelopes.iter().filter(|e| e.kind == Kind::Value).collect();
            let outside = !values.is_empty() && values.iter().all(|e| {
                let Ok(fact) = serde_json::from_value::<Fact>(e.payload.clone()) else { return false; };
                fact.validate().is_ok() && fact.key.record_id().is_ok_and(|id| id == record)
                    && (fact.key.profile != self.profile || matches!(fact.key.scope, Scope::Document { document } if Some(document) != query.document))
            });
            if outside {
                continue;
            }
            if envelopes.len() > 1 {
                result.diagnostics.conflicted_keys += 1;
                for envelope in &envelopes {
                    if let Ok(fact) = self.decode(envelope) {
                        if matches!(fact.key.scope, Scope::Global)
                            || matches!(fact.key.scope, Scope::Document { document } if Some(document) == query.document)
                        {
                            blocked_claims.insert(fact.claim);
                        }
                    }
                }
                continue;
            }
            let envelope = &envelopes[0];
            if envelope.kind == Kind::Tombstone {
                continue;
            }
            // A valid foreign profile is not an unsupported record in this profile's view.
            let foreign = envelope
                .payload
                .get("key")
                .and_then(|v| v.get("profile"))
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok())
                .is_some_and(|id| id != self.profile);
            if foreign {
                continue;
            }
            let fact = match self.decode(envelope) {
                Ok(fact) => fact,
                Err(_) => {
                    result.diagnostics.unsupported += 1;
                    continue;
                }
            };
            if !matches!(fact.key.scope, Scope::Global)
                && !matches!(fact.key.scope, Scope::Document { document } if Some(document) == query.document)
            {
                continue;
            }
            if fact.expires_ms.is_some_and(|expiry| expiry <= query.now_ms) {
                result.diagnostics.expired += 1;
                continue;
            }
            if query.current_claims.contains(&fact.claim) {
                result.diagnostics.suppressed += 1;
                continue;
            }
            by_claim.entry(fact.claim.clone()).or_default().push(Match {
                record,
                revision: envelope.revision_id,
                fact,
            });
        }
        let terms = tokens(&query.text);
        let mut candidates = vec![];
        for (claim, matches) in by_claim {
            if blocked_claims.contains(&claim) {
                result.diagnostics.contradicted_claims += 1;
                continue;
            }
            if matches
                .iter()
                .map(|m| &m.fact.value)
                .collect::<BTreeSet<_>>()
                .len()
                > 1
            {
                result.diagnostics.contradicted_claims += 1;
                continue;
            }
            let mut ranked: Vec<_> = matches
                .into_iter()
                .filter_map(|item| {
                    let mut words = tokens(&item.fact.value);
                    words.extend(tokens(&item.fact.claim));
                    words.extend(tokens(&item.fact.subject));
                    for tag in &item.fact.tags {
                        words.extend(tokens(tag));
                    }
                    let overlap = terms.intersection(&words).count();
                    (overlap > 0).then_some((overlap, item))
                })
                .collect();
            ranked.sort_by(|a, b| {
                b.0.cmp(&a.0)
                    .then_with(|| {
                        b.1.fact
                            .confidence_basis_points
                            .cmp(&a.1.fact.confidence_basis_points)
                    })
                    .then_with(|| a.1.record.cmp(&b.1.record))
            });
            if let Some(item) = ranked.into_iter().next() {
                candidates.push(item);
            }
        }
        candidates.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| {
                    b.1.fact
                        .confidence_basis_points
                        .cmp(&a.1.fact.confidence_basis_points)
                })
                .then_with(|| a.1.record.cmp(&b.1.record))
        });
        for (_, item) in candidates {
            if result.context.facts.len() >= query.max_matches {
                result.diagnostics.budget_omitted += 1;
                continue;
            }
            result.context.facts.push(item);
            if result.encoded()?.len() > query.max_context_bytes {
                result.context.facts.pop();
                result.diagnostics.budget_omitted += 1;
            }
        }
        ensure!(
            result.encoded()?.len() <= query.max_context_bytes,
            "memory context header exceeds budget"
        );
        let incomplete = result.diagnostics.conflicted_keys
            + result.diagnostics.contradicted_claims
            + result.diagnostics.unsupported
            + result.diagnostics.budget_omitted
            > 0;
        result.status = if incomplete {
            Status::Partial
        } else if result.context.facts.is_empty() {
            Status::CompleteEmpty
        } else {
            Status::Complete
        };
        Ok(result)
    }
}

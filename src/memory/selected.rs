//! Receiving selected memory over existing storage and admission, never a sync engine.
use super::{
    aggregate::Aggregate,
    catalog::{Catalog, Identity, Owner, Record, Registration, MAX_AGGREGATES},
    fact_envelope, query_heads, Forget, Key, Mutation, Provider, Query, QueryResult, Status,
};
use crate::{
    conversation::SelectedAdmission,
    storage::{
        digest,
        selection::{SelectedReadBudget, SelectedReadSnapshot, SelectionToken},
        Envelope, Namespace, ObjectRef, Store, Uuid, MAX_ITEMS,
    },
};
use anyhow::{ensure, Context, Result};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

fn budget() -> SelectedReadBudget {
    SelectedReadBudget::new(64, 4096, 8 * 1024 * 1024, 32 * 1024 * 1024)
}
#[derive(Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitializeAggregate {
    pub operation: Uuid,
    pub accepted_base_sha256: String,
    pub fact: super::Fact,
}
#[derive(Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitializeSelection {
    pub operation: Uuid,
    pub accepted_base_sha256: String,
    pub registrations: Vec<Registration>,
}
#[derive(Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterSelection {
    pub operation: Uuid,
    pub registration: Registration,
}
#[derive(serde::Serialize)]
pub struct SetupReceipt {
    pub transaction: Option<Uuid>,
    pub acknowledgment: &'static str,
    pub registration: Option<Registration>,
}
fn setup_identity(operation: Uuid, accepted_base: &str) -> Result<()> {
    ensure!(
        !operation.is_nil() && crate::storage::valid_digest(accepted_base),
        "invalid selected initializer"
    );
    Ok(())
}
impl InitializeAggregate {
    pub fn validate(&self, identity: &Identity) -> Result<()> {
        identity.validate()?;
        setup_identity(self.operation, &self.accepted_base_sha256)?;
        self.fact.validate()?;
        ensure!(
            self.fact.key.profile == identity.profile,
            "foreign initializer profile"
        );
        Ok(())
    }
}
impl InitializeSelection {
    pub fn validate(&self, identity: &Identity) -> Result<()> {
        identity.validate()?;
        setup_identity(self.operation, &self.accepted_base_sha256)?;
        ensure!(
            self.registrations.len() <= MAX_AGGREGATES,
            "excessive registrations"
        );
        let mut keys = std::collections::BTreeSet::new();
        for registration in &self.registrations {
            registration.validate(identity)?;
            ensure!(
                keys.insert(&registration.key_sha256),
                "duplicate registration"
            );
        }
        Ok(())
    }
}
impl RegisterSelection {
    pub fn validate(&self, identity: &Identity) -> Result<()> {
        identity.validate()?;
        ensure!(!self.operation.is_nil(), "invalid registration operation");
        self.registration.validate(identity)
    }
}
pub struct SelectedMemory {
    store: Arc<Store>,
    identity: Identity,
    catalog: SelectedAdmission,
    instance: Uuid,
}
impl SelectedMemory {
    pub fn initialize_aggregate(&self, input: InitializeAggregate) -> Result<SetupReceipt> {
        input.validate(&self.identity)?;
        let owner = match input.fact.key.scope {
            super::Scope::Global => Owner::GlobalFact {
                key: input.fact.key.clone(),
            },
            super::Scope::Document { document } => Owner::Document { document },
        };
        let scope = self.identity.aggregate_scope(&owner)?;
        let admission = SelectedAdmission::new(self.store.clone(), scope.clone())?;
        let mut allowance = budget();
        SelectedAdmission::with_gates(&[&admission], |store| {
            available(store)?;
            let prior = store.selected_snapshot_with_budget(&scope, MAX_ITEMS, &mut allowance)?;
            if prior.is_none() && matches!(owner, Owner::Document { .. }) {
                ensure!(
                    store
                        .snapshot_heads_matching(
                            &[
                                Namespace::Conversation,
                                Namespace::Source,
                                Namespace::ExportAssociation
                            ],
                            MAX_ITEMS,
                            |_| true
                        )?
                        .is_empty(),
                    "legacy document ownership must be explicitly resolved before initialization"
                );
            }
            let envelope = fact_envelope(
                store,
                self.identity.profile,
                &input.fact.key,
                input.operation,
                Default::default(),
                Some(input.fact.clone()),
            )?;
            let (manifest, objects) = initial_manifest(input.operation, b"aggregate", &[envelope])?;
            let publication = store.initialize_selected(
                &scope,
                crate::storage::selection::SelectionChange {
                    operation: input.operation,
                    accepted_base_sha256: input.accepted_base_sha256,
                    selected: manifest,
                    retained: vec![],
                },
                objects,
            )?;
            let raw_genesis = if publication.replayed {
                prior
                    .as_ref()
                    .context("replay lineage absent")?
                    .accepted_history
                    .iter()
                    .find(|item| item.transaction.operation == input.operation)
                    .context("replay genesis absent")?
                    .sha256
                    .clone()
            } else {
                publication.token.selection_sha256().to_owned()
            };
            Ok(SetupReceipt {
                transaction: Some(publication.transaction.selected.transaction_id),
                acknowledgment: if publication.replayed {
                    "replayed"
                } else {
                    "published"
                },
                registration: Some(Registration {
                    format: 1,
                    catalog: self.identity.record_id()?,
                    key_sha256: scope.key_sha256,
                    owner,
                    genesis_operation: input.operation,
                    genesis_sha256: raw_genesis,
                }),
            })
        })
    }
    pub fn initialize_selection(&self, input: InitializeSelection) -> Result<SetupReceipt> {
        input.validate(&self.identity)?;
        let mut admissions = vec![SelectedAdmission::new(
            self.store.clone(),
            self.identity.scope()?,
        )?];
        for registration in &input.registrations {
            admissions.push(SelectedAdmission::new(
                self.store.clone(),
                self.identity.aggregate_scope(&registration.owner)?,
            )?);
        }
        let handles: Vec<_> = admissions.iter().collect();
        let mut allowance = budget();
        SelectedAdmission::with_gates(&handles, |store| {
            available(store)?;
            if let Some(prior) = store.selected_snapshot_with_budget(
                &self.identity.scope()?,
                MAX_ITEMS,
                &mut allowance,
            )? {
                Catalog::project(&self.identity, &prior.snapshot, &prior.accepted_history)?;
            }
            let mut envelopes = vec![Record::root(&self.identity)?.envelope(
                &self.identity,
                store.actor_id,
                input.operation,
            )?];
            let mut registrations = input.registrations.clone();
            registrations.sort_by(|a, b| a.key_sha256.cmp(&b.key_sha256));
            for registration in registrations {
                let scope = self.identity.aggregate_scope(&registration.owner)?;
                let read = store
                    .selected_snapshot_with_budget(&scope, MAX_ITEMS, &mut allowance)?
                    .context("initializer aggregate absent")?;
                Aggregate::project(
                    &self.identity,
                    &registration,
                    &read.snapshot,
                    &read.accepted_history,
                )?;
                let operation = Uuid::new_v5(
                    &input.operation,
                    format!("registration:{}", registration.key_sha256).as_bytes(),
                );
                envelopes.push(Record::registration(registration).envelope(
                    &self.identity,
                    store.actor_id,
                    operation,
                )?);
            }
            let (manifest, objects) = initial_manifest(input.operation, b"catalog", &envelopes)?;
            let publication = store.initialize_selected(
                &self.identity.scope()?,
                crate::storage::selection::SelectionChange {
                    operation: input.operation,
                    accepted_base_sha256: input.accepted_base_sha256,
                    selected: manifest,
                    retained: vec![],
                },
                objects,
            )?;
            Ok(SetupReceipt {
                transaction: Some(publication.transaction.selected.transaction_id),
                acknowledgment: if publication.replayed {
                    "replayed"
                } else {
                    "published"
                },
                registration: None,
            })
        })
    }
    pub fn register_selection(&self, input: RegisterSelection) -> Result<SetupReceipt> {
        input.validate(&self.identity)?;
        let mut view = self.read_extra(Some(&input.registration))?;
        let token = view.catalog_read.snapshot.token.clone();
        let lineage: Vec<_> = view
            .catalog_read
            .accepted_history
            .iter()
            .map(|item| item.transaction.clone())
            .collect();
        let envelope = Record::registration(input.registration.clone()).envelope(
            &self.identity,
            self.store.actor_id,
            input.operation,
        )?;
        let adopted = view
            .registrations
            .contains_key(&input.registration.key_sha256);
        view.pins.guard(&self.store, || {
            if let Some(transaction) = original_receipt(&lineage, &envelope)? {
                return Ok(SetupReceipt {
                    transaction: Some(transaction),
                    acknowledgment: "replayed",
                    registration: None,
                });
            }
            if adopted {
                return Ok(SetupReceipt {
                    transaction: None,
                    acknowledgment: "adopted",
                    registration: Some(input.registration),
                });
            }
            let publication = self.store.commit_selected(
                &token,
                input.operation,
                vec![envelope],
                BTreeMap::new(),
            )?;
            Ok(SetupReceipt {
                transaction: Some(publication.transaction.selected.transaction_id),
                acknowledgment: "published",
                registration: None,
            })
        })
    }
    pub fn new(store: Arc<Store>, identity: Identity) -> Result<Self> {
        identity.validate()?;
        let catalog = SelectedAdmission::new(store.clone(), identity.scope()?)?;
        Ok(Self {
            store,
            identity,
            catalog,
            instance: Uuid::new_v4(),
        })
    }
    fn own(&self, key: &Key) -> Result<()> {
        key.validate()?;
        ensure!(
            key.profile == self.identity.profile,
            "foreign memory profile"
        );
        Ok(())
    }
    fn read(&self) -> Result<View> {
        self.read_extra(None)
    }
    fn read_extra(&self, extra: Option<&Registration>) -> Result<View> {
        let mut budget = budget();
        let catalog = SelectedAdmission::with_gates(&[&self.catalog], |store| {
            available(store)?;
            let read = store
                .selected_snapshot_with_budget(&self.identity.scope()?, MAX_ITEMS, &mut budget)?
                .context("memory catalog absent")?;
            let catalog = Catalog::project(&self.identity, &read.snapshot, &read.accepted_history)?;
            Ok((
                read.snapshot.token,
                catalog,
                read.accepted_history[0].sha256.clone(),
            ))
        })?;
        let mut declared = catalog.1.registrations.clone();
        if let Some(extra) = extra {
            extra.validate(&self.identity)?;
            if let Some(prior) = declared.get(&extra.key_sha256) {
                ensure!(prior == extra, "registration owner or genesis collision");
            } else {
                declared.insert(extra.key_sha256.clone(), extra.clone());
            }
        }
        ensure!(
            declared.len() <= MAX_AGGREGATES,
            "excessive aggregate count"
        );
        let mut admissions = vec![SelectedAdmission::new(
            self.store.clone(),
            self.identity.scope()?,
        )?];
        for registration in declared.values() {
            admissions.push(SelectedAdmission::new(
                self.store.clone(),
                self.identity.aggregate_scope(&registration.owner)?,
            )?);
        }
        let handles: Vec<_> = admissions.iter().collect();
        let (tokens, raw_heads, aggregates, reads, catalog_read) =
            SelectedAdmission::with_gates(&handles, |store| {
                available(store)?;
                let fresh = store
                    .selected_snapshot_with_budget(&self.identity.scope()?, MAX_ITEMS, &mut budget)?
                    .context("memory catalog absent")?;
                ensure!(
                    fresh.snapshot.token == catalog.0
                        && fresh.accepted_history[0].sha256 == catalog.2,
                    "memory catalog replaced during discovery"
                );
                Catalog::project(&self.identity, &fresh.snapshot, &fresh.accepted_history)?;
                let mut tokens = vec![fresh.snapshot.token.clone()];
                let mut raw_heads = vec![fresh.accepted_history[0].sha256.clone()];
                let mut aggregates = BTreeMap::new();
                let mut reads = BTreeMap::new();
                for registration in declared.values() {
                    let read = store
                        .selected_snapshot_with_budget(
                            &self.identity.aggregate_scope(&registration.owner)?,
                            MAX_ITEMS,
                            &mut budget,
                        )?
                        .context("registered memory aggregate absent")?;
                    let aggregate = Aggregate::project(
                        &self.identity,
                        registration,
                        &read.snapshot,
                        &read.accepted_history,
                    )?;
                    tokens.push(read.snapshot.token.clone());
                    raw_heads.push(read.accepted_history[0].sha256.clone());
                    aggregates.insert(registration.key_sha256.clone(), aggregate);
                    reads.insert(registration.key_sha256.clone(), read);
                }
                Ok((tokens, raw_heads, aggregates, reads, fresh))
            })?;
        Ok(View {
            pins: Pins {
                instance: self.instance,
                admissions,
                tokens,
                raw_heads,
                budget,
                consumed: false,
                result_sha256: None,
            },
            aggregates,
            reads,
            catalog_read,
            registrations: catalog.1.registrations,
        })
    }
    fn mutation(
        &self,
        key: &Key,
        operation: Uuid,
        expected: std::collections::BTreeSet<Uuid>,
        fact: Option<super::Fact>,
    ) -> Result<Uuid> {
        self.own(key)?;
        let envelope = fact_envelope(
            &self.store,
            self.identity.profile,
            key,
            operation,
            expected,
            fact.clone(),
        )?;
        if fact.is_none() {
            ensure!(
                !envelope.parents.is_empty(),
                "forget requires explicit heads"
            );
        }
        let owner = match key.scope {
            super::Scope::Global => super::catalog::Owner::GlobalFact { key: key.clone() },
            super::Scope::Document { document } => super::catalog::Owner::Document { document },
        };
        let aggregate_key = owner.key_sha256()?;
        let mut view = self.read()?;
        let read = view
            .reads
            .get(&aggregate_key)
            .context("memory aggregate is not registered")?;
        let token = read.snapshot.token.clone();
        let lineage: Vec<_> = read
            .accepted_history
            .iter()
            .map(|item| item.transaction.clone())
            .collect();
        let history = view.aggregates[&aggregate_key]
            .history
            .get(&envelope.record_id)
            .cloned()
            .unwrap_or_default();
        view.pins.guard(&self.store, || {
            if let Some(receipt) = original_receipt(&lineage, &envelope)? {
                return Ok(receipt);
            }
            if let Some(fact) = &fact {
                for prior in history
                    .iter()
                    .filter(|prior| prior.kind == crate::storage::Kind::Value)
                {
                    let prior: super::Fact = serde_json::from_value(prior.payload.clone())?;
                    ensure!(
                        prior.key == *key && prior.claim == fact.claim,
                        "memory immutable key or claim changed"
                    );
                }
            }
            Ok(self
                .store
                .commit_selected(&token, operation, vec![envelope], BTreeMap::new())?
                .transaction
                .selected
                .transaction_id)
        })
    }
}

fn initial_manifest(
    operation: Uuid,
    domain: &[u8],
    envelopes: &[Envelope],
) -> Result<(crate::storage::Manifest, BTreeMap<String, Vec<u8>>)> {
    let mut records = Vec::new();
    let mut objects = BTreeMap::new();
    let mut namespaces = BTreeMap::new();
    for envelope in envelopes {
        let bytes = serde_json::to_vec(envelope)?;
        let sha256 = digest(&bytes);
        records.push(ObjectRef {
            sha256: sha256.clone(),
            bytes: bytes.len() as u64,
        });
        namespaces.insert(sha256.clone(), envelope.namespace);
        objects.insert(sha256, bytes);
    }
    let manifest = crate::storage::Manifest {
        format: 1,
        transaction_id: Uuid::new_v5(&operation, domain),
        scope: crate::storage::Scope::SelectedRecords,
        records,
        record_namespaces: namespaces,
        media: vec![],
        media_coverage: vec![],
    };
    manifest.validate()?;
    Ok((manifest, objects))
}

fn available(store: &Store) -> Result<()> {
    ensure!(
        store.unavailable_commits()? == 0,
        "selected memory has unavailable transactions"
    );
    Ok(())
}
struct View {
    pins: Pins,
    aggregates: BTreeMap<String, Aggregate>,
    reads: BTreeMap<String, SelectedReadSnapshot>,
    catalog_read: SelectedReadSnapshot,
    registrations: BTreeMap<String, Registration>,
}
pub(crate) struct Pins {
    instance: Uuid,
    admissions: Vec<SelectedAdmission>,
    tokens: Vec<SelectionToken>,
    raw_heads: Vec<String>,
    budget: SelectedReadBudget,
    consumed: bool,
    result_sha256: Option<String>,
}
impl std::fmt::Debug for Pins {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectedMemoryPins").finish_non_exhaustive()
    }
}
impl Pins {
    fn guard<T>(&mut self, store: &Store, consume: impl FnOnce() -> Result<T>) -> Result<T> {
        ensure!(!self.consumed, "selected memory result already consumed");
        let handles: Vec<_> = self.admissions.iter().collect();
        SelectedAdmission::with_gates(&handles, |_| {
            available(store)?;
            for (token, raw_head) in self.tokens.iter().zip(&self.raw_heads) {
                let fresh = store
                    .selected_snapshot_with_budget(token.scope(), MAX_ITEMS, &mut self.budget)?
                    .context("selected memory membership absent")?;
                ensure!(
                    fresh.snapshot.token == *token && fresh.accepted_history[0].sha256 == *raw_head,
                    "selected memory membership replaced"
                );
            }
            consume()
        })
    }
}
impl Provider for SelectedMemory {
    fn put(&self, mutation: Mutation) -> Result<Uuid> {
        mutation.fact.validate()?;
        let key = mutation.fact.key.clone();
        self.mutation(
            &key,
            mutation.operation,
            mutation.expected,
            Some(mutation.fact),
        )
    }
    fn forget(&self, mutation: Forget) -> Result<Uuid> {
        self.mutation(&mutation.key, mutation.operation, mutation.expected, None)
    }
    fn inspect(&self, key: &Key) -> Result<Vec<Envelope>> {
        let inspection = self.inspect_for_handoff(key)?;
        inspection.with_current(self, &mut || Ok(()))?;
        Ok(inspection.envelopes)
    }
    fn inspect_for_handoff(&self, key: &Key) -> Result<super::Inspection> {
        self.own(key)?;
        let mut view = self.read()?;
        let heads = view
            .aggregates
            .values()
            .flat_map(|aggregate| &aggregate.heads)
            .filter(|envelope| key.record_id().is_ok_and(|id| id == envelope.record_id))
            .cloned()
            .collect();
        let mut admission = QueryResult::empty(Status::Complete);
        view.pins.result_sha256 = Some(result_identity(&admission)?);
        admission.pins = Some(Arc::new(Mutex::new(view.pins)));
        super::Inspection::new(heads, Some(admission))
    }
    fn query(&self, query: &Query) -> Result<QueryResult> {
        query.validate()?;
        let mut view = match self.read() {
            Ok(view) => view,
            Err(_) => {
                let mut result = QueryResult::empty(Status::Unavailable);
                result.diagnostics.snapshot_unavailable = true;
                return Ok(result);
            }
        };
        let heads = view
            .aggregates
            .values()
            .flat_map(|aggregate| &aggregate.heads)
            .cloned()
            .collect();
        let mut result = query_heads(self.identity.profile, query, heads)?;
        view.pins.result_sha256 = Some(result_identity(&result)?);
        result.pins = Some(Arc::new(Mutex::new(view.pins)));
        Ok(result)
    }
    fn with_current(
        &self,
        result: &QueryResult,
        consume: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        let mut pins = result
            .pins
            .as_ref()
            .context("selected memory pins absent")?
            .lock()
            .map_err(|_| anyhow::anyhow!("selected memory pins unavailable"))?;
        ensure!(
            pins.instance == self.instance,
            "foreign selected memory result"
        );
        ensure!(
            pins.result_sha256.as_deref() == Some(result_identity(result)?.as_str()),
            "selected memory result payload changed"
        );
        let output = pins.guard(&self.store, consume);
        pins.consumed = true;
        output
    }
}

fn result_identity(result: &QueryResult) -> Result<String> {
    Ok(digest(&serde_json::to_vec(&(
        result.status,
        &result.diagnostics,
        &result.context,
    ))?))
}

/// Historical acknowledgment only. No new publication or current token follows.
fn original_receipt(
    lineage: &[crate::storage::selection::SelectionTransaction],
    envelope: &Envelope,
) -> Result<Option<Uuid>> {
    let Some(index) = lineage
        .iter()
        .position(|transaction| transaction.operation == envelope.operation_id)
    else {
        return Ok(None);
    };
    let original = &lineage[index];
    ensure!(
        original.mutation == crate::storage::selection::SelectionMutation::Commit,
        "memory operation collision"
    );
    let previous = lineage
        .get(index + 1)
        .context("memory receipt predecessor absent")?;
    let original_media: BTreeMap<_, _> = original
        .selected
        .media
        .iter()
        .map(|r| (&r.sha256, r))
        .collect();
    let previous_media: BTreeMap<_, _> = previous
        .selected
        .media
        .iter()
        .map(|r| (&r.sha256, r))
        .collect();
    let original_coverage: BTreeMap<_, _> = original
        .selected
        .media_coverage
        .iter()
        .map(|r| (r.hash(), r))
        .collect();
    let previous_coverage: BTreeMap<_, _> = previous
        .selected
        .media_coverage
        .iter()
        .map(|r| (r.hash(), r))
        .collect();
    let encoded = serde_json::to_vec(envelope)?;
    let expected = ObjectRef {
        sha256: digest(&encoded),
        bytes: encoded.len() as u64,
    };
    let old: BTreeMap<_, _> = previous
        .selected
        .records
        .iter()
        .map(|reference| (&reference.sha256, reference))
        .collect();
    let new: Vec<_> = original
        .selected
        .records
        .iter()
        .filter(|reference| !old.contains_key(&reference.sha256))
        .collect();
    let mut namespaces = previous.selected.record_namespaces.clone();
    namespaces.insert(expected.sha256.clone(), Namespace::SubjectMemory);
    ensure!(
        new == vec![&expected]
            && previous
                .selected
                .records
                .iter()
                .all(|reference| original.selected.records.contains(reference))
            && original.selected.record_namespaces.get(&expected.sha256)
                == Some(&Namespace::SubjectMemory)
            && original_media == previous_media
            && original_coverage == previous_coverage
            && original.selected.record_namespaces == namespaces
            && original.retained == previous.retained,
        "memory operation intent collision"
    );
    Ok(Some(original.selected.transaction_id))
}

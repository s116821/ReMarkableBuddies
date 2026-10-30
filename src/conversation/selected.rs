//! Causal projection of one pinned selected closure. No all-history lookup.
use super::{IntentRecordRef, Ledger, Record};
use crate::storage::selection::SelectedSnapshot;
use crate::storage::{Envelope, Kind, Namespace, ObjectRef, Uuid, MAX_ITEMS, MAX_METADATA};
use anyhow::{ensure, Context, Result};
use std::collections::{BTreeMap, BTreeSet};

pub struct SelectedDomainProjection {
    ancestors: BTreeMap<Uuid, Envelope>,
    objects: BTreeMap<Uuid, ObjectRef>,
    records: BTreeMap<Uuid, Record>,
    heads: BTreeMap<(Namespace, Uuid), Uuid>,
}
/// Historical document grouping only; no current-device qualification or token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentOwnership {
    Document(Uuid),
    DeferredMissing,
    DeferredAmbiguous(BTreeSet<Uuid>),
}
impl SelectedDomainProjection {
    fn hashed_record(&self, hash: &str) -> Option<&Record> {
        self.records.iter().find_map(|(revision, record)| {
            (self.objects.get(revision)?.sha256 == hash).then_some(record)
        })
    }
    pub fn document_ownership(&self) -> Result<BTreeMap<Uuid, DocumentOwnership>> {
        let mut owners: BTreeMap<Uuid, BTreeSet<Uuid>> = BTreeMap::new();
        for record in self.records.values() {
            let conversation = conversation_id(record);
            ensure!(
                self.records
                    .values()
                    .any(|r| matches!(r, Record::Root(root) if root.id == conversation)),
                "selected conversation root missing"
            );
            owners.entry(conversation).or_default();
            let mut document = None;
            match record {
                Record::Root(root) => {
                    if let Some(binding) = root.binding {
                        self.require_binding(binding, conversation)?;
                    }
                }
                Record::Turn(turn) => {
                    if let Some(completion) = &turn.completion {
                        document = Some(completion.observation.document);
                    }
                    for source in &turn.sources {
                        ensure!(
                            self.records.values().any(|r| match r {
                                Record::Source(s) =>
                                    s.id == *source
                                        && s.conversation == conversation
                                        && s.turn == turn.id,
                                Record::Capture(s) =>
                                    s.id == *source
                                        && s.conversation == conversation
                                        && s.turn == turn.id,
                                Record::DevelopmentCapture(s) =>
                                    s.id == *source
                                        && s.conversation == conversation
                                        && s.turn == turn.id,
                                Record::LegacyCapture(s) =>
                                    s.id == *source
                                        && s.conversation == conversation
                                        && s.turn == turn.id,
                                Record::NormalizedText(s) =>
                                    s.id == *source
                                        && s.conversation == conversation
                                        && matches!(self.head(Namespace::Conversation, s.turn),
                                            Some((_, Some(Record::Turn(origin))))
                                            if origin.conversation == conversation
                                                && origin.sequence <= turn.sequence
                                                && origin.sources.contains(&s.id)),
                                Record::ArchivedNormalizedSource(s) =>
                                    s.id == *source
                                        && s.conversation == conversation
                                        && matches!(self.head(Namespace::Conversation, s.turn),
                                            Some((_, Some(Record::Turn(origin))))
                                            if origin.conversation == conversation
                                                && origin.sequence <= turn.sequence
                                                && origin.sources.contains(&s.id)),
                                _ => false,
                            }),
                            "selected turn source closure missing or foreign"
                        );
                    }
                    if let Some(prior) = turn.correction_of {
                        ensure!(
                            matches!(self.head(Namespace::Conversation, prior),
                            Some((_, Some(Record::Turn(t))))
                            if t.conversation == conversation && t.sequence < turn.sequence),
                            "selected correction chronology broken"
                        );
                    }
                }
                Record::Source(source) => {
                    self.require_turn(source.turn, conversation)?;
                    document = Some(source.observation.document);
                }
                Record::Capture(capture) => {
                    self.require_turn(capture.turn, conversation)?;
                    document = Some(capture.facts.source.document.value());
                }
                Record::DevelopmentCapture(capture) => {
                    self.require_turn(capture.turn, conversation)?;
                }
                Record::LegacyCapture(capture) => {
                    self.require_turn(capture.turn, conversation)?;
                }
                Record::NormalizedText(source) => {
                    self.require_turn(source.turn, conversation)?;
                    ensure!(
                        matches!(self.head(Namespace::Conversation, source.turn),
                            Some((_, Some(Record::Turn(origin))))
                            if origin.sources.contains(&source.id)),
                        "selected normalized source origin relationship missing"
                    );
                }
                Record::ArchivedNormalizedSource(source) => {
                    self.require_turn(source.turn, conversation)?;
                    ensure!(
                        matches!(self.head(Namespace::Conversation, source.turn),
                        Some((_, Some(Record::Turn(origin)))) if origin.sources.contains(&source.id)),
                        "selected notebook archive origin relationship missing"
                    );
                    // Historical document metadata never grants native ownership.
                }
                Record::NotebookPlan(plan) => {
                    ensure!(
                        matches!(self.hashed_record(&plan.source_revision),
                        Some(Record::ArchivedNormalizedSource(source)) if source.id == plan.archive_id
                        && source.conversation == conversation && source.identity_digest()? == plan.source_identity_digest),
                        "selected notebook plan source revision absent"
                    );
                }
                Record::NotebookStage(stage) => {
                    ensure!(
                        matches!(self.hashed_record(&stage.plan_revision),
                        Some(Record::NotebookPlan(plan)) if plan.id == stage.plan_id
                        && plan.conversation == conversation && plan.core_digest == stage.core_digest
                        && plan.source_revision == stage.source_revision),
                        "selected notebook stage plan revision absent"
                    );
                    for (id, revision, digest) in &stage.predecessors {
                        ensure!(
                            matches!(self.hashed_record(revision), Some(Record::NotebookStage(prior))
                            if prior.id == *id && prior.conversation == conversation
                            && prior.outcome == Some(super::notebook::NotebookStageOutcome::Generated)
                            && prior.observed_output_digest.as_ref() == Some(digest)),
                            "selected notebook predecessor absent"
                        );
                    }
                }
                Record::Binding(binding) => {
                    ensure!(
                        binding.receipt.observed_document == binding.receipt.source.document,
                        "selected binding document mismatch"
                    );
                    document = Some(binding.receipt.source.document);
                }
                Record::Export(_) => {}
                Record::RequestEvidence(_) => {
                    anyhow::bail!("ordinary request evidence excluded from selected projection")
                }
                Record::Receipt(receipt) => {
                    let ack = &receipt.acknowledgment;
                    ensure!(
                        matches!(self.records.get(&ack.applied_root_revision), Some(Record::Root(r)) if r.id == conversation),
                        "selected receipt root revision missing or foreign"
                    );
                    if let Some(binding) = ack.binding {
                        self.require_binding(binding, conversation)?;
                    }
                    for id in &ack.records {
                        ensure!(
                            self.records
                                .iter()
                                .any(|(revision, r)| self.ancestors[revision].record_id == *id
                                    && conversation_id(r) == conversation),
                            "selected receipt record missing or foreign"
                        );
                    }
                    if let Some(intent) = &receipt.admitted_intent {
                        ensure!(
                            matches!(self.exact_reference(&intent.root_revision)?, Record::Root(r) if r.id == conversation),
                            "intent root variant mismatch"
                        );
                        ensure!(
                            matches!(self.exact_reference(&intent.turn_revision)?, Record::Turn(t) if t.id == intent.turn && t.conversation == conversation),
                            "intent turn variant mismatch"
                        );
                        for reference in &intent.evidence {
                            ensure!(
                                conversation_id(self.exact_reference(reference)?) == conversation,
                                "foreign intent evidence"
                            );
                        }
                        document = Some(intent.source.document);
                    }
                }
                Record::OutcomeFact(fact) => {
                    self.require_turn(fact.turn, conversation)?;
                    if let Some(settlement) = &fact.settlement {
                        let Record::Receipt(original) =
                            self.exact_reference(&settlement.original_receipt)?
                        else {
                            anyhow::bail!("settlement receipt variant mismatch");
                        };
                        let intent = original
                            .admitted_intent
                            .as_ref()
                            .context("settlement receipt has no admitted intent")?;
                        ensure!(
                            intent.operation == settlement.operation
                                && intent.conversation == conversation
                                && intent.turn == fact.turn
                                && intent.request_fingerprint == settlement.request_fingerprint
                                && intent.root_revision == settlement.original_root
                                && intent.turn_revision == settlement.original_turn
                                && intent.selection == settlement.original_selection,
                            "settlement differs from admitted intent"
                        );
                        ensure!(
                            matches!(self.exact_reference(&settlement.original_receipt)?, Record::Receipt(r)
                            if r.acknowledgment.operation == settlement.operation && r.acknowledgment.conversation == conversation),
                            "settlement receipt variant mismatch"
                        );
                        ensure!(
                            matches!(self.exact_reference(&settlement.original_root)?, Record::Root(r) if r.id == conversation),
                            "settlement root variant mismatch"
                        );
                        ensure!(
                            matches!(self.exact_reference(&settlement.original_turn)?, Record::Turn(t) if t.id == fact.turn && t.conversation == conversation),
                            "settlement turn variant mismatch"
                        );
                        for reference in &settlement.evidence {
                            ensure!(
                                conversation_id(self.exact_reference(reference)?) == conversation,
                                "foreign settlement evidence"
                            );
                        }
                    }
                }
            }
            if let Some(document) = document {
                ensure!(!document.is_nil(), "selected document identity is nil");
                owners.get_mut(&conversation).unwrap().insert(document);
            }
        }
        // Chronology concerns current causal heads, not older revisions of a turn.
        let mut sequences = BTreeSet::new();
        for revision in self.heads.values() {
            if let Some(Record::Turn(turn)) = self.records.get(revision) {
                ensure!(
                    sequences.insert((turn.conversation, turn.sequence)),
                    "selected turn sequence collision"
                );
                let (_, root) = self
                    .head(Namespace::Conversation, turn.conversation)
                    .context("selected root head absent")?;
                if let Some(Record::Root(root)) = root {
                    ensure!(
                        turn.sequence < root.next_sequence,
                        "selected root chronology broken"
                    );
                } else {
                    ensure!(root.is_none(), "selected root head variant mismatch");
                }
            }
        }
        Ok(owners
            .into_iter()
            .map(|(id, documents)| {
                let owner = match documents.len() {
                    0 => DocumentOwnership::DeferredMissing,
                    1 => DocumentOwnership::Document(*documents.first().unwrap()),
                    _ => DocumentOwnership::DeferredAmbiguous(documents),
                };
                (id, owner)
            })
            .collect())
    }
    fn require_turn(&self, id: Uuid, conversation: Uuid) -> Result<()> {
        ensure!(
            self.records.values().any(
                |r| matches!(r, Record::Turn(t) if t.id == id && t.conversation == conversation)
            ),
            "selected turn reference missing or foreign"
        );
        Ok(())
    }
    fn require_binding(&self, id: Uuid, conversation: Uuid) -> Result<()> {
        ensure!(
            self.records.values().any(
                |r| matches!(r, Record::Binding(b) if b.id == id && b.conversation == conversation)
            ),
            "selected binding reference missing or foreign"
        );
        Ok(())
    }
    /// Retained records deliberately do not enter current projection.
    pub fn from_snapshot(snapshot: &SelectedSnapshot) -> Result<Self> {
        Self::from_mixed_records(
            &snapshot.selected_records,
            &snapshot.transaction.selected.records,
            &snapshot.transaction.scope,
        )
    }
    /// Explicit reciprocal document partition. Domain-only projection below
    /// stays strict; this path validates the complete mixed graph first.
    pub(crate) fn from_mixed_records(
        records: &[Envelope],
        references: &[ObjectRef],
        scope: &crate::storage::selection::SelectionScope,
    ) -> Result<Self> {
        if !records
            .iter()
            .any(|record| record.namespace == Namespace::SubjectMemory)
        {
            return Self::from_pinned_records(records, references);
        }
        ensure!(
            records.len() == references.len() && !records.is_empty() && records.len() <= MAX_ITEMS,
            "invalid mixed selected references"
        );
        let mut all = BTreeMap::new();
        let mut candidates: BTreeMap<(Namespace, Uuid), BTreeSet<Uuid>> = BTreeMap::new();
        let mut bytes = 0usize;
        let mut domain = Vec::new();
        let mut domain_refs = Vec::new();
        let mut memory = Vec::new();
        for (record, reference) in records.iter().zip(references) {
            record.validate()?;
            ensure!(
                crate::storage::valid_digest(&reference.sha256)
                    && reference.bytes > 0
                    && reference.bytes <= crate::storage::MAX_RECORD as u64,
                "invalid mixed object reference"
            );
            bytes = bytes
                .checked_add(usize::try_from(reference.bytes)?)
                .context("mixed metadata overflow")?;
            ensure!(bytes <= MAX_METADATA, "mixed metadata exceeds bound");
            ensure!(
                all.insert(record.revision_id, record).is_none(),
                "duplicate mixed revision"
            );
            candidates
                .entry((record.namespace, record.record_id))
                .or_default()
                .insert(record.revision_id);
            match record.namespace {
                Namespace::SubjectMemory => memory.push(record.clone()),
                Namespace::Conversation | Namespace::Source | Namespace::ExportAssociation => {
                    domain.push(record.clone());
                    domain_refs.push(reference.clone());
                }
                _ => anyhow::bail!("unsupported mixed selected namespace"),
            }
        }
        let mut remaining: BTreeMap<_, usize> = all
            .iter()
            .map(|(id, record)| (*id, record.parents.len()))
            .collect();
        let mut children: BTreeMap<Uuid, Vec<Uuid>> = BTreeMap::new();
        for record in records {
            for parent in &record.parents {
                let prior = all.get(parent).context("mixed ancestor missing")?;
                ensure!(
                    prior.namespace == record.namespace && prior.record_id == record.record_id,
                    "foreign mixed parent"
                );
                candidates
                    .get_mut(&(record.namespace, record.record_id))
                    .unwrap()
                    .remove(parent);
                children
                    .entry(*parent)
                    .or_default()
                    .push(record.revision_id);
            }
        }
        let mut ready: Vec<_> = remaining
            .iter()
            .filter_map(|(id, count)| (*count == 0).then_some(*id))
            .collect();
        let mut processed = 0;
        while let Some(id) = ready.pop() {
            processed += 1;
            for child in children.get(&id).into_iter().flatten() {
                let count = remaining.get_mut(child).unwrap();
                *count -= 1;
                if *count == 0 {
                    ready.push(*child);
                }
            }
        }
        ensure!(processed == records.len(), "mixed causal cycle");
        let mut pending = Vec::new();
        for heads in candidates.values() {
            ensure!(heads.len() == 1, "mixed causal fork or cycle");
            pending.extend(heads.iter().copied());
        }
        let mut reached = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if reached.insert(id) {
                pending.extend(all[&id].parents.iter().copied());
            }
        }
        ensure!(reached.len() == records.len(), "unreachable mixed history");
        let projection = Self::from_pinned_records(&domain, &domain_refs)?;
        let owners = projection.document_ownership()?;
        let Some(DocumentOwnership::Document(document)) = owners.values().next() else {
            anyhow::bail!("mixed document ownership unavailable")
        };
        ensure!(
            owners
                .values()
                .all(|owner| *owner == DocumentOwnership::Document(*document))
                && scope.key_sha256 == crate::memory::catalog::document_key(*document)?,
            "foreign or ambiguous mixed document"
        );
        crate::memory::aggregate::validate_document_partition(&memory, *document)?;
        Ok(projection)
    }
    #[cfg(test)]
    fn from_records(records: &[Envelope]) -> Result<Self> {
        let objects = records
            .iter()
            .map(|record| {
                let bytes = serde_json::to_vec(record)?;
                Ok(ObjectRef {
                    sha256: crate::storage::digest(&bytes),
                    bytes: bytes.len() as u64,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Self::from_pinned_records(records, &objects)
    }
    pub(crate) fn from_pinned_records(
        records: &[Envelope],
        references: &[ObjectRef],
    ) -> Result<Self> {
        // Store::selected_snapshot preserves validate_objects' manifest order.
        // These are identities of stored bytes, not reserialized envelopes.
        ensure!(
            records.len() == references.len(),
            "selected reference count mismatch"
        );
        let mut objects = BTreeMap::new();
        ensure!(
            !records.is_empty() && records.len() <= MAX_ITEMS,
            "invalid selected domain bound"
        );
        let mut ancestors = BTreeMap::new();
        let mut decoded = BTreeMap::new();
        let mut candidates: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        let mut bytes = 0usize;
        for (envelope, reference) in records.iter().zip(references) {
            envelope.validate()?;
            ensure!(
                crate::storage::valid_digest(&reference.sha256)
                    && reference.bytes > 0
                    && reference.bytes <= crate::storage::MAX_RECORD as u64,
                "invalid selected object reference"
            );
            objects.insert(envelope.revision_id, reference.clone());
            ensure!(
                matches!(
                    envelope.namespace,
                    Namespace::Conversation | Namespace::Source | Namespace::ExportAssociation
                ),
                "unsupported selected domain namespace; preserve and defer"
            );
            bytes = bytes
                .checked_add(usize::try_from(reference.bytes)?)
                .context("selected domain size overflow")?;
            ensure!(bytes <= MAX_METADATA, "selected domain exceeds bound");
            ensure!(
                ancestors
                    .insert(envelope.revision_id, envelope.clone())
                    .is_none(),
                "duplicate selected revision"
            );
            if envelope.kind == Kind::Value {
                decoded.insert(envelope.revision_id, Ledger::decode(envelope)?);
            }
            candidates
                .entry((envelope.namespace, envelope.record_id))
                .or_default()
                .insert(envelope.revision_id);
        }
        for envelope in records {
            for parent in &envelope.parents {
                let prior = ancestors.get(parent).context("selected ancestor missing")?;
                ensure!(
                    prior.namespace == envelope.namespace && prior.record_id == envelope.record_id,
                    "foreign selected parent"
                );
                candidates
                    .get_mut(&(envelope.namespace, envelope.record_id))
                    .unwrap()
                    .remove(parent);
            }
        }
        let mut outstanding: BTreeMap<_, _> = records
            .iter()
            .map(|e| (e.revision_id, e.parents.len()))
            .collect();
        let mut children: BTreeMap<Uuid, Vec<Uuid>> = BTreeMap::new();
        for envelope in records {
            for parent in &envelope.parents {
                children
                    .entry(*parent)
                    .or_default()
                    .push(envelope.revision_id);
            }
        }
        let mut ready: Vec<_> = outstanding
            .iter()
            .filter_map(|(id, count)| (*count == 0).then_some(*id))
            .collect();
        let mut processed = 0usize;
        while let Some(revision) = ready.pop() {
            processed += 1;
            for child in children.get(&revision).into_iter().flatten() {
                let count = outstanding.get_mut(child).unwrap();
                *count -= 1;
                if *count == 0 {
                    ready.push(*child);
                }
            }
        }
        ensure!(processed == ancestors.len(), "selected causal cycle");
        let mut heads = BTreeMap::new();
        for (key, revisions) in candidates {
            ensure!(revisions.len() == 1, "selected causal fork or cycle");
            heads.insert(key, *revisions.first().unwrap());
        }
        // Every ancestor must be reachable from a current head. A disconnected
        // cycle cannot hide beside a valid head and pass the head-count check.
        let mut reached = BTreeSet::new();
        let mut pending: Vec<_> = heads.values().copied().collect();
        while let Some(revision) = pending.pop() {
            if reached.insert(revision) {
                pending.extend(&ancestors[&revision].parents);
            }
        }
        ensure!(
            reached.len() == ancestors.len(),
            "unreachable selected causal history"
        );
        Ok(Self {
            ancestors,
            objects,
            records: decoded,
            heads,
        })
    }
    /// Current causal heads only; retained ancestors never become visible turns.
    pub fn inspect(&self, conversation: Uuid) -> Result<Vec<Record>> {
        self.document_ownership()?;
        let (_, root) = self
            .head(Namespace::Conversation, conversation)
            .context("selected conversation root absent")?;
        let Some(Record::Root(root)) = root else {
            anyhow::bail!("selected conversation root deleted or invalid")
        };
        let mut records: Vec<_> = self
            .heads
            .values()
            .filter_map(|revision| self.records.get(revision))
            .filter(|record| conversation_id(record) == conversation)
            .cloned()
            .collect();
        let turns = records
            .iter()
            .filter(|r| matches!(r, Record::Turn(_)))
            .count();
        ensure!(
            turns as u64 == root.next_sequence,
            "selected allocated conversation chronology is incomplete"
        );
        records.sort_by_key(|record| match record {
            Record::Turn(turn) => (0, turn.sequence),
            _ => (1, 0),
        });
        Ok(records)
    }
    pub fn head(&self, namespace: Namespace, id: Uuid) -> Option<(&Envelope, Option<&Record>)> {
        let revision = self.heads.get(&(namespace, id))?;
        Some((&self.ancestors[revision], self.records.get(revision)))
    }
    pub fn exact_reference(&self, reference: &IntentRecordRef) -> Result<&Record> {
        reference.validate()?;
        let envelope = self
            .ancestors
            .get(&reference.revision_id)
            .context("intent revision outside selected closure")?;
        ensure!(
            envelope.namespace == reference.namespace
                && envelope.record_id == reference.record_id
                && self.objects.get(&reference.revision_id) == Some(&reference.object),
            "intent immutable reference mismatch"
        );
        self.records
            .get(&reference.revision_id)
            .context("intent reference is a tombstone")
    }
}
pub(super) fn conversation_id(record: &Record) -> Uuid {
    match record {
        Record::Root(r) => r.id,
        Record::RequestEvidence(r) => r.conversation,
        Record::Turn(r) => r.conversation,
        Record::Source(r) => r.conversation,
        Record::NormalizedText(r) => r.conversation,
        Record::ArchivedNormalizedSource(r) => r.conversation,
        Record::NotebookPlan(r) => r.conversation,
        Record::NotebookStage(r) => r.conversation,
        Record::Capture(r) => r.conversation,
        Record::LegacyCapture(r) => r.conversation,
        Record::DevelopmentCapture(r) => r.conversation,
        Record::OutcomeFact(r) => r.conversation,
        Record::Binding(r) => r.conversation,
        Record::Export(r) => r.conversation,
        Record::Receipt(r) => r.acknowledgment.conversation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{Root, SCHEMA};
    use crate::storage::{digest, FORMAT};
    fn root() -> Envelope {
        let id = Uuid::new_v4();
        Envelope {
            envelope_version: FORMAT,
            namespace: Namespace::Conversation,
            domain_schema_version: SCHEMA,
            record_id: id,
            revision_id: Uuid::new_v4(),
            parents: BTreeSet::new(),
            operation_id: Uuid::new_v4(),
            actor_id: Uuid::new_v4(),
            kind: Kind::Value,
            payload: serde_json::to_value(Record::Root(Root {
                id,
                next_sequence: 1,
                binding: None,
                created_ms: 1,
                updated_ms: 1,
            }))
            .unwrap(),
            media_descriptors: vec![],
        }
    }
    fn edit(prior: &Envelope) -> Envelope {
        let mut next = prior.clone();
        next.revision_id = Uuid::new_v4();
        next.operation_id = Uuid::new_v4();
        next.parents = BTreeSet::from([prior.revision_id]);
        next
    }
    #[test]
    fn supplied_writer_text_requires_its_turn_without_native_document_ownership() {
        use crate::conversation::{Mode, NormalizedTextSource, Outcome, Role, TextScope, Turn};
        use crate::retrieval::{NormalizedDocument, SourceFormat, SourceUnit};
        let root = root();
        let conversation = root.record_id;
        let turn_id = Uuid::new_v4();
        let source = NormalizedTextSource::new(
            conversation,
            turn_id,
            NormalizedDocument {
                document: Uuid::new_v4(),
                format: SourceFormat::Notebook,
                source_sha256: "a".repeat(64),
                extractor_revision: "selected-writer-fixture-v1".into(),
                units: vec![SourceUnit {
                    page: Some(1),
                    section: None,
                    text: "Force = 6 N.".into(),
                }],
            },
            TextScope::SuppliedNotebook,
            None,
        )
        .unwrap();
        let mut source_envelope = root.clone();
        source_envelope.namespace = Namespace::Source;
        source_envelope.domain_schema_version = 2;
        source_envelope.record_id = source.id;
        source_envelope.revision_id = Uuid::new_v4();
        source_envelope.payload =
            serde_json::to_value(Record::NormalizedText(Box::new(source))).unwrap();
        let mut turn = root.clone();
        turn.record_id = turn_id;
        turn.revision_id = Uuid::new_v4();
        turn.payload = serde_json::to_value(Record::Turn(Turn {
            id: turn_id,
            conversation,
            exchange: Uuid::new_v4(),
            sequence: 0,
            role: Role::User,
            mode: Mode::Writer,
            outcome: Outcome::Interpreted,
            text: Some("Make notes".into()),
            sources: vec![source_envelope.record_id],
            correction_of: None,
            created_ms: 1,
            updated_ms: 1,
            completion: None,
        }))
        .unwrap();
        let projection = SelectedDomainProjection::from_records(&[
            root.clone(),
            turn.clone(),
            source_envelope.clone(),
        ])
        .unwrap();
        assert_eq!(
            projection.document_ownership().unwrap()[&conversation],
            DocumentOwnership::DeferredMissing
        );
        let missing =
            SelectedDomainProjection::from_records(&[root.clone(), source_envelope.clone()])
                .unwrap();
        assert!(missing.document_ownership().is_err());
        for namespace in [
            Namespace::Conversation,
            Namespace::ExportAssociation,
            Namespace::SubjectMemory,
        ] {
            let mut wrong_domain = source_envelope.clone();
            wrong_domain.namespace = namespace;
            assert!(SelectedDomainProjection::from_records(&[
                root.clone(),
                turn.clone(),
                wrong_domain
            ])
            .is_err());
        }
        source_envelope.domain_schema_version = 1;
        assert!(SelectedDomainProjection::from_records(&[root, turn, source_envelope]).is_err());
    }
    #[test]
    fn generated_and_refined_writer_history_has_valid_selected_source_closure() {
        use crate::conversation::TextScope;
        use crate::retrieval::{NormalizedDocument, SourceFormat, SourceUnit};
        use crate::storage::{Store, StorePaths};
        use crate::writer::{self, Request, Status};
        use std::sync::Arc;
        struct Model;
        impl crate::LLMEngine for Model {
            fn add_text_content(&mut self, _: &str) {}
            fn add_image_content(&mut self, _: &str) {
                panic!("text-only fixture")
            }
            fn clear_content(&mut self) {}
            fn execute(&mut self) -> Result<String> {
                Ok("# Notes\n\nForce = 6 N (page 1).".into())
            }
        }
        let path = std::env::temp_dir().join(format!("rmb-selected-writer-{}", Uuid::new_v4()));
        let store = Arc::new(
            Store::open(StorePaths {
                data: path.join("data"),
                cache: path.join("cache"),
                credentials: path.join("credentials"),
            })
            .unwrap(),
        );
        let ledger = Ledger::new(store.clone());
        let conversation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
        let preference = crate::config::Personalization::default()
            .snapshot()
            .unwrap();
        let generated = writer::run(
            &ledger,
            &mut Model,
            &Request::Generate {
                conversation,
                operation: Uuid::new_v4(),
                request: "Make study notes".into(),
                document: NormalizedDocument {
                    document: Uuid::new_v4(),
                    format: SourceFormat::Notebook,
                    source_sha256: "a".repeat(64),
                    extractor_revision: "writer-selected-fixture-v1".into(),
                    units: vec![SourceUnit {
                        page: Some(1),
                        section: None,
                        text: "Force = 6 N.".into(),
                    }],
                },
                selection: TextScope::SuppliedNotebook,
                guidance: None,
            },
            &preference,
            &mut || false,
        );
        assert_eq!(generated.status, Status::Generated);
        let refined = writer::run(
            &ledger,
            &mut Model,
            &Request::Refine {
                conversation,
                operation: Uuid::new_v4(),
                request: "Keep the exact force".into(),
                draft: generated.draft.unwrap(),
            },
            &preference,
            &mut || false,
        );
        assert_eq!(refined.status, Status::Generated);
        let records = store
            .snapshot_revisions_matching(
                &[Namespace::Conversation, Namespace::Source],
                MAX_ITEMS,
                |_| true,
            )
            .unwrap();
        let projection = SelectedDomainProjection::from_records(&records).unwrap();
        assert_eq!(
            projection.document_ownership().unwrap()[&conversation],
            DocumentOwnership::DeferredMissing
        );
        let source = records
            .iter()
            .find(|e| e.namespace == Namespace::Source)
            .unwrap();
        let Record::NormalizedText(source_record) = Ledger::decode(source).unwrap() else {
            panic!("text source")
        };
        let missing_origin: Vec<_> = records
            .iter()
            .filter(|e| e.record_id != source_record.turn)
            .cloned()
            .collect();
        assert!(SelectedDomainProjection::from_records(&missing_origin)
            .unwrap()
            .document_ownership()
            .is_err());
        let mut foreign = records.clone();
        for envelope in &mut foreign {
            if envelope.record_id == source_record.turn {
                envelope.payload["record"]["conversation"] =
                    serde_json::to_value(Uuid::new_v4()).unwrap();
            }
        }
        assert!(SelectedDomainProjection::from_records(&foreign)
            .unwrap()
            .document_ownership()
            .is_err());
        drop(ledger);
        drop(store);
        let resolved = path.canonicalize().unwrap();
        assert_eq!(
            resolved.parent().unwrap(),
            std::env::temp_dir().canonicalize().unwrap()
        );
        assert!(resolved
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("rmb-selected-writer-"));
        std::fs::remove_dir_all(resolved).unwrap();
    }
    #[test]
    fn ordinary_chain_has_one_head_and_exact_ancestor_lookup() {
        let first = root();
        let second = edit(&first);
        let third = edit(&second);
        let view = SelectedDomainProjection::from_records(&[third.clone(), first.clone(), second])
            .unwrap();
        assert_eq!(
            view.head(first.namespace, first.record_id)
                .unwrap()
                .0
                .revision_id,
            third.revision_id
        );
        let bytes = serde_json::to_vec(&first).unwrap();
        let mut reference = IntentRecordRef {
            namespace: first.namespace,
            record_id: first.record_id,
            revision_id: first.revision_id,
            object: ObjectRef {
                sha256: digest(&bytes),
                bytes: bytes.len() as u64,
            },
        };
        assert!(view.exact_reference(&reference).is_ok());
        reference.object.sha256 = "0".repeat(64);
        assert!(view.exact_reference(&reference).is_err());
        reference.revision_id = Uuid::new_v4();
        assert!(view.exact_reference(&reference).is_err());
    }
    #[test]
    fn fork_missing_foreign_and_duplicate_parents_refuse() {
        let first = root();
        let second = edit(&first);
        let fork = edit(&first);
        assert!(
            SelectedDomainProjection::from_records(&[first.clone(), second.clone(), fork]).is_err()
        );
        assert!(SelectedDomainProjection::from_records(std::slice::from_ref(&second)).is_err());
        let foreign = root();
        let mut wrong = second.clone();
        wrong.parents = BTreeSet::from([foreign.revision_id]);
        assert!(SelectedDomainProjection::from_records(&[first.clone(), foreign, wrong]).is_err());
        assert!(SelectedDomainProjection::from_records(&[first.clone(), first]).is_err());
    }
    #[test]
    fn selected_tombstone_does_not_resolve_a_historical_value_as_current() {
        let first = root();
        let mut deleted = edit(&first);
        deleted.kind = Kind::Tombstone;
        deleted.payload = serde_json::Value::Null;
        let view = SelectedDomainProjection::from_records(&[first.clone(), deleted]).unwrap();
        let (head, record) = view.head(first.namespace, first.record_id).unwrap();
        assert_eq!(head.kind, Kind::Tombstone);
        assert!(record.is_none());
        assert!(view.head(first.namespace, Uuid::new_v4()).is_none());
    }
    #[test]
    fn cycle_behind_a_single_apparent_head_refuses() {
        let mut first = root();
        let second = edit(&first);
        let tail = edit(&second);
        first.parents.insert(second.revision_id);
        assert!(SelectedDomainProjection::from_records(&[first, second, tail]).is_err());
    }
    #[test]
    fn real_store_preserves_noncanonical_selected_identity_and_rejects_unstored_identity() {
        use crate::storage::selection::{SelectionChange, SelectionScope};
        use crate::storage::{Manifest, Scope, Store, StorePaths};
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let directory =
            Fixture(std::env::temp_dir().join(format!("buddy-selected-domain-{}", Uuid::new_v4())));
        let store = Store::open(StorePaths {
            data: directory.0.join("data"),
            cache: directory.0.join("cache"),
            credentials: directory.0.join("secrets"),
        })
        .unwrap();
        let mut envelope = root();
        envelope.actor_id = store.actor_id;
        let bytes = serde_json::to_vec_pretty(&envelope).unwrap();
        let stored = ObjectRef {
            sha256: digest(&bytes),
            bytes: bytes.len() as u64,
        };
        let scope = SelectionScope {
            group: Uuid::new_v4(),
            key_sha256: digest(b"domain selected regression"),
            binding_sha256: digest(b"binding"),
        };
        store
            .initialize_selected(
                &scope,
                SelectionChange {
                    operation: Uuid::new_v4(),
                    accepted_base_sha256: digest(b"base"),
                    retained: vec![],
                    selected: Manifest {
                        format: FORMAT,
                        transaction_id: Uuid::new_v4(),
                        scope: Scope::SelectedRecords,
                        records: vec![stored.clone()],
                        record_namespaces: BTreeMap::from([(
                            stored.sha256.clone(),
                            envelope.namespace,
                        )]),
                        media: vec![],
                        media_coverage: vec![],
                    },
                },
                BTreeMap::from([(stored.sha256.clone(), bytes)]),
            )
            .unwrap();
        let snapshot = store.selected_snapshot(&scope, MAX_ITEMS).unwrap().unwrap();
        let view = SelectedDomainProjection::from_snapshot(&snapshot).unwrap();
        let mut reference = IntentRecordRef {
            namespace: envelope.namespace,
            record_id: envelope.record_id,
            revision_id: envelope.revision_id,
            object: stored.clone(),
        };
        assert!(view.exact_reference(&reference).is_ok());
        let canonical = serde_json::to_vec(&envelope).unwrap();
        reference.object = ObjectRef {
            sha256: digest(&canonical),
            bytes: canonical.len() as u64,
        };
        assert_ne!(reference.object, stored);
        assert!(!snapshot
            .transaction
            .selected
            .records
            .contains(&reference.object));
        assert!(view.exact_reference(&reference).is_err());
    }
    fn completed_turn(root: &Envelope, sequence: u64, document: Uuid) -> Envelope {
        use crate::conversation::{
            CompletionEvidence, Mode, Outcome, Role, SourceObservation, Turn,
        };
        let mut envelope = root.clone();
        envelope.record_id = Uuid::new_v4();
        envelope.revision_id = Uuid::new_v4();
        envelope.operation_id = Uuid::new_v4();
        envelope.payload = serde_json::to_value(Record::Turn(Turn {
            id: envelope.record_id,
            conversation: root.record_id,
            exchange: Uuid::new_v4(),
            sequence,
            role: Role::Assistant,
            mode: Mode::Reader,
            outcome: Outcome::Completed,
            text: Some("answer".into()),
            sources: vec![],
            correction_of: None,
            created_ms: 1,
            updated_ms: 2,
            completion: Some(CompletionEvidence {
                native_operation: Uuid::new_v4(),
                exact_text: "answer".into(),
                procedure: "synthetic domain fixture, no native authority".into(),
                observation: SourceObservation {
                    document,
                    page: Uuid::new_v4(),
                    session: "fixture".into(),
                    visit: "fixture".into(),
                    content_revision: None,
                    order_revision: None,
                    capability_revision: "fixture".into(),
                    evidence_procedure: "fixture".into(),
                },
            }),
        }))
        .unwrap();
        envelope
    }
    #[test]
    fn ownership_defers_missing_and_ambiguous_documents_without_inventing_identity() {
        let mut first = root();
        let Record::Root(mut value) = Ledger::decode(&first).unwrap() else {
            unreachable!()
        };
        value.next_sequence = 2;
        first.payload = serde_json::to_value(Record::Root(value)).unwrap();
        let missing = SelectedDomainProjection::from_records(std::slice::from_ref(&first)).unwrap();
        assert_eq!(
            missing.document_ownership().unwrap()[&first.record_id],
            DocumentOwnership::DeferredMissing
        );
        let document = Uuid::new_v4();
        let one = completed_turn(&first, 0, document);
        let known = SelectedDomainProjection::from_records(&[first.clone(), one.clone()]).unwrap();
        assert_eq!(
            known.document_ownership().unwrap()[&first.record_id],
            DocumentOwnership::Document(document)
        );
        let other_document = Uuid::new_v4();
        let two = completed_turn(&first, 1, other_document);
        let ambiguous = SelectedDomainProjection::from_records(&[first.clone(), one, two]).unwrap();
        assert_eq!(
            ambiguous.document_ownership().unwrap()[&first.record_id],
            DocumentOwnership::DeferredAmbiguous(BTreeSet::from([document, other_document]))
        );
    }
    #[test]
    fn ownership_refuses_missing_root_broken_sources_and_current_chronology() {
        let first = root();
        let one = completed_turn(&first, 0, Uuid::new_v4());
        assert!(
            SelectedDomainProjection::from_records(std::slice::from_ref(&one))
                .unwrap()
                .document_ownership()
                .is_err()
        );
        let mut bad = one.clone();
        let Record::Turn(mut turn) = Ledger::decode(&bad).unwrap() else {
            unreachable!()
        };
        turn.sources.push(Uuid::new_v4());
        bad.payload = serde_json::to_value(Record::Turn(turn)).unwrap();
        assert!(
            SelectedDomainProjection::from_records(&[first.clone(), bad])
                .unwrap()
                .document_ownership()
                .is_err()
        );
        let collision = completed_turn(&first, 0, Uuid::new_v4());
        assert!(
            SelectedDomainProjection::from_records(&[first.clone(), one, collision])
                .unwrap()
                .document_ownership()
                .is_err()
        );
        let beyond = completed_turn(&first, 1, Uuid::new_v4());
        assert!(SelectedDomainProjection::from_records(&[first, beyond])
            .unwrap()
            .document_ownership()
            .is_err());
    }
    #[test]
    fn correction_cannot_use_an_old_target_sequence() {
        let mut base = root();
        let Record::Root(mut value) = Ledger::decode(&base).unwrap() else {
            unreachable!()
        };
        value.next_sequence = 3;
        base.payload = serde_json::to_value(Record::Root(value)).unwrap();
        let document = Uuid::new_v4();
        let original = completed_turn(&base, 0, document);
        let mut latest = original.clone();
        latest.revision_id = Uuid::new_v4();
        latest.parents.insert(original.revision_id);
        let Record::Turn(mut target) = Ledger::decode(&latest).unwrap() else {
            unreachable!()
        };
        target.sequence = 2;
        latest.payload = serde_json::to_value(Record::Turn(target)).unwrap();
        let mut correction = completed_turn(&base, 1, document);
        let Record::Turn(mut turn) = Ledger::decode(&correction).unwrap() else {
            unreachable!()
        };
        turn.correction_of = Some(original.record_id);
        correction.payload = serde_json::to_value(Record::Turn(turn)).unwrap();
        assert!(SelectedDomainProjection::from_records(&[
            base.clone(),
            original.clone(),
            correction.clone()
        ])
        .unwrap()
        .document_ownership()
        .is_ok());
        assert!(
            SelectedDomainProjection::from_records(&[base, original, latest, correction])
                .unwrap()
                .document_ownership()
                .is_err()
        );
    }
}

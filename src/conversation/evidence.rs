//! Durable ordinary historical selections. These facts confer no effect authority.
use super::{receiving::*, *};
pub use crate::storage::captured::RecordRef;
use crate::storage::{
    captured::{Captured, ReadBudget},
    MAX_METADATA, MAX_RECORD,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotIdentity {
    pub actor: Uuid,
    pub generation: Uuid,
    pub heads_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestEvidence {
    pub evidence_version: u32,
    pub id: Uuid,
    pub conversation: Uuid,
    pub request_turn: Uuid,
    pub operation: Uuid,
    pub recorded_ms: u64,
    pub frame_json: String,
    pub frame_sha256: String,
    pub counted_tokens: usize,
    pub limits: ReceivingBudget,
    pub snapshot: SnapshotIdentity,
    pub root: RecordRef,
    pub turns: Vec<RecordRef>,
    pub sources: Vec<RecordRef>,
}
impl RequestEvidence {
    pub fn identity(operation: Uuid) -> Uuid {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("urn:rmb:request-evidence:v1:{operation}").as_bytes(),
        )
    }
    pub(super) fn validate_shape(&self) -> Result<()> {
        let limit = validate_budget(&self.limits)?;
        ensure!(
            self.evidence_version == 1
                && !self.operation.is_nil()
                && self.id == Self::identity(self.operation)
                && !self.conversation.is_nil()
                && !self.request_turn.is_nil()
                && !self.snapshot.actor.is_nil()
                && !self.snapshot.generation.is_nil()
                && crate::storage::valid_digest(&self.snapshot.heads_sha256)
                && self.frame_json.len() <= self.limits.max_encoded_bytes
                && digest(self.frame_json.as_bytes()) == self.frame_sha256
                && self.counted_tokens > 0
                && self.counted_tokens <= limit,
            "invalid request evidence"
        );
        let mut ids = BTreeSet::new();
        let mut bytes = 0u64;
        for r in std::iter::once(&self.root)
            .chain(&self.turns)
            .chain(&self.sources)
        {
            ensure!(
                !r.record.is_nil()
                    && !r.revision.is_nil()
                    && crate::storage::valid_digest(&r.object.sha256)
                    && r.object.bytes > 0
                    && r.object.bytes <= MAX_RECORD as u64
                    && ids.insert((r.namespace, r.record)),
                "invalid evidence reference"
            );
            bytes = bytes
                .checked_add(r.object.bytes)
                .context("reference overflow")?;
        }
        ensure!(
            ids.len() <= MAX_ITEMS && bytes <= MAX_METADATA as u64,
            "evidence reference bound"
        );
        Ok(())
    }
    fn fingerprint(&self) -> Result<String> {
        Ledger::fingerprint(&(
            "request-evidence-v1",
            self.conversation,
            self.request_turn,
            self.recorded_ms,
            &self.frame_json,
            self.counted_tokens,
            &self.limits,
        ))
    }
}
pub struct PreparedRequestEvidence {
    context: FrozenRequestContext,
    capture: Captured,
    root: RecordRef,
    turns: Vec<RecordRef>,
    sources: Vec<RecordRef>,
    limits: ReceivingBudget,
}
impl PreparedRequestEvidence {
    pub fn context(&self) -> &FrozenRequestContext {
        &self.context
    }
}
#[derive(Debug)]
pub struct HistoricalRequestEvidence {
    pub record: RequestEvidence,
    pub acknowledgment: Acknowledgment,
    /// Present availability, separate from the unchanged recorded frame.
    pub unavailable_media: Vec<Media>,
}
pub(super) fn matches(envelope: &Envelope, conversation: Uuid) -> bool {
    if !matches!(
        envelope.namespace,
        Namespace::Conversation | Namespace::Source | Namespace::ExportAssociation
    ) {
        return false;
    }
    let identity = conversation.to_string();
    let payload = &envelope.payload["record"];
    payload["conversation"].as_str() == Some(&identity)
        || payload["acknowledgment"]["conversation"].as_str() == Some(&identity)
        || (envelope.namespace == Namespace::Conversation && envelope.record_id == conversation)
}
impl Ledger {
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_request_evidence(
        &self,
        conversation: Uuid,
        request_turn: Uuid,
        selection: Option<TurnRange>,
        source: RequestSource,
        budget: ReceivingBudget,
        retrieval: Option<&dyn crate::retrieval::Provider>,
        token_count: impl Fn(&str) -> Result<usize>,
    ) -> Result<PreparedRequestEvidence> {
        validate_budget(&budget)?;
        self.complete_store()?;
        let capture = self.store.capture(conversation, matches)?;
        let envelopes = capture
            .records
            .iter()
            .map(|(e, _)| e.clone())
            .collect::<Vec<_>>();
        let records = Self::inspect_snapshot(conversation, false, &envelopes)?;
        self.validate_evidence_records(&records)?;
        let context = self.assemble_request_context(
            conversation,
            request_turn,
            selection,
            source,
            budget.clone(),
            retrieval,
            token_count,
            records,
        )?;
        let mut needed = context
            .history()
            .iter()
            .chain(std::iter::once(context.request()))
            .flat_map(|t| t.sources.iter().copied())
            .collect::<BTreeSet<_>>();
        if let SourceAssociation::Original { source_record, .. } = context.source() {
            needed.insert(source_id(source_record).context("original source absent")?);
        }
        let root = capture
            .records
            .iter()
            .find(|(e, _)| e.namespace == Namespace::Conversation && e.record_id == conversation)
            .context("root absent")?
            .1
            .clone();
        let mut turns = Vec::new();
        let mut sources = Vec::new();
        for (e, reference) in &capture.records {
            if e.kind != Kind::Value {
                continue;
            }
            match Self::decode(e)? {
                Record::Turn(t) if t.sequence <= context.request().sequence => {
                    turns.push((t.sequence, reference.clone()))
                }
                r if source_id(&r).is_some_and(|id| needed.contains(&id)) => {
                    sources.push(reference.clone())
                }
                _ => {}
            }
        }
        turns.sort_by_key(|(sequence, _)| *sequence);
        Ok(PreparedRequestEvidence {
            context,
            capture,
            root,
            turns: turns.into_iter().map(|(_, r)| r).collect(),
            sources,
            limits: budget,
        })
    }
    pub fn persist_request_evidence(
        &self,
        operation: Uuid,
        recorded_ms: u64,
        prepared: &PreparedRequestEvidence,
    ) -> Result<WriteResult> {
        ensure!(!operation.is_nil(), "nil evidence operation");
        let context = &prepared.context;
        let fact = RequestEvidence {
            evidence_version: 1,
            id: RequestEvidence::identity(operation),
            conversation: context.request().conversation,
            request_turn: context.request().id,
            operation,
            recorded_ms,
            frame_json: context.encoded_json().into(),
            frame_sha256: digest(context.encoded_json().as_bytes()),
            counted_tokens: context.counted_tokens(),
            limits: prepared.limits.clone(),
            snapshot: SnapshotIdentity {
                actor: prepared.capture.actor,
                generation: prepared.capture.generation,
                heads_sha256: prepared.capture.digest()?,
            },
            root: prepared.root.clone(),
            turns: prepared.turns.clone(),
            sources: prepared.sources.clone(),
        };
        fact.validate_shape()?;
        let fingerprint = fact.fingerprint()?;
        if let Some(prior) = self.replay_evidence(&fact, &fingerprint)? {
            return Ok(prior);
        }
        let pinned_records = std::iter::once(&fact.root)
            .chain(&fact.turns)
            .chain(&fact.sources)
            .map(|reference| {
                let (envelope, _) = prepared
                    .capture
                    .records
                    .iter()
                    .find(|(_, r)| r == reference)
                    .context("captured reference absent")?;
                Self::decode(envelope)
            })
            .collect::<Result<Vec<_>>>()?;
        validate_records(&fact, &pinned_records)?;
        let root_envelope = prepared
            .capture
            .records
            .iter()
            .find(|(_, r)| r == &fact.root)
            .context("captured root absent")?
            .0
            .clone();
        let Record::Root(mut root) = Self::decode(&root_envelope)? else {
            bail!("captured root invalid");
        };
        root.updated_ms = root.updated_ms.max(recorded_ms);
        let root = self.envelope(
            Namespace::Conversation,
            fact.conversation,
            ExpectedHeads(BTreeSet::from([root_envelope.revision_id])),
            Record::Root(root),
            vec![],
        )?;
        let ack = Acknowledgment {
            operation,
            conversation: fact.conversation,
            records: vec![fact.id],
            applied_root_revision: root.revision_id,
            binding: match Self::decode(&root)? {
                Record::Root(r) => r.binding,
                _ => unreachable!(),
            },
        };
        let evidence = self.envelope(
            Namespace::Conversation,
            fact.id,
            ExpectedHeads::default(),
            Record::RequestEvidence(Box::new(fact.clone())),
            vec![],
        )?;
        // Include string escaping and every envelope field in the bounded serialization.
        encoded(&evidence, MAX_RECORD)?;
        let receipt = self.envelope(
            Namespace::Conversation,
            Self::receipt_id(operation),
            ExpectedHeads::default(),
            Record::Receipt(Receipt {
                fingerprint: fingerprint.clone(),
                acknowledgment: ack,
                admitted_intent: None,
            }),
            vec![],
        )?;
        // Admit the complete original readback before committing. Otherwise a
        // large but individually valid fact could commit and immediately exceed
        // the shared reference-work bound while recovering its acknowledgment.
        let mut readback = ReadBudget::default();
        for reference in std::iter::once(&fact.root)
            .chain(&fact.turns)
            .chain(&fact.sources)
        {
            readback.charge(0)?;
            readback.charge(
                reference
                    .object
                    .bytes
                    .checked_mul(2)
                    .context("readback work overflow")?,
            )?;
        }
        for envelope in [&evidence, &root, &receipt] {
            readback.charge(0)?;
            readback.charge(
                (encoded(envelope, MAX_RECORD)?.len() as u64)
                    .checked_mul(2)
                    .context("readback envelope work overflow")?,
            )?;
        }
        let attempted = self
            .store
            .commit_captured(&prepared.capture, vec![evidence, root, receipt]);
        // Recover the original winner even after an interrupted durable commit or a race.
        if let Some(prior) = self.replay_evidence(&fact, &fingerprint)? {
            return Ok(prior);
        }
        attempted?;
        bail!("evidence receipt unavailable after publication")
    }
    fn replay_evidence(
        &self,
        fact: &RequestEvidence,
        fingerprint: &str,
    ) -> Result<Option<WriteResult>> {
        let Some(prior) = self.replay_with_bounds(fact.operation, fingerprint, true)? else {
            ensure!(
                self.store
                    .bounded_heads(Namespace::Conversation, fact.id, 1)?
                    .is_empty(),
                "evidence without receipt"
            );
            return Ok(None);
        };
        let historical = self
            .request_evidence(fact.conversation, fact.request_turn, fact.operation)?
            .context("evidence receipt missing fact")?;
        ensure!(
            historical.record.fingerprint()? == fingerprint
                && historical.acknowledgment == prior.historical,
            "evidence replay mismatch"
        );
        Ok(Some(prior))
    }
    pub fn request_evidence(
        &self,
        conversation: Uuid,
        request_turn: Uuid,
        operation: Uuid,
    ) -> Result<Option<HistoricalRequestEvidence>> {
        self.complete_store()?;
        ensure!(
            !conversation.is_nil() && !request_turn.is_nil() && !operation.is_nil(),
            "nil historical identity"
        );
        let heads = self.store.heads(
            Namespace::Conversation,
            RequestEvidence::identity(operation),
        )?;
        if heads.is_empty() {
            ensure!(
                self.store
                    .bounded_heads(Namespace::Conversation, Self::receipt_id(operation), 1)?
                    .is_empty(),
                "receipt without evidence"
            );
            return Ok(None);
        }
        ensure!(
            heads.len() == 1 && heads[0].parents.is_empty(),
            "conflicted or mutable evidence"
        );
        let Record::RequestEvidence(fact) = Self::decode(&heads[0])? else {
            bail!("wrong evidence kind");
        };
        ensure!(
            fact.conversation == conversation
                && fact.request_turn == request_turn
                && fact.operation == operation,
            "historical evidence identity mismatch"
        );
        let mut budget = ReadBudget::default();
        let (ack, media) = self.validate_evidence(&fact, &mut budget)?;
        Ok(Some(HistoricalRequestEvidence {
            record: *fact,
            acknowledgment: ack,
            unavailable_media: media,
        }))
    }
    pub(super) fn validate_evidence_records(&self, records: &[Record]) -> Result<()> {
        let mut budget = ReadBudget::default();
        for record in records {
            if let Record::RequestEvidence(fact) = record {
                self.validate_evidence(fact, &mut budget)?;
            }
        }
        Ok(())
    }
    fn validate_evidence(
        &self,
        fact: &RequestEvidence,
        budget: &mut ReadBudget,
    ) -> Result<(Acknowledgment, Vec<Media>)> {
        fact.validate_shape()?;
        let mut records = Vec::new();
        for reference in std::iter::once(&fact.root)
            .chain(&fact.turns)
            .chain(&fact.sources)
        {
            records.push(Self::decode(
                &self.store.resolve_record(reference, budget)?,
            )?);
        }
        let media = validate_records(fact, &records)?;
        let heads = self
            .store
            .bounded_heads(Namespace::Conversation, fact.id, 1)?;
        ensure!(
            heads.len() == 1
                && heads[0].parents.is_empty()
                && Self::decode(&heads[0])? == Record::RequestEvidence(Box::new(fact.clone())),
            "evidence head mismatch"
        );
        let transaction = self
            .store
            .original_transaction(heads[0].revision_id, budget)?;
        let ack = validate_transaction(fact, &transaction)?;
        validate_root_successor(fact, &records[0], &transaction)?;
        let receipt_heads = self.store.bounded_heads(
            Namespace::Conversation,
            Self::receipt_id(fact.operation),
            1,
        )?;
        ensure!(
            receipt_heads.len() == 1 && transaction.iter().any(|e| e == &receipt_heads[0]),
            "receipt head mismatch"
        );
        let unavailable = media
            .into_iter()
            .filter(|m| {
                self.store
                    .open_object(&ObjectRef {
                        sha256: m.sha256.clone(),
                        bytes: m.bytes,
                    })
                    .is_err()
            })
            .collect();
        Ok((ack, unavailable))
    }
}
fn validate_root_successor(
    fact: &RequestEvidence,
    original: &Record,
    transaction: &[Envelope],
) -> Result<()> {
    let Record::Root(mut expected) = original.clone() else {
        bail!("original root kind");
    };
    expected.updated_ms = expected.updated_ms.max(fact.recorded_ms);
    ensure!(
        transaction
            .iter()
            .any(|e| Ledger::decode(e).ok() == Some(Record::Root(expected.clone()))),
        "evidence changed root allocation or binding"
    );
    Ok(())
}
/// Maintenance already verified the raw objects. Reuse the domain validators
/// against its immutable inventory, including exact original manifest refs.
pub(super) fn validate_cleanup(
    snapshot: &crate::storage::conversation_cleanup::Snapshot,
) -> Result<()> {
    let mut budget = ReadBudget::default();
    let mut cache = BTreeMap::<Uuid, Record>::new();
    for e in &snapshot.history {
        if e.kind != Kind::Value {
            continue;
        }
        let Record::RequestEvidence(fact) = Ledger::decode(e)? else {
            continue;
        };
        fact.validate_shape()?;
        ensure!(
            e.parents.is_empty()
                && snapshot
                    .current
                    .iter()
                    .filter(|h| h.namespace == e.namespace && h.record_id == e.record_id)
                    .eq(std::iter::once(e)),
            "mutable or conflicted evidence"
        );
        let mut records = Vec::new();
        for r in std::iter::once(&fact.root)
            .chain(&fact.turns)
            .chain(&fact.sources)
        {
            budget.charge(0)?;
            ensure!(
                snapshot.raw_hashes.get(&r.revision) == Some(&r.object.sha256),
                "cleanup pinned hash mismatch"
            );
            let envelope = snapshot
                .history
                .iter()
                .find(|h| h.revision_id == r.revision)
                .context("cleanup pinned record absent")?;
            ensure!(
                envelope.namespace == r.namespace && envelope.record_id == r.record,
                "cleanup pinned owner mismatch"
            );
            ensure!(
                snapshot
                    .manifests
                    .iter()
                    .any(|m| m.records.contains(&r.object)),
                "cleanup original ref absent"
            );
            if let std::collections::btree_map::Entry::Vacant(entry) = cache.entry(r.revision) {
                budget.charge(
                    r.object
                        .bytes
                        .checked_mul(2)
                        .context("cleanup ref work overflow")?,
                )?;
                entry.insert(Ledger::decode(envelope)?);
            }
            records.push(cache[&r.revision].clone());
        }
        validate_records(&fact, &records)?;
        let hash = snapshot
            .raw_hashes
            .get(&e.revision_id)
            .context("cleanup evidence raw hash")?;
        let mut manifests = Vec::new();
        for m in &snapshot.manifests {
            for r in &m.records {
                budget.charge(0)?;
                if &r.sha256 == hash {
                    manifests.push(m);
                }
            }
        }
        ensure!(
            manifests.len() == 1 && manifests[0].records.len() == 3,
            "ambiguous cleanup evidence transaction"
        );
        let mut transaction = Vec::new();
        for r in &manifests[0].records {
            let member = snapshot
                .history
                .iter()
                .find(|h| snapshot.raw_hashes.get(&h.revision_id) == Some(&r.sha256))
                .context("cleanup transaction member absent")?;
            budget.charge(r.bytes.checked_mul(2).context("cleanup transaction work")?)?;
            transaction.push(member.clone());
        }
        let ack = validate_transaction(&fact, &transaction)?;
        validate_root_successor(&fact, &records[0], &transaction)?;
        let receipt = transaction
            .iter()
            .find(|h| h.record_id == Ledger::receipt_id(ack.operation))
            .context("cleanup receipt absent")?;
        ensure!(
            snapshot
                .current
                .iter()
                .filter(|h| h.namespace == receipt.namespace && h.record_id == receipt.record_id)
                .eq(std::iter::once(receipt)),
            "cleanup receipt head mismatch"
        );
    }
    Ok(())
}
pub(super) fn validate_records(fact: &RequestEvidence, records: &[Record]) -> Result<Vec<Media>> {
    fact.validate_shape()?;
    ensure!(
        records.len() == 1 + fact.turns.len() + fact.sources.len(),
        "evidence reference count"
    );
    let Record::Root(root) = &records[0] else {
        bail!("historical root absent");
    };
    ensure!(
        root.id == fact.conversation
            && fact.root.record == root.id
            && fact.root.namespace == Namespace::Conversation,
        "historical root mismatch"
    );
    let mut turns = Vec::new();
    for (index, record) in records[1..1 + fact.turns.len()].iter().enumerate() {
        let Record::Turn(turn) = record else {
            bail!("historical turn absent");
        };
        ensure!(
            turn.conversation == fact.conversation
                && turn.sequence == index as u64
                && turn.sequence < root.next_sequence,
            "historical turn prefix incomplete"
        );
        turns.push(turn);
    }
    ensure!(
        turns.last().is_some_and(|t| t.id == fact.request_turn),
        "historical request not prefix endpoint"
    );
    for turn in &turns {
        if let Some(corrected) = turn.correction_of {
            ensure!(
                turn.role == Role::User
                    && turns
                        .iter()
                        .any(|prior| prior.id == corrected && prior.sequence < turn.sequence),
                "historical correction target absent or foreign"
            );
        }
    }
    for record in &records[1 + fact.turns.len()..] {
        ensure!(source_id(record).is_some(), "historical source kind");
    }
    receiving::validate_historical(
        &fact.frame_json,
        fact.conversation,
        fact.request_turn,
        &fact.limits,
        records,
    )
}
pub(super) fn validate_transaction(
    fact: &RequestEvidence,
    transaction: &[Envelope],
) -> Result<Acknowledgment> {
    ensure!(transaction.len() == 3, "evidence transaction size");
    let mut root = None;
    let mut receipt = None;
    let mut evidence = None;
    for e in transaction {
        match Ledger::decode(e)? {
            Record::Root(r) if r.id == fact.conversation && root.is_none() => root = Some((e, r)),
            Record::RequestEvidence(r)
                if *r == *fact && evidence.is_none() && e.parents.is_empty() =>
            {
                evidence = Some(e)
            }
            Record::Receipt(r) if receipt.is_none() && e.parents.is_empty() => receipt = Some(r),
            _ => bail!("invalid evidence transaction member"),
        }
    }
    let (root_e, root) = root.context("evidence root absent")?;
    let receipt = receipt.context("evidence receipt absent")?;
    ensure!(
        evidence.is_some()
            && root_e.parents == BTreeSet::from([fact.root.revision])
            && receipt.admitted_intent.is_none()
            && receipt.fingerprint == fact.fingerprint()?
            && receipt.acknowledgment
                == Acknowledgment {
                    operation: fact.operation,
                    conversation: fact.conversation,
                    records: vec![fact.id],
                    applied_root_revision: root_e.revision_id,
                    binding: root.binding
                },
        "evidence acknowledgment mismatch"
    );
    Ok(receipt.acknowledgment)
}

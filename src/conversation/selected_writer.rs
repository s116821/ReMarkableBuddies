//! Selected supplied-text Writer transactions. No native/export authority.
use super::*;
use crate::storage::selection::{
    SelectedReadBudget, SelectedSnapshot, SelectionMutation, SelectionPublication, SelectionToken,
    SelectionTransaction,
};
use crate::storage::{MAX_METADATA, MAX_RECORD};
use serde::Deserialize;
mod regional;
pub(crate) use regional::RegionalImage;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub enum SelectedWriterSource {
    Generate {
        normalized: NormalizedTextSource,
    },
    GenerateRegion {
        regional: SelectedRegionalSource,
    },
    Refine {
        source: IntentRecordRef,
        draft: IntentRecordRef,
    },
}
/// Constructed only from the private selected regional resolver, never deserialized.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(transparent)]
pub struct SelectedRegionalSource {
    region: Box<target::RegionSource>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct SelectedWriterPrepare {
    pub operation: Uuid,
    pub root: IntentRecordRef,
    pub request: String,
    pub source: SelectedWriterSource,
}
#[derive(Clone, Debug)]
pub struct SelectedWriterPrepared {
    pub accepted: SelectionToken,
    pub root: IntentRecordRef,
    pub user: IntentRecordRef,
    pub source: IntentRecordRef,
    pub refined_draft: Option<IntentRecordRef>,
    pub preparation_fingerprint: String,
}
/// Cannot be cloned, serialized or minted from historical acknowledgment.
pub struct SelectedWriterModelPermit {
    owner: SelectedAdmission,
    fingerprint: String,
}
pub enum SelectedWriterPreparation {
    Fresh {
        prepared: Box<SelectedWriterPrepared>,
        model: SelectedWriterModelPermit,
    },
    Historical {
        transaction: Box<SelectionTransaction>,
        finished: Option<Box<SelectedWriterPublication>>,
    },
}
#[derive(Clone, Debug)]
pub struct SelectedWriterFinish {
    pub operation: Uuid,
    pub prepared: SelectedWriterPrepared,
    pub assistant: Uuid,
    pub outcome: Outcome,
    pub text: Option<String>,
    pub updated_ms: u64,
}
#[derive(Clone, Debug)]
pub struct SelectedWriterAcknowledgment {
    pub receipt: Receipt,
    pub root: IntentRecordRef,
    pub turn: IntentRecordRef,
    pub written: Turn,
}
pub enum SelectedWriterPublication {
    Published {
        publication: SelectionPublication,
        acknowledgment: Box<SelectedWriterAcknowledgment>,
    },
    Historical {
        transaction: SelectionTransaction,
        acknowledgment: Box<SelectedWriterAcknowledgment>,
    },
}
pub struct SelectedWriterInput {
    pub source: NormalizedTextSource,
    pub regional: Option<Box<target::RegionSource>>,
    pub prior: Option<Turn>,
    pub history: Vec<Turn>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedWriterImage {
    pub source: IntentRecordRef,
    pub turn: IntentRecordRef,
    pub ordinal: u32,
    pub page: u32,
}

impl SelectedWriterImage {
    pub(crate) fn validate(&self) -> Result<()> {
        self.source.validate()?;
        self.turn.validate()?;
        ensure!(
            self.source.namespace == Namespace::Source
                && self.turn.namespace == Namespace::Conversation
                && self.page > 0,
            "invalid selected Writer image reference"
        );
        Ok(())
    }
}

pub(crate) struct SelectedWriterImageBytes {
    pub document: Uuid,
    pub reference: ObjectRef,
    pub png: Vec<u8>,
}

pub(crate) struct WriterNormalizationPrepared {
    pub token: SelectionToken,
    pub root: IntentRecordRef,
}
fn projection(store: &Store, tx: &SelectionTransaction) -> Result<SelectedDomainProjection> {
    projection_with_reads(store, tx, None)
}
fn projection_with_reads(
    store: &Store,
    tx: &SelectionTransaction,
    mut reads: Option<&mut SelectedReadBudget>,
) -> Result<SelectedDomainProjection> {
    let records = tx
        .selected
        .records
        .iter()
        .map(|r| {
            if let Some(reads) = reads.as_deref_mut() {
                reads.charge(0, 1, r.bytes, 0)?;
            }
            serde_json::from_slice::<Envelope>(&store.read_object(r)?).map_err(Into::into)
        })
        .collect::<Result<Vec<_>>>()?;
    let p =
        SelectedDomainProjection::from_mixed_records(&records, &tx.selected.records, &tx.scope)?;
    p.document_ownership()?;
    Ok(p)
}
fn current<'a>(p: &'a SelectedDomainProjection, r: &IntentRecordRef) -> Result<&'a Record> {
    r.validate()?;
    let value = p.exact_reference(r)?;
    let (e, Some(_)) = p
        .head(r.namespace, r.record_id)
        .context("selected Writer head absent")?
    else {
        bail!("selected Writer head deleted")
    };
    ensure!(
        e.revision_id == r.revision_id,
        "selected Writer reference stale"
    );
    Ok(value)
}
fn record(
    operation: Uuid,
    actor: Uuid,
    parents: BTreeSet<Uuid>,
    value: Record,
) -> Result<Envelope> {
    let (id, namespace) = match &value {
        Record::Root(x) => (x.id, Namespace::Conversation),
        Record::Turn(x) => (x.id, Namespace::Conversation),
        Record::NormalizedText(x) => (x.id, Namespace::Source),
        Record::RegionSource(x) => (x.id, Namespace::Source),
        Record::Receipt(x) => (
            Ledger::receipt_id(x.acknowledgment.operation),
            Namespace::Conversation,
        ),
        _ => bail!("invalid selected Writer record"),
    };
    let media = match &value {
        Record::RegionSource(r) => r.media(),
        _ => vec![],
    };
    let regional = matches!(&value, Record::RegionSource(_));
    let e = Envelope {
        envelope_version: FORMAT,
        namespace,
        domain_schema_version: if regional {
            target::SCHEMA
        } else if namespace == Namespace::Source {
            TEXT_SOURCE_SCHEMA
        } else {
            SCHEMA
        },
        record_id: id,
        revision_id: Uuid::new_v5(
            &operation,
            format!("selected-writer:revision:{id}").as_bytes(),
        ),
        operation_id: Uuid::new_v5(
            &operation,
            format!("selected-writer:record:{id}").as_bytes(),
        ),
        actor_id: actor,
        parents,
        kind: Kind::Value,
        payload: serde_json::to_value(value)?,
        media_descriptors: media,
    };
    e.validate()?;
    Ledger::decode(&e)?;
    ensure!(
        serde_json::to_vec(&e)?.len() <= MAX_RECORD,
        "selected Writer record bound"
    );
    Ok(e)
}
fn root<'a>(p: &'a SelectedDomainProjection, reference: &IntentRecordRef) -> Result<&'a Root> {
    ensure!(
        reference.namespace == Namespace::Conversation,
        "Writer root namespace"
    );
    let Record::Root(root) = current(p, reference)? else {
        bail!("Writer root variant")
    };
    p.inspect(root.id)?;
    Ok(root)
}
fn source_input(
    p: &SelectedDomainProjection,
    req: &SelectedWriterPrepare,
) -> Result<SelectedWriterInput> {
    let root = root(p, &req.root)?;
    let mut history = Vec::new();
    for r in p.inspect(root.id)? {
        if let Record::Turn(t) = r {
            history.push(t);
        }
    }
    let mut regional = None;
    let (source, prior) = match &req.source {
        SelectedWriterSource::Generate { normalized } => {
            normalized.validate()?;
            ensure!(
                normalized.conversation == root.id
                    && normalized.turn == crate::writer::stage(req.operation, "user")
                    && p.head(Namespace::Source, normalized.id).is_none(),
                "selected Writer new source ownership"
            );
            (normalized.clone(), None)
        }
        SelectedWriterSource::GenerateRegion { regional: admitted } => {
            let value = &admitted.region;
            value.validate()?;
            ensure!(
                value.conversation == root.id
                    && value.turn == crate::writer::stage(req.operation, "user")
                    && p.head(Namespace::Source, value.id).is_none(),
                "regional Writer source ownership"
            );
            regional = Some(value.clone());
            (value.normalized.clone(), None)
        }
        SelectedWriterSource::Refine { source, draft } => {
            ensure!(
                source.namespace == Namespace::Source && draft.namespace == Namespace::Conversation,
                "selected Writer namespace"
            );
            let s = match current(p, source)? {
                Record::NormalizedText(s) => s.as_ref(),
                Record::RegionSource(r) => {
                    regional = Some(r.clone());
                    &r.normalized
                }
                _ => bail!("selected Writer source variant"),
            };
            let Record::Turn(t) = current(p, draft)? else {
                bail!("selected Writer draft variant")
            };
            s.validate()?;
            ensure!(
                s.conversation == root.id
                    && t.conversation == root.id
                    && t.role == Role::Assistant
                    && t.mode == Mode::Writer
                    && t.outcome == Outcome::Generated
                    && t.sources == vec![s.id],
                "selected Writer draft ownership"
            );
            (s.clone(), Some(t.clone()))
        }
    };
    ensure!(
        p.document_ownership()?
            .iter()
            .all(|(conversation, owner)| match owner {
                DocumentOwnership::Document(document) => *document == source.document.document,
                DocumentOwnership::DeferredMissing =>
                    *conversation == root.id && history.is_empty(),
                DocumentOwnership::DeferredAmbiguous(_) => false,
            }),
        "selected Writer document ownership unavailable or foreign"
    );
    Ok(SelectedWriterInput {
        regional,
        source,
        prior,
        history,
    })
}
fn validate_document(
    scope: &crate::storage::selection::SelectionScope,
    source: &NormalizedTextSource,
) -> Result<()> {
    let expected = crate::memory::catalog::document_key(source.document.document)?;
    ensure!(
        scope.key_sha256 == expected,
        "selected Writer document scope differs"
    );
    Ok(())
}
fn add_receipt(
    records: &mut Vec<Envelope>,
    operation: Uuid,
    actor: Uuid,
    fingerprint: &str,
    conversation: Uuid,
    binding: Option<Uuid>,
) -> Result<Receipt> {
    let receipt = Receipt {
        fingerprint: fingerprint.into(),
        acknowledgment: Acknowledgment {
            operation,
            conversation,
            records: records.iter().map(|e| e.record_id).collect(),
            applied_root_revision: records[0].revision_id,
            binding,
        },
        admitted_intent: None,
    };
    records.push(record(
        operation,
        actor,
        BTreeSet::new(),
        Record::Receipt(receipt.clone()),
    )?);
    ensure!(
        records
            .iter()
            .map(|e| serde_json::to_vec(e).map(|b| b.len()))
            .collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .sum::<usize>()
            <= MAX_METADATA,
        "selected Writer publication bound"
    );
    Ok(receipt)
}
fn prepare_records(
    store: &Store,
    tx: &SelectionTransaction,
    req: &SelectedWriterPrepare,
    fingerprint: &str,
    ms: u64,
    actor: Uuid,
) -> Result<(Vec<Envelope>, SelectedWriterInput)> {
    let p = projection(store, tx)?;
    prepare_records_from_projection(&p, tx, req, fingerprint, ms, actor)
}

fn prepare_records_from_projection(
    p: &SelectedDomainProjection,
    tx: &SelectionTransaction,
    req: &SelectedWriterPrepare,
    fingerprint: &str,
    ms: u64,
    actor: Uuid,
) -> Result<(Vec<Envelope>, SelectedWriterInput)> {
    let mut root = root(p, &req.root)?.clone();
    let input = source_input(p, req)?;
    validate_document(&tx.scope, &input.source)?;
    ensure!(
        !req.operation.is_nil()
            && !req.request.trim().is_empty()
            && req.request.len() <= 8192
            && !req.request.contains('\0'),
        "selected Writer request"
    );
    let user = Turn {
        id: crate::writer::stage(req.operation, "user"),
        conversation: root.id,
        exchange: req.operation,
        sequence: root.next_sequence,
        role: Role::User,
        mode: Mode::Writer,
        outcome: Outcome::Interpreted,
        text: Some(req.request.clone()),
        sources: vec![input.source.id],
        correction_of: input.prior.as_ref().map(|t| t.id),
        created_ms: ms,
        updated_ms: ms,
        completion: None,
    };
    ensure!(
        ms >= root.updated_ms && p.head(Namespace::Conversation, user.id).is_none(),
        "selected Writer chronology/replay"
    );
    Ledger::validate_turn(&user)?;
    root.next_sequence = root
        .next_sequence
        .checked_add(1)
        .context("sequence overflow")?;
    root.updated_ms = ms;
    let binding = root.binding;
    let conversation = root.id;
    let mut records = vec![record(
        req.operation,
        actor,
        BTreeSet::from([req.root.revision_id]),
        Record::Root(root),
    )?];
    if let Some(regional) = &input.regional {
        if !matches!(req.source, SelectedWriterSource::Refine { .. }) {
            records.push(record(
                req.operation,
                actor,
                BTreeSet::new(),
                Record::RegionSource(regional.clone()),
            )?);
        }
    } else if matches!(req.source, SelectedWriterSource::Generate { .. }) {
        records.push(record(
            req.operation,
            actor,
            BTreeSet::new(),
            Record::NormalizedText(Box::new(input.source.clone())),
        )?);
    }
    records.push(record(
        req.operation,
        actor,
        BTreeSet::new(),
        Record::Turn(user),
    )?);
    add_receipt(
        &mut records,
        req.operation,
        actor,
        fingerprint,
        conversation,
        binding,
    )?;
    Ok((records, input))
}
fn original(
    store: &Store,
    accepted: &SelectionToken,
    op: Uuid,
    fingerprint: &str,
) -> Result<
    Option<(
        SelectionTransaction,
        SelectionTransaction,
        SelectedDomainProjection,
        Receipt,
    )>,
> {
    original_with_reads(store, accepted, op, fingerprint, None)
}

fn original_with_reads(
    store: &Store,
    accepted: &SelectionToken,
    op: Uuid,
    fingerprint: &str,
    mut reads: Option<&mut SelectedReadBudget>,
) -> Result<
    Option<(
        SelectionTransaction,
        SelectionTransaction,
        SelectedDomainProjection,
        Receipt,
    )>,
> {
    let Some(tx) = store.selected_receipt_inner(accepted.scope(), op, reads.as_deref_mut())? else {
        return Ok(None);
    };
    ensure!(
        tx.mutation == SelectionMutation::Commit
            && tx.previous_sha256.as_deref() == Some(accepted.selection_sha256()),
        "selected Writer original selection differs"
    );
    let prev = store
        .selected_predecessor_inner(accepted.scope(), op, reads.as_deref_mut())?
        .context("Writer predecessor absent")?;
    ensure!(
        prev.store_generation == accepted.store_generation()
            && prev.aggregate_generation == accepted.aggregate_generation()
            && prev.accepted_base_sha256 == accepted.accepted_base_sha256()
            && prev.scope == *accepted.scope(),
        "selected Writer original token differs"
    );
    let p = projection_with_reads(store, &tx, reads)?;
    let (_, Some(Record::Receipt(receipt))) = p
        .head(Namespace::Conversation, Ledger::receipt_id(op))
        .context("Writer receipt absent")?
    else {
        bail!("Writer receipt variant")
    };
    ensure!(
        receipt.fingerprint == fingerprint && receipt.admitted_intent.is_none(),
        "Writer replay fingerprint differs"
    );
    let receipt = receipt.clone();
    Ok(Some((tx, prev, p, receipt)))
}
fn verify_records(p: &SelectedDomainProjection, records: &[Envelope]) -> Result<()> {
    for e in records {
        ensure!(
            p.exact_reference(&pending::pin(e)?)? == &Ledger::decode(e)?,
            "selected Writer historical records differ"
        );
    }
    Ok(())
}
impl SelectedWriterModelPermit {
    pub(crate) fn consume(
        self,
        writer: &SelectedAdmission,
        prepared: &SelectedWriterPrepared,
    ) -> Result<()> {
        self.require_prepared(writer, prepared)?;
        writer.writer_input(prepared)?;
        Ok(())
    }

    pub(crate) fn require_prepared(
        &self,
        writer: &SelectedAdmission,
        prepared: &SelectedWriterPrepared,
    ) -> Result<()> {
        ensure!(
            self.owner.uses_same_admission(writer)
                && self.fingerprint == prepared_identity(prepared)?,
            "foreign Writer model permit"
        );
        Ok(())
    }
}
impl SelectedAdmission {
    pub(crate) fn selected_writer_image_locked(
        &self,
        store: &Store,
        token: &SelectionToken,
        root_ref: &IntentRecordRef,
        image: &SelectedWriterImage,
        reads: &mut SelectedReadBudget,
    ) -> Result<SelectedWriterImageBytes> {
        image.validate()?;
        ensure!(
            std::ptr::eq(store, self.writer_store().as_ref()) && token.scope() == self.scope(),
            "foreign selected Writer image"
        );
        let mut snapshot = self.writer_snapshot(store, Some(reads))?;
        ensure!(
            snapshot.token == *token,
            "selected Writer image selection changed"
        );
        let projection = SelectedDomainProjection::from_snapshot(&snapshot)?;
        let root = root(&projection, root_ref)?;
        let Record::Turn(turn) = current(&projection, &image.turn)? else {
            bail!("selected Writer image turn differs");
        };
        ensure!(
            turn.conversation == root.id
                && turn.role == Role::User
                && turn.sources.contains(&image.source.record_id),
            "selected Writer image turn link differs"
        );
        let (document, media) = match current(&projection, &image.source)? {
            Record::Source(_) => bail!("selected Writer source has no retained numeric page proof"),
            Record::ResidentCapture(source)
                if source.turn == turn.id && source.conversation == root.id =>
            {
                let media = source
                    .images
                    .iter()
                    .find(|item| item.provider_ordinal == image.ordinal)
                    .context("selected Writer capture ordinal absent")?;
                ensure!(
                    source.before.order.get((image.page - 1) as usize) == Some(&source.before.page),
                    "selected Writer capture page differs"
                );
                (source.before.document, &media.media)
            }
            _ => bail!("selected Writer source image correspondence differs"),
        };
        ensure!(
            crate::memory::catalog::document_key(document)? == self.scope().key_sha256,
            "selected Writer image document differs"
        );
        let reference = ObjectRef {
            sha256: media.sha256.clone(),
            bytes: media.bytes,
        };
        ensure!(
            reference.bytes <= 8 * 1024 * 1024,
            "selected Writer image exceeds bound"
        );
        let png = super::notebook::selected::read_selected_media(&mut snapshot, &reference, reads)?;
        Ok(SelectedWriterImageBytes {
            document,
            reference,
            png,
        })
    }

    pub(crate) fn prepare_writer_normalization_locked(
        &self,
        store: &Store,
        selection: (&SelectionToken, &IntentRecordRef),
        request: (Uuid, &str, u64),
        reads: &mut SelectedReadBudget,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<WriterNormalizationPrepared> {
        progress()?;
        let (operation, fingerprint, timestamp) = request;
        let (accepted, root_ref) = selection;
        ensure!(
            std::ptr::eq(store, self.writer_store().as_ref()) && accepted.scope() == self.scope(),
            "foreign Writer normalization selection"
        );
        ensure!(
            store
                .selected_receipt_with_budget(self.scope(), operation, reads)?
                .is_none(),
            "Writer normalization already has an original preparation"
        );
        let snapshot = self.writer_snapshot(store, Some(reads))?;
        ensure!(
            snapshot.token == *accepted,
            "Writer normalization selection changed"
        );
        let projection = SelectedDomainProjection::from_snapshot(&snapshot)?;
        let mut root = root(&projection, root_ref)?.clone();
        ensure!(
            timestamp >= root.updated_ms,
            "Writer normalization chronology"
        );
        let turn = Turn {
            id: crate::writer::stage(operation, "source-normalization"),
            conversation: root.id,
            exchange: operation,
            sequence: root.next_sequence,
            role: Role::User,
            mode: Mode::Writer,
            outcome: Outcome::Prepared,
            text: None,
            sources: Vec::new(),
            correction_of: None,
            created_ms: timestamp,
            updated_ms: timestamp,
            completion: None,
        };
        root.next_sequence = root
            .next_sequence
            .checked_add(1)
            .context("Writer sequence overflow")?;
        root.updated_ms = timestamp;
        let (conversation, binding) = (root.id, root.binding);
        let mut records = vec![
            record(
                operation,
                store.actor_id,
                BTreeSet::from([root_ref.revision_id]),
                Record::Root(root),
            )?,
            record(
                operation,
                store.actor_id,
                BTreeSet::new(),
                Record::Turn(turn),
            )?,
        ];
        add_receipt(
            &mut records,
            operation,
            store.actor_id,
            fingerprint,
            conversation,
            binding,
        )?;
        let root = pending::pin(&records[0])?;
        let expected = records
            .iter()
            .map(pending::pin)
            .collect::<Result<Vec<_>>>()?;
        progress()?;
        let publication = store.commit_selected(accepted, operation, records, BTreeMap::new())?;
        let original = store
            .selected_receipt_with_budget(self.scope(), operation, reads)?
            .context("Writer normalization preparation acknowledgment absent")?;
        ensure!(
            !publication.replayed
                && original.request_sha256 == publication.transaction.request_sha256
                && original.previous_sha256.as_deref() == Some(accepted.selection_sha256())
                && expected
                    .iter()
                    .all(|record| original.selected.records.contains(&record.object)),
            "Writer normalization preparation acknowledgment differs"
        );
        Ok(WriterNormalizationPrepared {
            token: publication.token,
            root,
        })
    }

    pub(crate) fn recover_writer_normalization_locked(
        &self,
        store: &Store,
        operation: Uuid,
        evidence: &IntentSelectionEvidence,
        raw_head: &str,
        fingerprint: &str,
        reads: &mut SelectedReadBudget,
    ) -> Result<Option<SelectionTransaction>> {
        ensure!(
            std::ptr::eq(store, self.writer_store().as_ref()),
            "foreign Writer Store"
        );
        let Some(original) = store.selected_receipt_with_budget(self.scope(), operation, reads)?
        else {
            return Ok(None);
        };
        let predecessor = store
            .historical_predecessor_token_inner(self.scope(), operation, Some(reads))?
            .context("Writer normalization predecessor absent")?;
        ensure!(
            original.mutation == SelectionMutation::Commit
                && original.previous_sha256.as_deref() == Some(predecessor.selection_sha256())
                && IntentSelectionEvidence::from_token(&predecessor) == *evidence,
            "Writer normalization original selection differs"
        );
        let read = store
            .selected_snapshot_with_budget(self.scope(), MAX_ITEMS, reads)?
            .context("Writer normalization lineage absent")?;
        let parent = read
            .accepted_history
            .iter()
            .find(|entry| entry.sha256 == raw_head)
            .context("Writer normalization original raw head absent")?;
        ensure!(
            digest(&serde_json::to_vec(&parent.transaction)?) == predecessor.selection_sha256(),
            "Writer normalization original raw head differs"
        );
        let entry = read
            .accepted_history
            .iter()
            .find(|item| item.transaction.operation == operation)
            .context("Writer normalization original history absent")?;
        let projection = SelectedDomainProjection::from_mixed_records(
            &entry.selected_records,
            &entry.transaction.selected.records,
            self.scope(),
        )?;
        ensure!(
            matches!(
                projection.head(Namespace::Conversation, Ledger::receipt_id(operation)),
                Some((_, Some(Record::Receipt(receipt)))) if receipt.fingerprint == fingerprint
                    && receipt.acknowledgment.operation == operation && receipt.admitted_intent.is_none()
            ),
            "Writer normalization original request differs"
        );
        Ok(Some(original))
    }

    /// Evidence-only recovery. Never writes or mints a fresh model permit.
    pub fn recover_selected_writer_evidence(
        &self,
        evidence: &IntentSelectionEvidence,
        raw_head: &str,
        req: &SelectedWriterPrepare,
    ) -> Result<Option<SelectedWriterPreparation>> {
        evidence.validate()?;
        ensure!(
            crate::storage::valid_digest(raw_head),
            "invalid original raw head"
        );
        // Historical custody never mints a model permit or current readiness.
        // Keep original-only recovery readable across mode/readiness changes.
        Self::with_gates(&[self], |store| {
            if store
                .selected_receipt(self.scope(), req.operation)?
                .is_none()
            {
                return Ok(None);
            }
            let token = store
                .historical_predecessor_token(self.scope(), req.operation)?
                .context("Writer predecessor token absent")?;
            ensure!(
                IntentSelectionEvidence::from_token(&token) == *evidence,
                "Writer original evidence differs"
            );
            let mut budget = crate::storage::selection::SelectedReadBudget::new(
                64,
                4096,
                8 * 1024 * 1024,
                32 * 1024 * 1024,
            );
            let read = store
                .selected_snapshot_with_budget(self.scope(), MAX_ITEMS, &mut budget)?
                .context("Writer lineage absent")?;
            let predecessor = read
                .accepted_history
                .iter()
                .find(|item| item.sha256 == raw_head)
                .context("Writer original raw head absent")?;
            ensure!(
                predecessor.transaction.scope == *token.scope()
                    && predecessor.transaction.store_generation == token.store_generation()
                    && crate::storage::digest(&serde_json::to_vec(&predecessor.transaction)?)
                        == token.selection_sha256(),
                "Writer original raw head differs"
            );
            let fingerprint = Ledger::fingerprint(&("selected-writer-prepare-v1", evidence, req))?;
            historical_preparation(store, &token, req, &fingerprint)
        })
    }
    pub(crate) fn uses_same_admission(&self, other: &Self) -> bool {
        self.scope() == other.scope() && self.same_store(other)
    }
    pub fn selected_writer_input(
        &self,
        accepted: &SelectionToken,
        req: &SelectedWriterPrepare,
    ) -> Result<SelectedWriterInput> {
        self.with_current_store(accepted, |store, s| {
            let input = source_input(&SelectedDomainProjection::from_snapshot(s)?, req)?;
            validate_document(self.scope(), &input.source)?;
            reader_uncertainty::validate_document(store, s, input.source.document.document, None)?;
            Ok(input)
        })
    }
    pub fn prepare_selected_writer(
        &self,
        accepted: &SelectionToken,
        req: SelectedWriterPrepare,
        ms: u64,
    ) -> Result<SelectedWriterPreparation> {
        ensure!(accepted.scope() == self.scope(), "foreign Writer selection");
        let fingerprint = Ledger::fingerprint(&(
            "selected-writer-prepare-v1",
            IntentSelectionEvidence::from_token(accepted),
            &req,
        ))?;
        Self::with_ready_gate_set(&[self], |store| {
            if let Some(history) = historical_preparation(store, accepted, &req, &fingerprint)? {
                return Ok(history);
            }
            let _mode = self.reader_mode_lease()?;
            self.prepare_writer_current(store, accepted, &req, (&fingerprint, ms), None, || {
                Ok(BTreeMap::new())
            })
        })
    }

    pub(crate) fn prepare_normalized_writer_locked(
        &self,
        store: &Store,
        accepted: &SelectionToken,
        req: &SelectedWriterPrepare,
        ms: u64,
        reads: &mut SelectedReadBudget,
    ) -> Result<SelectedWriterPreparation> {
        ensure!(
            store
                .selected_receipt_with_budget(self.scope(), req.operation, reads)?
                .is_none(),
            "Writer normalization result is already historical"
        );
        let fingerprint = Ledger::fingerprint(&(
            "selected-writer-prepare-v1",
            IntentSelectionEvidence::from_token(accepted),
            req,
        ))?;
        self.prepare_writer_current(
            store,
            accepted,
            req,
            (&fingerprint, ms),
            Some(reads),
            || Ok(BTreeMap::new()),
        )
    }

    fn prepare_writer_current(
        &self,
        store: &Store,
        accepted: &SelectionToken,
        req: &SelectedWriterPrepare,
        metadata: (&str, u64),
        mut reads: Option<&mut SelectedReadBudget>,
        objects: impl FnOnce() -> Result<BTreeMap<String, Vec<u8>>>,
    ) -> Result<SelectedWriterPreparation> {
        let (fingerprint, ms) = metadata;
        ensure!(
            std::ptr::eq(store, self.writer_store().as_ref()) && accepted.scope() == self.scope(),
            "foreign Writer selection"
        );
        ensure!(store.unavailable_commits()? == 0, "Writer store incomplete");
        let snapshot = self.writer_snapshot(store, reads.as_deref_mut())?;
        ensure!(snapshot.token == *accepted, "Writer selection replaced");
        let projection = SelectedDomainProjection::from_snapshot(&snapshot)?;
        let (records, input) = prepare_records_from_projection(
            &projection,
            &snapshot.transaction,
            req,
            fingerprint,
            ms,
            store.actor_id,
        )?;
        reader_uncertainty::validate_document_with_reads(
            store,
            &snapshot,
            input.source.document.document,
            None,
            reads.as_deref_mut(),
        )?;
        // Unknown commit acknowledgment cannot mint a fresh model permit.
        let objects = objects()?;
        let published = store.commit_selected(accepted, req.operation, records.clone(), objects)?;
        ensure!(!published.replayed, "Writer preparation replay");
        let after = self.writer_snapshot(store, reads)?;
        ensure!(
            after.token == published.token,
            "Writer post-preparation selection replaced"
        );
        if let Some(receipt) = records
            .last()
            .filter(|_| after.transaction.operation == req.operation)
        {
            ensure!(
                after
                    .transaction
                    .selected
                    .records
                    .contains(&pending::pin(receipt)?.object),
                "Writer preparation receipt absent"
            );
        } else {
            bail!("Writer preparation operation absent");
        }
        let p = SelectedDomainProjection::from_snapshot(&after)?;
        let pin = |ns, id| -> Result<IntentRecordRef> {
            let (e, Some(_)) = p
                .head(ns, id)
                .context("Writer acknowledged reference absent")?
            else {
                bail!("Writer reference deleted")
            };
            pending::pin(e)
        };
        let refined_draft = match &req.source {
            SelectedWriterSource::Refine { draft, .. } => Some(draft.clone()),
            _ => None,
        };
        let prepared = SelectedWriterPrepared {
            accepted: after.token,
            root: pin(Namespace::Conversation, req.root.record_id)?,
            user: pin(
                Namespace::Conversation,
                crate::writer::stage(req.operation, "user"),
            )?,
            source: match &req.source {
                SelectedWriterSource::Refine { source, .. } => source.clone(),
                SelectedWriterSource::Generate { .. }
                | SelectedWriterSource::GenerateRegion { .. } => {
                    pin(Namespace::Source, input.source.id)?
                }
            },
            refined_draft,
            preparation_fingerprint: fingerprint.into(),
        };
        let identity = prepared_identity(&prepared)?;
        Ok(SelectedWriterPreparation::Fresh {
            prepared: Box::new(prepared),
            model: SelectedWriterModelPermit {
                owner: self.clone(),
                fingerprint: identity,
            },
        })
    }
    fn writer_snapshot(
        &self,
        store: &Store,
        reads: Option<&mut SelectedReadBudget>,
    ) -> Result<SelectedSnapshot> {
        match reads {
            Some(reads) => store
                .selected_snapshot_with_budget(self.scope(), MAX_ITEMS, reads)?
                .map(|read| read.snapshot),
            None => store.selected_snapshot(self.scope(), MAX_ITEMS)?,
        }
        .context("Writer scope absent")
    }
    pub fn writer_input(&self, prepared: &SelectedWriterPrepared) -> Result<SelectedWriterInput> {
        self.with_current(&prepared.accepted, |snapshot| {
            prepared_input(
                &SelectedDomainProjection::from_snapshot(snapshot)?,
                prepared,
            )
        })
    }
    /// The caller holds this Writer's complete ordered ready gate set. Keep
    /// locked validation separate so joined handoff never reacquires its gate.
    pub(crate) fn writer_input_locked(
        &self,
        store: &Store,
        prepared: &SelectedWriterPrepared,
    ) -> Result<SelectedWriterInput> {
        self.writer_input_with_reads_locked(store, prepared, None)
    }

    pub(crate) fn writer_input_with_reads_locked(
        &self,
        store: &Store,
        prepared: &SelectedWriterPrepared,
        mut reads: Option<&mut SelectedReadBudget>,
    ) -> Result<SelectedWriterInput> {
        ensure!(
            std::ptr::eq(store, self.writer_store().as_ref())
                && prepared.accepted.scope() == self.scope(),
            "foreign locked Writer input"
        );
        let read = self.writer_snapshot(store, reads.as_deref_mut())?;
        ensure!(
            read.token == prepared.accepted,
            "Writer preparation replaced"
        );
        let input = prepared_input(&SelectedDomainProjection::from_snapshot(&read)?, prepared)?;
        reader_uncertainty::validate_document_with_reads(
            store,
            &read,
            input.source.document.document,
            None,
            reads,
        )?;
        Ok(input)
    }

    pub(crate) fn writer_context_with_reads_locked(
        &self,
        store: &Store,
        prepared: &SelectedWriterPrepared,
        budget: &ContextBudget,
        count: impl Fn(&[Turn]) -> Result<usize>,
        reads: &mut SelectedReadBudget,
    ) -> Result<ContextView> {
        let snapshot = self.writer_snapshot(store, Some(reads))?;
        ensure!(
            snapshot.token == prepared.accepted,
            "Writer preparation replaced"
        );
        Self::writer_context_locked(&snapshot, prepared.root.record_id, budget, count)
    }
}
fn recover_completed(
    store: &Store,
    prepared: &SelectedWriterPrepared,
    operation: Uuid,
) -> Result<Option<SelectedWriterPublication>> {
    let finish_op = crate::writer::stage(operation, "finish");
    let Some(tx) = store.selected_receipt(prepared.accepted.scope(), finish_op)? else {
        return Ok(None);
    };
    let p = projection(store, &tx)?;
    let assistant = crate::writer::stage(operation, "draft");
    let (_, Some(Record::Turn(turn))) = p
        .head(Namespace::Conversation, assistant)
        .context("Writer finished turn absent")?
    else {
        bail!("Writer finished variant")
    };
    let f = SelectedWriterFinish {
        operation: finish_op,
        prepared: prepared.clone(),
        assistant,
        outcome: turn.outcome.clone(),
        text: turn.text.clone(),
        updated_ms: turn.created_ms,
    };
    let Some((transaction, prev, actual, _)) = original(
        store,
        &prepared.accepted,
        finish_op,
        &finish_fingerprint(&f)?,
    )?
    else {
        bail!("Writer finished receipt absent")
    };
    let (records, ack) = finish_records(store, &prev, &f, turn.created_ms, store.actor_id)?;
    verify_records(&actual, &records)?;
    Ok(Some(SelectedWriterPublication::Historical {
        transaction,
        acknowledgment: Box::new(ack),
    }))
}
fn prepared_identity(p: &SelectedWriterPrepared) -> Result<String> {
    Ledger::fingerprint(&(
        IntentSelectionEvidence::from_token(&p.accepted),
        &p.root,
        &p.user,
        &p.source,
        &p.refined_draft,
        &p.preparation_fingerprint,
    ))
}
fn prepared_input(
    p: &SelectedDomainProjection,
    prepared: &SelectedWriterPrepared,
) -> Result<SelectedWriterInput> {
    let root = root(p, &prepared.root)?;
    let Record::Turn(user) = current(p, &prepared.user)? else {
        bail!("Writer prepared user variant")
    };
    let (source, regional) = match current(p, &prepared.source)? {
        Record::NormalizedText(s) => (s.as_ref(), None),
        Record::RegionSource(r) => (&r.normalized, Some(r.clone())),
        _ => bail!("Writer prepared source variant"),
    };
    let (_, Some(Record::Receipt(receipt))) = p
        .head(Namespace::Conversation, Ledger::receipt_id(user.exchange))
        .context("Writer preparation receipt absent")?
    else {
        bail!("Writer preparation receipt variant")
    };
    ensure!(
        receipt.fingerprint == prepared.preparation_fingerprint
            && receipt.acknowledgment.applied_root_revision == prepared.root.revision_id
            && receipt.acknowledgment.conversation == root.id
            && receipt.acknowledgment.records.contains(&user.id)
            && user.id == crate::writer::stage(user.exchange, "user"),
        "Writer preparation receipt differs"
    );
    source.validate()?;
    ensure!(
        user.conversation == root.id
            && user.role == Role::User
            && user.mode == Mode::Writer
            && user.outcome == Outcome::Interpreted
            && user.text.is_some()
            && user.sources == vec![source.id]
            && source.conversation == root.id,
        "Writer prepared ownership"
    );
    let prior = if let Some(reference) = &prepared.refined_draft {
        let Record::Turn(t) = current(p, reference)? else {
            bail!("Writer prior variant")
        };
        ensure!(
            t.role == Role::Assistant
                && t.mode == Mode::Writer
                && t.outcome == Outcome::Generated
                && t.conversation == root.id
                && t.sources == vec![source.id]
                && user.correction_of == Some(t.id),
            "Writer prior ownership"
        );
        Some(t.clone())
    } else {
        ensure!(
            user.correction_of.is_none() && source.turn == user.id,
            "Writer generated source owner"
        );
        None
    };
    let history = p
        .inspect(root.id)?
        .into_iter()
        .filter_map(|r| {
            if let Record::Turn(t) = r {
                (t.sequence < user.sequence).then_some(t)
            } else {
                None
            }
        })
        .collect();
    Ok(SelectedWriterInput {
        source: source.clone(),
        regional,
        prior,
        history,
    })
}
fn finish_fingerprint(f: &SelectedWriterFinish) -> Result<String> {
    Ledger::fingerprint(&(
        "selected-writer-finish-v1",
        IntentSelectionEvidence::from_token(&f.prepared.accepted),
        &f.prepared.root,
        &f.prepared.user,
        &f.prepared.source,
        &f.prepared.refined_draft,
        &f.prepared.preparation_fingerprint,
        f.operation,
        f.assistant,
        &f.outcome,
        &f.text,
    ))
}
fn finish_records(
    store: &Store,
    tx: &SelectionTransaction,
    f: &SelectedWriterFinish,
    ms: u64,
    actor: Uuid,
) -> Result<(Vec<Envelope>, SelectedWriterAcknowledgment)> {
    let p = projection(store, tx)?;
    finish_records_from_projection(&p, tx, f, ms, actor)
}
fn finish_records_from_projection(
    p: &SelectedDomainProjection,
    tx: &SelectionTransaction,
    f: &SelectedWriterFinish,
    ms: u64,
    actor: Uuid,
) -> Result<(Vec<Envelope>, SelectedWriterAcknowledgment)> {
    let input = prepared_input(p, &f.prepared)?;
    validate_document(&tx.scope, &input.source)?;
    let mut root = root(p, &f.prepared.root)?.clone();
    let Record::Turn(user) = current(p, &f.prepared.user)? else {
        unreachable!()
    };
    ensure!(
        f.operation == crate::writer::stage(user.exchange, "finish")
            && f.assistant == crate::writer::stage(user.exchange, "draft")
            && p.head(Namespace::Conversation, f.assistant).is_none()
            && ms >= root.updated_ms,
        "Writer finish identity/chronology"
    );
    ensure!(
        matches!(
            &f.outcome,
            Outcome::Generated | Outcome::Failed | Outcome::Canceled
        ) && (if f.outcome == Outcome::Generated {
            f.text
                .as_ref()
                .is_some_and(|s| !s.is_empty() && s.len() <= MAX_RECORD)
        } else {
            f.text.is_none()
        }),
        "Writer finish outcome"
    );
    let written = Turn {
        id: f.assistant,
        conversation: root.id,
        exchange: user.exchange,
        sequence: root.next_sequence,
        role: Role::Assistant,
        mode: Mode::Writer,
        outcome: f.outcome.clone(),
        text: f.text.clone(),
        sources: vec![input.source.id],
        correction_of: None,
        created_ms: ms,
        updated_ms: ms,
        completion: None,
    };
    Ledger::validate_turn(&written)?;
    root.next_sequence = root
        .next_sequence
        .checked_add(1)
        .context("Writer sequence overflow")?;
    root.updated_ms = ms;
    let conversation = root.id;
    let binding = root.binding;
    let mut records = vec![
        record(
            f.operation,
            actor,
            BTreeSet::from([f.prepared.root.revision_id]),
            Record::Root(root),
        )?,
        record(
            f.operation,
            actor,
            BTreeSet::new(),
            Record::Turn(written.clone()),
        )?,
    ];
    let receipt = add_receipt(
        &mut records,
        f.operation,
        actor,
        &finish_fingerprint(f)?,
        conversation,
        binding,
    )?;
    let acknowledgment = SelectedWriterAcknowledgment {
        receipt,
        root: pending::pin(&records[0])?,
        turn: pending::pin(&records[1])?,
        written,
    };
    Ok((records, acknowledgment))
}
impl SelectedAdmission {
    pub fn finish_selected_writer(
        &self,
        f: SelectedWriterFinish,
    ) -> Result<SelectedWriterPublication> {
        self.finish_selected_writer_joined(f, &[], |_| Ok(()))
    }
    pub(crate) fn finish_selected_writer_joined(
        &self,
        f: SelectedWriterFinish,
        additional: &[&SelectedAdmission],
        verify_new: impl FnOnce(&Store) -> Result<()>,
    ) -> Result<SelectedWriterPublication> {
        let mut admissions = vec![self];
        admissions.extend_from_slice(additional);
        Self::with_ready_gate_set(&admissions, |store| {
            self.finish_writer_locked(store, f, None, verify_new)
        })
    }

    pub(crate) fn finish_writer_locked(
        &self,
        store: &Store,
        f: SelectedWriterFinish,
        reads: Option<&mut SelectedReadBudget>,
        verify_new: impl FnOnce(&Store) -> Result<()>,
    ) -> Result<SelectedWriterPublication> {
        self.finish_writer_guarded_locked(store, f, reads, verify_new, || Ok(()))
    }

    pub(crate) fn finish_writer_guarded_locked(
        &self,
        store: &Store,
        f: SelectedWriterFinish,
        mut reads: Option<&mut SelectedReadBudget>,
        verify_new: impl FnOnce(&Store) -> Result<()>,
        before_commit: impl FnOnce() -> Result<()>,
    ) -> Result<SelectedWriterPublication> {
        ensure!(
            std::ptr::eq(store, self.writer_store().as_ref())
                && f.prepared.accepted.scope() == self.scope(),
            "foreign Writer finish scope"
        );
        let fingerprint = finish_fingerprint(&f)?;
        let recover =
            |store: &Store,
             mut reads: Option<&mut SelectedReadBudget>|
             -> Result<Option<(SelectionTransaction, SelectedWriterAcknowledgment)>> {
                let Some((tx, prev, p, _)) = original_with_reads(
                    store,
                    &f.prepared.accepted,
                    f.operation,
                    &fingerprint,
                    reads.as_deref_mut(),
                )?
                else {
                    return Ok(None);
                };
                let (_, Some(Record::Turn(turn))) = p
                    .head(Namespace::Conversation, f.assistant)
                    .context("Writer historical assistant absent")?
                else {
                    bail!("Writer historical assistant variant")
                };
                let previous = projection_with_reads(store, &prev, reads)?;
                let (records, ack) = finish_records_from_projection(
                    &previous,
                    &prev,
                    &f,
                    turn.created_ms,
                    store.actor_id,
                )?;
                verify_records(&p, &records)?;
                Ok(Some((tx, ack)))
            };
        if let Some((transaction, ack)) = recover(store, reads.as_deref_mut())? {
            return Ok(SelectedWriterPublication::Historical {
                transaction,
                acknowledgment: Box::new(ack),
            });
        }
        verify_new(store)?;
        ensure!(store.unavailable_commits()? == 0, "Writer incomplete Store");
        let _mode = self.reader_mode_lease()?;
        let snapshot = self.writer_snapshot(store, reads.as_deref_mut())?;
        ensure!(
            snapshot.token == f.prepared.accepted,
            "Writer preparation replaced"
        );
        let projection = SelectedDomainProjection::from_snapshot(&snapshot)?;
        let input = prepared_input(&projection, &f.prepared)?;
        reader_uncertainty::validate_document_with_reads(
            store,
            &snapshot,
            input.source.document.document,
            None,
            reads.as_deref_mut(),
        )?;
        let (records, _) = finish_records_from_projection(
            &projection,
            &snapshot.transaction,
            &f,
            f.updated_ms,
            store.actor_id,
        )?;
        before_commit()?;
        let result =
            store.commit_selected(&f.prepared.accepted, f.operation, records, BTreeMap::new());
        let recovered = recover(store, reads)?;
        match (result, recovered) {
            (Ok(publication), Some((_, ack))) if !publication.replayed => {
                Ok(SelectedWriterPublication::Published {
                    publication,
                    acknowledgment: Box::new(ack),
                })
            }
            (_, Some((transaction, ack))) => Ok(SelectedWriterPublication::Historical {
                transaction,
                acknowledgment: Box::new(ack),
            }),
            (Err(e), None) => Err(e),
            _ => bail!("Writer acknowledgment unavailable"),
        }
    }
}

fn historical_preparation(
    store: &Store,
    accepted: &SelectionToken,
    req: &SelectedWriterPrepare,
    fingerprint: &str,
) -> Result<Option<SelectedWriterPreparation>> {
    if let Some((tx, prev, p, _)) = original(store, accepted, req.operation, fingerprint)? {
        let (_, Some(Record::Turn(user))) = p
            .head(
                Namespace::Conversation,
                crate::writer::stage(req.operation, "user"),
            )
            .context("historical Writer user absent")?
        else {
            bail!("historical Writer user variant")
        };
        let (records, _) = prepare_records(
            store,
            &prev,
            req,
            fingerprint,
            user.created_ms,
            store.actor_id,
        )?;
        verify_records(&p, &records)?;
        let token = store
            .historical_selection_token(accepted.scope(), req.operation)?
            .context("Writer original token absent")?;
        let pin = |ns, id| -> Result<IntentRecordRef> {
            let (e, Some(_)) = p.head(ns, id).context("Writer original reference absent")? else {
                bail!("Writer original reference deleted")
            };
            pending::pin(e)
        };
        let source = match &req.source {
            SelectedWriterSource::Generate { normalized } => pin(Namespace::Source, normalized.id)?,
            SelectedWriterSource::GenerateRegion { regional } => {
                pin(Namespace::Source, regional.region.id)?
            }
            SelectedWriterSource::Refine { source, .. } => source.clone(),
        };
        let refined_draft = match &req.source {
            SelectedWriterSource::Refine { draft, .. } => Some(draft.clone()),
            _ => None,
        };
        let prepared = SelectedWriterPrepared {
            accepted: token,
            root: pin(Namespace::Conversation, req.root.record_id)?,
            user: pin(
                Namespace::Conversation,
                crate::writer::stage(req.operation, "user"),
            )?,
            source,
            refined_draft,
            preparation_fingerprint: fingerprint.to_owned(),
        };
        let finished = recover_completed(store, &prepared, req.operation)?;
        return Ok(Some(SelectedWriterPreparation::Historical {
            transaction: Box::new(tx),
            finished: finished.map(Box::new),
        }));
    }
    Ok(None)
}

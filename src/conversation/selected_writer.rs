//! Selected supplied-text Writer transactions. No native/export authority.
use super::*;
use crate::storage::selection::{
    SelectionMutation, SelectionPublication, SelectionToken, SelectionTransaction,
};
use crate::storage::{MAX_METADATA, MAX_RECORD};

#[derive(Clone, Debug, Serialize, PartialEq)]
pub enum SelectedWriterSource {
    Generate {
        normalized: NormalizedTextSource,
    },
    Refine {
        source: IntentRecordRef,
        draft: IntentRecordRef,
    },
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
    pub prior: Option<Turn>,
    pub history: Vec<Turn>,
}
fn projection(store: &Store, tx: &SelectionTransaction) -> Result<SelectedDomainProjection> {
    let records = tx
        .selected
        .records
        .iter()
        .map(|r| serde_json::from_slice::<Envelope>(&store.read_object(r)?).map_err(Into::into))
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
        Record::Receipt(x) => (
            Ledger::receipt_id(x.acknowledgment.operation),
            Namespace::Conversation,
        ),
        _ => bail!("invalid selected Writer record"),
    };
    let e = Envelope {
        envelope_version: FORMAT,
        namespace,
        domain_schema_version: if namespace == Namespace::Source {
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
        media_descriptors: vec![],
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
        SelectedWriterSource::Refine { source, draft } => {
            ensure!(
                source.namespace == Namespace::Source && draft.namespace == Namespace::Conversation,
                "selected Writer namespace"
            );
            let Record::NormalizedText(s) = current(p, source)? else {
                bail!("selected Writer source variant")
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
            (s.as_ref().clone(), Some(t.clone()))
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
    let mut root = root(&p, &req.root)?.clone();
    let input = source_input(&p, req)?;
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
    if matches!(req.source, SelectedWriterSource::Generate { .. }) {
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
    let Some(tx) = store.selected_receipt(accepted.scope(), op)? else {
        return Ok(None);
    };
    ensure!(
        tx.mutation == SelectionMutation::Commit
            && tx.previous_sha256.as_deref() == Some(accepted.selection_sha256()),
        "selected Writer original selection differs"
    );
    let prev = store
        .selected_predecessor(accepted.scope(), op)?
        .context("Writer predecessor absent")?;
    ensure!(
        prev.store_generation == accepted.store_generation()
            && prev.aggregate_generation == accepted.aggregate_generation()
            && prev.accepted_base_sha256 == accepted.accepted_base_sha256()
            && prev.scope == *accepted.scope(),
        "selected Writer original token differs"
    );
    let p = projection(store, &tx)?;
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
        ensure!(
            self.owner.uses_same_admission(writer)
                && self.fingerprint == prepared_identity(prepared)?,
            "foreign Writer model permit"
        );
        writer.writer_input(prepared)?;
        Ok(())
    }
}
impl SelectedAdmission {
    pub(crate) fn uses_same_admission(&self, other: &Self) -> bool {
        self.scope() == other.scope() && self.same_store(other)
    }
    pub fn selected_writer_input(
        &self,
        accepted: &SelectionToken,
        req: &SelectedWriterPrepare,
    ) -> Result<SelectedWriterInput> {
        self.with_current(accepted, |s| {
            source_input(&SelectedDomainProjection::from_snapshot(s)?, req)
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
        Self::with_gates(&[self], |store| {
            if let Some((tx, prev, p, _)) = original(store, accepted, req.operation, &fingerprint)?
            {
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
                    &req,
                    &fingerprint,
                    user.created_ms,
                    store.actor_id,
                )?;
                verify_records(&p, &records)?;
                let token = store
                    .historical_selection_token(self.scope(), req.operation)?
                    .context("Writer original token absent")?;
                let pin = |ns, id| -> Result<IntentRecordRef> {
                    let (e, Some(_)) =
                        p.head(ns, id).context("Writer original reference absent")?
                    else {
                        bail!("Writer original reference deleted")
                    };
                    pending::pin(e)
                };
                let source = match &req.source {
                    SelectedWriterSource::Generate { normalized } => {
                        pin(Namespace::Source, normalized.id)?
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
                    preparation_fingerprint: fingerprint.clone(),
                };
                let finished = recover_completed(store, &prepared, req.operation)?;
                return Ok(SelectedWriterPreparation::Historical {
                    transaction: Box::new(tx),
                    finished: finished.map(Box::new),
                });
            }
            ensure!(store.unavailable_commits()? == 0, "Writer store incomplete");
            let _mode = self.reader_mode_lease()?;
            let snapshot = store
                .selected_snapshot(self.scope(), MAX_ITEMS)?
                .context("Writer scope absent")?;
            ensure!(snapshot.token == *accepted, "Writer selection replaced");
            let (records, input) = prepare_records(
                store,
                &snapshot.transaction,
                &req,
                &fingerprint,
                ms,
                store.actor_id,
            )?;
            // Unknown commit acknowledgment cannot mint a fresh model permit.
            let published =
                store.commit_selected(accepted, req.operation, records, BTreeMap::new())?;
            ensure!(!published.replayed, "Writer preparation replay");
            let after = store
                .selected_snapshot(self.scope(), MAX_ITEMS)?
                .context("Writer post-state absent")?;
            ensure!(
                after.token == published.token,
                "Writer post-preparation selection replaced"
            );
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
                    SelectedWriterSource::Generate { .. } => {
                        pin(Namespace::Source, input.source.id)?
                    }
                },
                refined_draft,
                preparation_fingerprint: fingerprint.clone(),
            };
            let identity = prepared_identity(&prepared)?;
            Ok(SelectedWriterPreparation::Fresh {
                prepared: Box::new(prepared),
                model: SelectedWriterModelPermit {
                    owner: self.clone(),
                    fingerprint: identity,
                },
            })
        })
    }
    pub fn writer_input(&self, prepared: &SelectedWriterPrepared) -> Result<SelectedWriterInput> {
        self.with_current(&prepared.accepted, |snapshot| {
            prepared_input(
                &SelectedDomainProjection::from_snapshot(snapshot)?,
                prepared,
            )
        })
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
    let Record::NormalizedText(source) = current(p, &prepared.source)? else {
        bail!("Writer prepared source variant")
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
        source: source.as_ref().clone(),
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
    let input = prepared_input(&p, &f.prepared)?;
    validate_document(&tx.scope, &input.source)?;
    let mut root = root(&p, &f.prepared.root)?.clone();
    let Record::Turn(user) = current(&p, &f.prepared.user)? else {
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
        ensure!(
            f.prepared.accepted.scope() == self.scope(),
            "foreign Writer finish scope"
        );
        let fingerprint = finish_fingerprint(&f)?;
        let mut admissions = vec![self];
        admissions.extend_from_slice(additional);
        Self::with_gates(&admissions, |store| {
            let recover = |store: &Store| -> Result<
                Option<(SelectionTransaction, SelectedWriterAcknowledgment)>,
            > {
                let Some((tx, prev, p, _)) =
                    original(store, &f.prepared.accepted, f.operation, &fingerprint)?
                else {
                    return Ok(None);
                };
                let (_, Some(Record::Turn(turn))) = p
                    .head(Namespace::Conversation, f.assistant)
                    .context("Writer historical assistant absent")?
                else {
                    bail!("Writer historical assistant variant")
                };
                let (records, ack) =
                    finish_records(store, &prev, &f, turn.created_ms, store.actor_id)?;
                verify_records(&p, &records)?;
                Ok(Some((tx, ack)))
            };
            if let Some((transaction, ack)) = recover(store)? {
                return Ok(SelectedWriterPublication::Historical {
                    transaction,
                    acknowledgment: Box::new(ack),
                });
            }
            verify_new(store)?;
            ensure!(store.unavailable_commits()? == 0, "Writer incomplete Store");
            let _mode = self.reader_mode_lease()?;
            let snapshot = store
                .selected_snapshot(self.scope(), MAX_ITEMS)?
                .context("Writer scope absent")?;
            ensure!(
                snapshot.token == f.prepared.accepted,
                "Writer preparation replaced"
            );
            let (records, _) = finish_records(
                store,
                &snapshot.transaction,
                &f,
                f.updated_ms,
                store.actor_id,
            )?;
            let result =
                store.commit_selected(&f.prepared.accepted, f.operation, records, BTreeMap::new());
            let recovered = recover(store)?;
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
        })
    }
}

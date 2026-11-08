//! Selected text publication. Acknowledgment and historical evidence are not effects.
use super::*;
mod targets;
use crate::storage::selection::{
    SelectedReadBudget, SelectionMutation, SelectionPublication, SelectionToken,
    SelectionTransaction,
};
use crate::storage::{MAX_METADATA, MAX_RECORD};
use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub enum SelectedReaderText {
    Interpret,
    InterpretTarget { candidate: target::Candidate },
    Generate { assistant: Uuid },
}
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct SelectedReaderWrite {
    pub operation: Uuid,
    pub root: IntentRecordRef,
    pub user: IntentRecordRef,
    pub updated_ms: u64,
    pub text: String,
    pub kind: SelectedReaderText,
}
#[derive(Clone, Debug)]
pub struct SelectedReaderAcknowledgment {
    pub receipt: Receipt,
    pub root: IntentRecordRef,
    pub turn: IntentRecordRef,
    pub user: Turn,
    pub written: Turn,
}
pub enum SelectedReaderPublication {
    Published {
        publication: SelectionPublication,
        acknowledgment: Box<SelectedReaderAcknowledgment>,
    },
    Historical {
        transaction: SelectionTransaction,
        acknowledgment: Box<SelectedReaderAcknowledgment>,
    },
}
fn input(request: &SelectedReaderWrite) -> Result<()> {
    request.root.validate()?;
    request.user.validate()?;
    ensure!(
        !request.operation.is_nil()
            && request.root.namespace == Namespace::Conversation
            && request.user.namespace == Namespace::Conversation
            && request.root.record_id != request.user.record_id
            && !request.text.is_empty()
            && request.text.len() <= MAX_RECORD
            && serde_json::to_vec(request)?.len() <= MAX_RECORD,
        "invalid selected Reader text request"
    );
    if let SelectedReaderText::Generate { assistant } = request.kind {
        ensure!(
            !assistant.is_nil()
                && assistant != request.root.record_id
                && assistant != request.user.record_id
                && assistant != Ledger::receipt_id(request.operation),
            "invalid selected assistant identity"
        );
    }
    Ok(())
}
fn fingerprint(accepted: &SelectionToken, request: &SelectedReaderWrite) -> Result<String> {
    Ledger::fingerprint(&(
        "selected-reader-text-v1",
        IntentSelectionEvidence::from_token(accepted),
        request,
    ))
}
fn record(
    actor: Uuid,
    request: &SelectedReaderWrite,
    parents: BTreeSet<Uuid>,
    value: Record,
) -> Result<Envelope> {
    let id = match &value {
        Record::SourceTarget(v) => v.id,
        Record::TargetUse(v) => v.id,
        Record::TargetState(v) => v.id,
        Record::Root(root) => root.id,
        Record::Turn(turn) => turn.id,
        Record::Receipt(receipt) => Ledger::receipt_id(receipt.acknowledgment.operation),
        _ => bail!("invalid selected Reader write variant"),
    };
    let envelope = Envelope {
        envelope_version: FORMAT,
        namespace: if matches!(&value, Record::SourceTarget(_)) {
            Namespace::Source
        } else {
            Namespace::Conversation
        },
        domain_schema_version: if matches!(
            &value,
            Record::SourceTarget(_) | Record::TargetUse(_) | Record::TargetState(_)
        ) {
            target::SCHEMA
        } else {
            SCHEMA
        },
        record_id: id,
        revision_id: Uuid::new_v5(
            &request.operation,
            format!("reader-text:revision:{id}").as_bytes(),
        ),
        operation_id: Uuid::new_v5(
            &request.operation,
            format!("reader-text:record:{id}").as_bytes(),
        ),
        actor_id: actor,
        parents,
        kind: Kind::Value,
        media_descriptors: if let Record::SourceTarget(v) = &value {
            v.media()
        } else {
            vec![]
        },
        payload: serde_json::to_value(value)?,
    };
    envelope.validate()?;
    Ledger::decode(&envelope)?;
    ensure!(
        serde_json::to_vec(&envelope)?.len() <= MAX_RECORD,
        "selected Reader record exceeds bound"
    );
    Ok(envelope)
}
fn projection(
    store: &Store,
    transaction: &SelectionTransaction,
    mut reads: Option<&mut SelectedReadBudget>,
) -> Result<SelectedDomainProjection> {
    let records = transaction
        .selected
        .records
        .iter()
        .map(|reference| {
            if let Some(reads) = reads.as_deref_mut() {
                reads.charge(0, 1, reference.bytes, 0)?;
            }
            serde_json::from_slice::<Envelope>(&store.read_object(reference)?).map_err(Into::into)
        })
        .collect::<Result<Vec<_>>>()?;
    let projection = SelectedDomainProjection::from_mixed_records(
        &records,
        &transaction.selected.records,
        &transaction.scope,
    )?;
    projection.document_ownership()?;
    Ok(projection)
}
fn current<'a>(
    projection: &'a SelectedDomainProjection,
    reference: &IntentRecordRef,
) -> Result<(&'a Envelope, &'a Record)> {
    let value = projection.exact_reference(reference)?;
    let (envelope, Some(_)) = projection
        .head(reference.namespace, reference.record_id)
        .context("selected Reader head absent")?
    else {
        bail!("selected Reader head deleted")
    };
    ensure!(
        envelope.revision_id == reference.revision_id,
        "selected Reader head changed"
    );
    Ok((envelope, value))
}
fn prepare(
    store: &Store,
    transaction: &SelectionTransaction,
    request: &SelectedReaderWrite,
    actor: Uuid,
    reads: Option<&mut SelectedReadBudget>,
) -> Result<(Vec<Envelope>, Turn, Turn)> {
    let projection = projection(store, transaction, reads)?;
    let (_, Record::Root(root)) = current(&projection, &request.root)? else {
        bail!("selected Reader root variant")
    };
    let (_, Record::Turn(user)) = current(&projection, &request.user)? else {
        bail!("selected Reader user variant")
    };
    projection.inspect(root.id)?;
    ensure!(
        user.conversation == root.id
            && user.role == Role::User
            && user.mode == Mode::Reader
            && !user.sources.is_empty()
            && request.updated_ms >= root.updated_ms
            && request.updated_ms >= user.updated_ms,
        "selected Reader user or chronology unavailable"
    );
    for source in &user.sources {
        let (_, Some(value)) = projection
            .head(Namespace::Source, *source)
            .context("selected Reader source absent")?
        else {
            bail!("selected Reader source deleted")
        };
        let media = match value {
            Record::Source(source) if source.conversation == root.id && source.turn == user.id => {
                let mut media = vec![source.image.clone()];
                if let Some(parent) = &source.parent {
                    if parent.sha256 != source.image.sha256 {
                        media.push(parent.clone());
                    }
                }
                media
            }
            Record::Capture(capture)
                if capture.conversation == root.id && capture.turn == user.id =>
            {
                capture.facts.media()?
            }
            Record::LegacyCapture(capture)
                if capture.conversation == root.id && capture.turn == user.id =>
            {
                capture.media()?
            }
            Record::DevelopmentCapture(capture)
                if capture.conversation == root.id && capture.turn == user.id =>
            {
                capture.media()?
            }
            Record::ResidentCapture(capture)
                if capture.conversation == root.id && capture.turn == user.id =>
            {
                capture.media()?
            }
            _ => bail!("selected Reader source ownership mismatch"),
        };
        for media in media {
            let reference = transaction
                .selected
                .media
                .iter()
                .find(|reference| {
                    reference.sha256 == media.sha256 && reference.bytes == media.bytes
                })
                .context("selected Reader required media omitted")?;
            store.read_object(reference)?;
        }
    }
    let mut root = root.clone();
    root.updated_ms = request.updated_ms;
    let mut written = user.clone();
    let parents = match &request.kind {
        SelectedReaderText::Interpret | SelectedReaderText::InterpretTarget { .. } => {
            ensure!(
                user.outcome == Outcome::Prepared && user.text.is_none(),
                "selected request already interpreted"
            );
            written.outcome = Outcome::Interpreted;
            BTreeSet::from([request.user.revision_id])
        }
        SelectedReaderText::Generate { assistant } => {
            ensure!(
                user.outcome == Outcome::Interpreted && user.text.is_some(),
                "selected request not interpreted"
            );
            ensure!(
                projection
                    .head(Namespace::Conversation, *assistant)
                    .is_none(),
                "selected assistant already exists"
            );
            written.id = *assistant;
            written.role = Role::Assistant;
            written.outcome = Outcome::Generated;
            written.sequence = root.next_sequence;
            written.sources.clear();
            written.correction_of = None;
            written.created_ms = request.updated_ms;
            root.next_sequence = root
                .next_sequence
                .checked_add(1)
                .context("selected sequence exhausted")?;
            BTreeSet::new()
        }
    };
    written.text = Some(request.text.clone());
    written.updated_ms = request.updated_ms;
    written.completion = None;
    Ledger::validate_turn(&written)?;
    let mut records = vec![
        record(
            actor,
            request,
            BTreeSet::from([request.root.revision_id]),
            Record::Root(root),
        )?,
        record(actor, request, parents, Record::Turn(written.clone()))?,
    ];
    records.extend(targets::records(&projection, request, &written, actor)?);
    Ok((records, user.clone(), written))
}
fn recover(
    store: &Store,
    accepted: &SelectionToken,
    request: &SelectedReaderWrite,
    expected_fingerprint: &str,
    mut reads: Option<&mut SelectedReadBudget>,
) -> Result<Option<(SelectionTransaction, SelectedReaderAcknowledgment)>> {
    let transaction = match reads.as_deref_mut() {
        Some(reads) => {
            store.selected_receipt_with_budget(accepted.scope(), request.operation, reads)
        }
        None => store.selected_receipt(accepted.scope(), request.operation),
    }?;
    let Some(transaction) = transaction else {
        return Ok(None);
    };
    ensure!(
        transaction.mutation == SelectionMutation::Commit
            && transaction.previous_sha256.as_deref() == Some(accepted.selection_sha256()),
        "selected Reader original publication differs"
    );
    let predecessor = match reads.as_deref_mut() {
        Some(reads) => {
            store.selected_predecessor_with_budget(accepted.scope(), request.operation, reads)
        }
        None => store.selected_predecessor(accepted.scope(), request.operation),
    }?
    .context("selected Reader predecessor absent")?;
    ensure!(
        predecessor.store_generation == accepted.store_generation()
            && predecessor.aggregate_generation == accepted.aggregate_generation()
            && predecessor.accepted_base_sha256 == accepted.accepted_base_sha256()
            && predecessor.scope == *accepted.scope(),
        "selected Reader original token differs"
    );
    let actual = projection(store, &transaction, reads.as_deref_mut())?;
    let (receipt_envelope, Some(Record::Receipt(original_receipt))) = actual
        .head(
            Namespace::Conversation,
            Ledger::receipt_id(request.operation),
        )
        .context("selected Reader original receipt absent")?
    else {
        bail!("selected Reader original receipt variant")
    };
    ensure!(
        receipt_envelope.domain_schema_version == SCHEMA
            && original_receipt.admitted_intent.is_none()
            && original_receipt.fingerprint == expected_fingerprint,
        "selected Reader original fingerprint differs"
    );
    let actor = receipt_envelope.actor_id;
    let (mut expected, user, written) = prepare(store, &predecessor, request, actor, reads)?;
    let root = pending::pin(&expected[0])?;
    let turn = pending::pin(&expected[1])?;
    let receipt = Receipt {
        fingerprint: expected_fingerprint.into(),
        acknowledgment: Acknowledgment {
            operation: request.operation,
            conversation: request.root.record_id,
            records: vec![written.id, request.root.record_id],
            applied_root_revision: root.revision_id,
            binding: match &Ledger::decode(&expected[0])? {
                Record::Root(root) => root.binding,
                _ => None,
            },
        },
        admitted_intent: None,
    };
    expected.push(record(
        actor,
        request,
        BTreeSet::new(),
        Record::Receipt(receipt.clone()),
    )?);
    actual.inspect(request.root.record_id)?;
    for envelope in expected {
        let reference = pending::pin(&envelope)?;
        ensure!(
            actual.exact_reference(&reference)? == &Ledger::decode(&envelope)?,
            "selected Reader original records differ"
        );
    }
    Ok(Some((
        transaction,
        SelectedReaderAcknowledgment {
            receipt,
            root,
            turn,
            user,
            written,
        },
    )))
}
impl SelectedAdmission {
    #[cfg(target_os = "linux")]
    pub(crate) fn recover_reader_text(
        &self,
        accepted: &SelectionToken,
        request: &SelectedReaderWrite,
        reads: &mut SelectedReadBudget,
    ) -> Result<bool> {
        input(request)?;
        ensure!(
            accepted.scope() == self.scope(),
            "foreign selected Reader scope"
        );
        Self::with_gates(&[self], |store| {
            Ok(recover(
                store,
                accepted,
                request,
                &fingerprint(accepted, request)?,
                Some(reads),
            )?
            .is_some())
        })
    }

    pub fn publish_reader_text(
        &self,
        accepted: &SelectionToken,
        request: SelectedReaderWrite,
    ) -> Result<SelectedReaderPublication> {
        self.publish_reader_text_joined(accepted, request, &[], |_| Ok(()))
    }
    /// Own the complete ordered set; the verifier must only use the supplied
    /// Store and must never reacquire a participating admission.
    pub(crate) fn publish_reader_text_joined(
        &self,
        accepted: &SelectionToken,
        request: SelectedReaderWrite,
        additional: &[&SelectedAdmission],
        verify_new: impl FnOnce(&Store) -> Result<()>,
    ) -> Result<SelectedReaderPublication> {
        self.publish_reader_text_with_reads(accepted, request, additional, None, |store, _| {
            verify_new(store)
        })
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn publish_resident_reader_text_joined(
        &self,
        accepted: &SelectionToken,
        request: SelectedReaderWrite,
        additional: &[&SelectedAdmission],
        reads: &mut SelectedReadBudget,
        verify_new: impl FnOnce(&Store, &mut SelectedReadBudget) -> Result<()>,
    ) -> Result<SelectedReaderPublication> {
        self.publish_reader_text_with_reads(
            accepted,
            request,
            additional,
            Some(reads),
            |store, reads| verify_new(store, reads.context("resident Reader allowance absent")?),
        )
    }

    fn publish_reader_text_with_reads(
        &self,
        accepted: &SelectionToken,
        request: SelectedReaderWrite,
        additional: &[&SelectedAdmission],
        mut reads: Option<&mut SelectedReadBudget>,
        verify_new: impl FnOnce(&Store, Option<&mut SelectedReadBudget>) -> Result<()>,
    ) -> Result<SelectedReaderPublication> {
        input(&request)?;
        ensure!(
            accepted.scope() == self.scope(),
            "foreign selected Reader scope"
        );
        let fingerprint = fingerprint(accepted, &request)?;
        let mut admissions = Vec::with_capacity(additional.len() + 1);
        admissions.push(self);
        admissions.extend_from_slice(additional);
        let resident = reads.is_some();
        let publish = |store: &Store| {
            let mut verify = Some(verify_new);
            if let Some((transaction, acknowledgment)) = recover(
                store,
                accepted,
                &request,
                &fingerprint,
                reads.as_deref_mut(),
            )? {
                return Ok(SelectedReaderPublication::Historical {
                    transaction,
                    acknowledgment: Box::new(acknowledgment),
                });
            }
            if !resident {
                verify.take().unwrap()(store, reads.as_deref_mut())?;
            }
            ensure!(
                store.unavailable_commits()? == 0,
                "selected Reader store incomplete"
            );
            let _mode_lease = if resident {
                None
            } else {
                self.reader_mode_lease()?
            };
            let snapshot = match reads.as_deref_mut() {
                Some(reads) => store
                    .selected_snapshot_with_budget(self.scope(), MAX_ITEMS, reads)?
                    .map(|read| read.snapshot),
                None => store.selected_snapshot(self.scope(), MAX_ITEMS)?,
            }
            .context("selected Reader scope absent")?;
            ensure!(
                snapshot.token == *accepted,
                "selected Reader selection replaced"
            );
            let (mut records, _, written) = prepare(
                store,
                &snapshot.transaction,
                &request,
                store.actor_id,
                reads.as_deref_mut(),
            )?;
            let receipt = Receipt {
                fingerprint: fingerprint.clone(),
                acknowledgment: Acknowledgment {
                    operation: request.operation,
                    conversation: request.root.record_id,
                    records: vec![written.id, request.root.record_id],
                    applied_root_revision: records[0].revision_id,
                    binding: match Ledger::decode(&records[0])? {
                        Record::Root(root) => root.binding,
                        _ => None,
                    },
                },
                admitted_intent: None,
            };
            records.push(record(
                store.actor_id,
                &request,
                BTreeSet::new(),
                Record::Receipt(receipt),
            )?);
            ensure!(
                records
                    .iter()
                    .map(|record| serde_json::to_vec(record).map(|bytes| bytes.len()))
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .iter()
                    .sum::<usize>()
                    <= MAX_METADATA,
                "selected Reader publication exceeds bound"
            );
            if let Some(verify) = verify {
                verify(store, reads.as_deref_mut())?;
            }
            let publication =
                store.commit_selected(accepted, request.operation, records, BTreeMap::new());
            let recovered = recover(store, accepted, &request, &fingerprint, reads)?;
            match (publication, recovered) {
                (Ok(publication), Some((_, acknowledgment))) if !publication.replayed => {
                    Ok(SelectedReaderPublication::Published {
                        publication,
                        acknowledgment: Box::new(acknowledgment),
                    })
                }
                (_, Some((transaction, acknowledgment))) => {
                    Ok(SelectedReaderPublication::Historical {
                        transaction,
                        acknowledgment: Box::new(acknowledgment),
                    })
                }
                (Err(error), None) => Err(error),
                (Ok(_), None) => bail!("selected Reader acknowledgment unavailable"),
            }
        };
        if resident {
            Self::with_gates(&admissions, publish)
        } else {
            Self::with_ready_gate_set(&admissions, publish)
        }
    }
}

//! Selected resident Prepared publication. A receipt is data custody only.
use super::*;
use crate::storage::selection::{SelectedReadBudget, SelectionMutation, SelectionToken};

pub(crate) struct ResidentPrepared {
    operation: Uuid,
    records: Vec<Envelope>,
    media: BTreeMap<String, Vec<u8>>,
    acknowledgment: ResidentPreparedAcknowledgment,
    source_binding: Uuid,
}

#[derive(Clone)]
pub(crate) struct ResidentPreparedAcknowledgment {
    pub(crate) root: IntentRecordRef,
    pub(crate) user: IntentRecordRef,
    pub(crate) capture: IntentRecordRef,
}

impl ResidentPrepared {
    pub(crate) fn new(
        store: &Store,
        accepted: &SelectionToken,
        capture: &ResidentSourceCapture,
        operation: Uuid,
        timestamp: u64,
    ) -> Result<Self> {
        Self::build(
            store, accepted, capture, operation, operation, timestamp, false,
        )
    }

    #[cfg(all(
        target_os = "linux",
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    pub(crate) fn new_writer(
        store: &Store,
        accepted: &SelectionToken,
        capture: &ResidentSourceCapture,
        logical: Uuid,
        timestamp: u64,
        binding: &str,
    ) -> Result<Self> {
        let operation = crate::writer::selected::normalization_preparations(logical)[1];
        let mut packet = Self::build(
            store, accepted, capture, operation, logical, timestamp, true,
        )?;
        let fingerprint = Ledger::fingerprint(&(
            "selected-fresh-regional-prepare-v1",
            IntentSelectionEvidence::from_token(accepted),
            logical,
            binding,
            &packet.acknowledgment.root,
            &packet.acknowledgment.user,
            &packet.acknowledgment.capture,
        ))?;
        let receipt = packet
            .records
            .last_mut()
            .context("fresh Writer receipt absent")?;
        let Record::Receipt(mut value) = Ledger::decode(receipt)? else {
            bail!("fresh Writer receipt variant")
        };
        value.fingerprint = fingerprint;
        receipt.payload = serde_json::to_value(Record::Receipt(value))?;
        Ledger::decode(receipt)?;
        Ok(packet)
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        store: &Store,
        accepted: &SelectionToken,
        capture: &ResidentSourceCapture,
        operation: Uuid,
        logical: Uuid,
        timestamp: u64,
        writer: bool,
    ) -> Result<Self> {
        ensure!(!operation.is_nil(), "nil selected Prepare operation");
        ensure!(
            crate::memory::catalog::document_key(capture.document())?
                == accepted.scope().key_sha256,
            "resident capture belongs to another selected document"
        );
        let id = |label: &str| Uuid::new_v5(&operation, label.as_bytes());
        let conversation = if writer {
            crate::writer::stage(logical, "fresh-root")
        } else {
            id("reader-prepare:root")
        };
        let user = if writer {
            crate::writer::stage(logical, "user")
        } else {
            id("reader-prepare:user")
        };
        let evidence_id = if writer {
            crate::writer::stage(logical, "fresh-capture")
        } else {
            id("reader-prepare:capture")
        };
        let prepared = capture.prepare_history(conversation, user, evidence_id)?;
        let turn = Turn {
            id: user,
            conversation,
            exchange: if writer {
                logical
            } else {
                id("reader-prepare:exchange")
            },
            sequence: 0,
            role: Role::User,
            mode: if writer { Mode::Writer } else { Mode::Reader },
            outcome: Outcome::Prepared,
            text: None,
            sources: vec![evidence_id],
            correction_of: None,
            created_ms: timestamp,
            updated_ms: timestamp,
            completion: None,
        };
        Ledger::validate_turn(&turn)?;
        let prefix = if writer {
            "fresh-writer-prepare"
        } else {
            "reader-prepare"
        };
        let record =
            |namespace, record_id, record: Record, media_descriptors| -> Result<Envelope> {
                let envelope = Envelope {
                    envelope_version: FORMAT,
                    namespace,
                    domain_schema_version: if namespace == Namespace::Source {
                        TEXT_SOURCE_SCHEMA
                    } else {
                        SCHEMA
                    },
                    record_id,
                    revision_id: id(&format!("{prefix}:revision:{record_id}")),
                    operation_id: id(&format!("{prefix}:record:{record_id}")),
                    actor_id: store.actor_id,
                    parents: BTreeSet::new(),
                    kind: Kind::Value,
                    payload: serde_json::to_value(record)?,
                    media_descriptors,
                };
                envelope.validate()?;
                Ledger::decode(&envelope)?;
                Ok(envelope)
            };
        let root = record(
            Namespace::Conversation,
            conversation,
            Record::Root(Root {
                id: conversation,
                next_sequence: 1,
                binding: None,
                created_ms: timestamp,
                updated_ms: timestamp,
            }),
            vec![],
        )?;
        let user_record = record(Namespace::Conversation, user, Record::Turn(turn), vec![])?;
        let mut media = BTreeMap::from([(
            prepared.evidence.original_png.sha256.clone(),
            prepared.original_png,
        )]);
        for (image, bytes) in prepared.evidence.images.iter().zip(prepared.images) {
            media.insert(image.media.sha256.clone(), bytes);
        }
        let descriptors = prepared.evidence.media()?;
        let source = record(
            Namespace::Source,
            evidence_id,
            Record::ResidentCapture(Box::new(prepared.evidence)),
            descriptors,
        )?;
        let acknowledgment = ResidentPreparedAcknowledgment {
            root: pending::pin(&root)?,
            user: pending::pin(&user_record)?,
            capture: pending::pin(&source)?,
        };
        let receipt = Receipt {
            fingerprint: Ledger::fingerprint(&(
                "selected-resident-prepare-v1",
                IntentSelectionEvidence::from_token(accepted),
                &acknowledgment.root,
                &acknowledgment.user,
                &acknowledgment.capture,
            ))?,
            acknowledgment: Acknowledgment {
                operation,
                conversation,
                records: vec![user, evidence_id, conversation],
                applied_root_revision: root.revision_id,
                binding: None,
            },
            admitted_intent: None,
        };
        Ok(Self {
            operation,
            records: vec![
                root,
                user_record,
                source,
                record(
                    Namespace::Conversation,
                    Ledger::receipt_id(operation),
                    Record::Receipt(receipt),
                    vec![],
                )?,
            ],
            media,
            acknowledgment,
            source_binding: Ledger::binding_id(capture.document(), capture.page()),
        })
    }

    #[cfg(all(
        target_os = "linux",
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    pub(crate) fn capture_exclusions(
        capture: &ResidentSourceCapture,
        logical: Uuid,
    ) -> Result<BTreeSet<String>> {
        let prepared = capture.prepare_history(
            crate::writer::stage(logical, "fresh-root"),
            crate::writer::stage(logical, "user"),
            crate::writer::stage(logical, "fresh-capture"),
        )?;
        Ok(prepared
            .evidence
            .media()?
            .into_iter()
            .map(|m| m.sha256)
            .collect())
    }

    /// Must be called under the original joined gates and exact slot claim.
    #[cfg(all(
        target_os = "linux",
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    pub(crate) fn publish_writer_locked(
        mut self,
        store: &Store,
        accepted: &SelectionToken,
        logical: Uuid,
        reads: &mut SelectedReadBudget,
        check: &mut dyn FnMut() -> Result<()>,
    ) -> Result<(SelectionToken, ResidentPreparedAcknowledgment)> {
        check()?;
        ensure!(
            self.operation == crate::writer::selected::normalization_preparations(logical)[1],
            "fresh Writer admission identity"
        );
        let snapshot = store
            .selected_snapshot_with_budget(accepted.scope(), crate::storage::MAX_ITEMS, reads)?
            .context("fresh Writer document absent")?;
        ensure!(
            snapshot.snapshot.token == *accepted,
            "fresh Writer selection changed"
        );
        let p = SelectedDomainProjection::from_snapshot(&snapshot.snapshot)?;
        ensure!(
            !matches!(
                p.head(Namespace::Conversation, self.source_binding),
                Some((_, Some(Record::Binding(_))))
            ),
            "fresh Writer source is a known Buddy page"
        );
        let mut records = snapshot.snapshot.selected_records;
        records.extend_from_slice(&self.records);
        let mut references = snapshot.snapshot.transaction.selected.records;
        references.extend(
            self.records
                .iter()
                .map(pending::pin)
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .map(|p| p.object),
        );
        SelectedDomainProjection::from_mixed_records(&records, &references, accepted.scope())?
            .inspect(self.acknowledgment.root.record_id)?;
        let publication = store.commit_selected_guarded(
            accepted,
            self.operation,
            self.records.clone(),
            std::mem::take(&mut self.media),
            reads,
            &crate::writer::selected::normalization_prerequisites(logical),
            check,
        )?;
        ensure!(!publication.replayed, "fresh Writer preparation historical");
        Ok((publication.token, self.acknowledgment))
    }

    /// Resolve the existing original transaction even when live admission was
    /// lost. Exact records and predecessor are required before crediting it.
    pub(crate) fn original(
        &self,
        store: &Store,
        accepted: &SelectionToken,
        reads: &mut SelectedReadBudget,
    ) -> Result<bool> {
        let Some(transaction) =
            store.selected_receipt_with_budget(accepted.scope(), self.operation, reads)?
        else {
            return Ok(false);
        };
        ensure!(
            transaction.mutation == SelectionMutation::Commit
                && transaction.previous_sha256.as_deref() == Some(accepted.selection_sha256())
                && transaction.store_generation == accepted.store_generation()
                && transaction.accepted_base_sha256 == accepted.accepted_base_sha256(),
            "selected Prepare original predecessor differs"
        );
        for record in &self.records {
            let pin = pending::pin(record)?;
            reads.charge(0, 1, pin.object.bytes, 0)?;
            ensure!(
                transaction.selected.records.contains(&pin.object)
                    && store.read_object(&pin.object)? == serde_json::to_vec(record)?,
                "selected Prepare original record differs"
            );
            for media in &record.media_descriptors {
                let object = ObjectRef {
                    sha256: media.sha256.clone(),
                    bytes: media.bytes,
                };
                ensure!(
                    transaction.selected.media.contains(&object),
                    "selected Prepare original media absent"
                );
            }
        }
        Ok(true)
    }

    /// Caller holds the registered document and preference gates. Historical
    /// acknowledgment is deliberately never returned as a live input.
    pub(crate) fn publish_locked(
        mut self,
        store: &Store,
        accepted: &SelectionToken,
        reads: &mut SelectedReadBudget,
    ) -> Result<(SelectionToken, ResidentPreparedAcknowledgment)> {
        ensure!(
            !self.original(store, accepted, reads)?,
            "selected Prepare already historical"
        );
        let current = store
            .selected_snapshot_with_budget(accepted.scope(), crate::storage::MAX_ITEMS, reads)?
            .context("selected Prepare document absent")?;
        ensure!(
            current.snapshot.token == *accepted,
            "selected Prepare document changed"
        );
        let current_projection = SelectedDomainProjection::from_snapshot(&current.snapshot)?;
        ensure!(
            !matches!(
                current_projection.head(Namespace::Conversation, self.source_binding),
                Some((_, Some(Record::Binding(_))))
            ),
            "selected Reader source is a known Buddy page"
        );
        let mut records = current.snapshot.selected_records;
        records.extend_from_slice(&self.records);
        let mut references = current.snapshot.transaction.selected.records;
        for record in &self.records {
            references.push(pending::pin(record)?.object);
        }
        let projection =
            SelectedDomainProjection::from_mixed_records(&records, &references, accepted.scope())?;
        projection.inspect(self.acknowledgment.root.record_id)?;
        let publication = store.commit_selected(
            accepted,
            self.operation,
            self.records.clone(),
            std::mem::take(&mut self.media),
        );
        let original = self.original(store, accepted, reads)?;
        let publication = publication?;
        ensure!(
            !publication.replayed && original,
            "selected Prepare acknowledgment unavailable"
        );
        Ok((publication.token, self.acknowledgment))
    }
}

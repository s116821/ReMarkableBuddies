//! Local-first conversation ledger. Stored facts are not native effect permissions.
mod admission;
pub mod capture_facts;
pub mod development;
mod intent;
mod legacy;
pub use development::{DevelopmentCaptureHistory, PreparedDevelopmentCapture};
mod pending;
mod reader_dispatch;
mod reader_plan;
mod reader_uncertainty;
mod selected;
mod settlement;
mod types;
use crate::storage::{
    digest, Conflict, Envelope, Kind, Media, Namespace, ObjectRef, Store, Uuid, FORMAT, MAX_ITEMS,
};
pub use admission::{HistoricalIntent, SelectedAdmission};
use anyhow::{bail, ensure, Context, Result};
pub use intent::*;
pub use pending::{PendingIntentPublication, PendingIntentRequest, SourceAdmission};
pub(crate) use pending::{ReaderContext, ReaderPreparation};
pub use reader_plan::{ReaderEraseRectangle, ReaderHandoff, ReaderPlan, SmartEraseInput};
pub use selected::*;
use serde::Serialize;
pub use settlement::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    sync::Arc,
};
pub use types::*;

pub struct Ledger {
    store: Arc<Store>,
}
impl Ledger {
    fn complete_store(&self) -> Result<()> {
        let unavailable_commits = self.store.unavailable_commits()?;
        if unavailable_commits != 0 {
            return Err(IncompleteStore {
                unavailable_commits,
            }
            .into());
        }
        Ok(())
    }
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }
    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }
    pub fn binding_id(document: Uuid, page: Uuid) -> Uuid {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("urn:remarkable-buddies:binding:v1:{document}:{page}").as_bytes(),
        )
    }
    fn receipt_id(operation: Uuid) -> Uuid {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("urn:remarkable-buddies:operation:v1:{operation}").as_bytes(),
        )
    }
    pub fn expected(&self, namespace: Namespace, id: Uuid) -> Result<ExpectedHeads> {
        Ok(ExpectedHeads(
            self.store
                .heads(namespace, id)?
                .iter()
                .map(|e| e.revision_id)
                .collect(),
        ))
    }
    fn envelope(
        &self,
        namespace: Namespace,
        id: Uuid,
        parents: ExpectedHeads,
        record: Record,
        media: Vec<Media>,
    ) -> Result<Envelope> {
        let envelope = Envelope {
            envelope_version: FORMAT,
            namespace,
            domain_schema_version: match &record {
                Record::Receipt(receipt) if receipt.admitted_intent.is_some() => INTENT_SCHEMA,
                Record::OutcomeFact(fact) if fact.settlement.is_some() => INTENT_SCHEMA,
                _ => SCHEMA,
            },
            record_id: id,
            revision_id: Uuid::new_v4(),
            parents: parents.0,
            operation_id: Uuid::new_v4(),
            actor_id: self.store.actor_id,
            kind: Kind::Value,
            payload: serde_json::to_value(record)?,
            media_descriptors: media,
        };
        envelope.validate()?;
        Ok(envelope)
    }
    fn decode(envelope: &Envelope) -> Result<Record> {
        ensure!(
            matches!(envelope.domain_schema_version, SCHEMA | INTENT_SCHEMA),
            "unsupported conversation schema"
        );
        ensure!(envelope.kind == Kind::Value, "deleted conversation record");
        let record: Record = serde_json::from_value(envelope.payload.clone())?;
        if envelope.domain_schema_version == INTENT_SCHEMA {
            ensure!(
                envelope.namespace == Namespace::Conversation
                    && matches!(&record, Record::Receipt(_) | Record::OutcomeFact(_)),
                "schema 2 is restricted to intent/settlement records"
            );
        }
        let (id, namespace, conversation) = match &record {
            Record::Root(root) => (root.id, Namespace::Conversation, root.id),
            Record::Turn(turn) => {
                Self::validate_turn(turn)?;
                (turn.id, Namespace::Conversation, turn.conversation)
            }
            Record::Source(source) => {
                Self::validate_source_facts(source)?;
                (source.id, Namespace::Source, source.conversation)
            }
            Record::Capture(capture) => {
                capture.facts.validate()?;
                ensure!(
                    capture.id == capture.facts.operation.value() && !capture.turn.is_nil(),
                    "invalid historical capture identity"
                );
                (capture.id, Namespace::Source, capture.conversation)
            }
            Record::DevelopmentCapture(capture) => {
                capture.validate()?;
                (capture.id, Namespace::Source, capture.conversation)
            }
            Record::LegacyCapture(capture) => {
                capture.validate()?;
                (capture.id, Namespace::Source, capture.conversation)
            }
            Record::OutcomeFact(fact) => {
                intent::validate_settlement_schema(envelope.domain_schema_version, fact)?;
                ensure!(
                    !fact.id.is_nil()
                        && !fact.turn.is_nil()
                        && (matches!(
                            fact.outcome,
                            Outcome::Failed | Outcome::Canceled | Outcome::ReconcileRequired
                        ) || (fact.outcome == Outcome::Completed
                            && envelope.domain_schema_version == 2
                            && fact.settlement.as_ref().is_some_and(|settlement| {
                                settlement.state == IntentSettlementState::VerifiedSubmitted
                            }))),
                    "invalid attempt outcome fact"
                );
                (fact.id, Namespace::Conversation, fact.conversation)
            }
            Record::Binding(binding) => {
                Self::validate_observation(&binding.receipt.source)?;
                ensure!(
                    binding.id
                        == Self::binding_id(
                            binding.receipt.observed_document,
                            binding.receipt.observed_page
                        )
                        && binding.conversation == binding.receipt.conversation,
                    "binding payload identity mismatch"
                );
                (binding.id, Namespace::Conversation, binding.conversation)
            }
            Record::Export(export) => {
                (export.id, Namespace::ExportAssociation, export.conversation)
            }
            Record::Receipt(receipt) => {
                intent::validate_receipt_schema(envelope.domain_schema_version, receipt)?;
                ensure!(
                    crate::storage::valid_digest(&receipt.fingerprint)
                        && !receipt.acknowledgment.applied_root_revision.is_nil()
                        && !receipt.acknowledgment.operation.is_nil(),
                    "invalid operation acknowledgment"
                );
                (
                    Self::receipt_id(receipt.acknowledgment.operation),
                    Namespace::Conversation,
                    receipt.acknowledgment.conversation,
                )
            }
        };
        ensure!(
            id == envelope.record_id && namespace == envelope.namespace && !conversation.is_nil(),
            "domain envelope identity mismatch"
        );
        let mut media = match &record {
            Record::Source(source) => vec![source.image.clone()],
            Record::Capture(capture) => capture.facts.media()?,
            Record::LegacyCapture(capture) => capture.media()?,
            Record::DevelopmentCapture(capture) => capture.media()?,
            Record::Receipt(receipt) => receipt
                .admitted_intent
                .as_ref()
                .map(|i| i.media.clone())
                .unwrap_or_default(),
            Record::OutcomeFact(fact) => fact
                .settlement
                .as_ref()
                .map(|s| s.media.clone())
                .unwrap_or_default(),
            _ => vec![],
        };
        if let Record::Source(source) = &record {
            if let Some(parent) = &source.parent {
                if parent.sha256 != source.image.sha256 {
                    media.push(parent.clone());
                }
            }
        }
        media.sort_by(|a, b| a.sha256.cmp(&b.sha256));
        let mut declared = envelope.media_descriptors.clone();
        declared.sort_by(|a, b| a.sha256.cmp(&b.sha256));
        ensure!(declared == media, "domain media descriptors mismatch");
        Ok(record)
    }
    fn live_root(
        &self,
        conversation: Uuid,
        expected: Option<&ExpectedHeads>,
    ) -> Result<(Envelope, Root)> {
        self.complete_store()?;
        let heads = self.store.heads(Namespace::Conversation, conversation)?;
        if let Some(expected) = expected {
            ensure!(
                heads.iter().map(|e| e.revision_id).collect::<BTreeSet<_>>() == expected.0,
                "stale conversation heads"
            );
        }
        if heads.len() > 1 {
            return Err(Conflict {
                heads: heads.iter().map(|e| e.revision_id).collect(),
            }
            .into());
        }
        let envelope = heads.into_iter().next().context("conversation absent")?;
        match Self::decode(&envelope)? {
            Record::Root(root) if root.id == conversation => Ok((envelope, root)),
            _ => bail!("invalid conversation root"),
        }
    }
    fn fingerprint(value: &impl Serialize) -> Result<String> {
        Ok(digest(&serde_json::to_vec(value)?))
    }
    fn replay(&self, operation: Uuid, fingerprint: &str) -> Result<Option<WriteResult>> {
        self.complete_store()?;
        ensure!(!operation.is_nil(), "nil domain operation identity");
        ensure!(!operation.is_nil(), "nil operation identity");
        let Some(e) = self
            .store
            .value(Namespace::Conversation, Self::receipt_id(operation))?
        else {
            return Ok(None);
        };
        let Record::Receipt(receipt) = Self::decode(&e)? else {
            bail!("invalid operation receipt");
        };
        ensure!(
            receipt.fingerprint == fingerprint,
            "operation fingerprint conflict"
        );
        ensure!(
            receipt.acknowledgment.operation == operation,
            "operation receipt identity mismatch"
        );
        let current_binding = match receipt.acknowledgment.binding {
            Some(id) => self.store.heads(Namespace::Conversation, id)?,
            None => vec![],
        };
        Ok(Some(WriteResult {
            current_root: self
                .store
                .heads(Namespace::Conversation, receipt.acknowledgment.conversation)?,
            current_binding,
            historical: receipt.acknowledgment,
        }))
    }
    fn publish(
        &self,
        operation: Uuid,
        fingerprint: String,
        root: Envelope,
        mut records: Vec<Envelope>,
        binding: Option<Uuid>,
    ) -> Result<WriteResult> {
        let ack = Acknowledgment {
            operation,
            conversation: root.record_id,
            records: records.iter().map(|e| e.record_id).collect(),
            applied_root_revision: root.revision_id,
            binding,
        };
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
        records.push(root);
        records.push(receipt);
        let result = self.store.commit(records, BTreeMap::new());
        // A committed-but-lost acknowledgment, or a concurrent identical winner,
        // is recognized before propagating stale-head/injected-error outcomes.
        if let Some(prior) = self.replay(operation, &fingerprint)? {
            return Ok(prior);
        }
        result?;
        bail!("committed operation receipt unavailable")
    }
    pub fn create(
        &self,
        conversation: Uuid,
        operation: Uuid,
        created_ms: u64,
    ) -> Result<WriteResult> {
        ensure!(!conversation.is_nil(), "nil conversation identity");
        let fingerprint = Self::fingerprint(&("create", conversation, created_ms))?;
        if let Some(prior) = self.replay(operation, &fingerprint)? {
            return Ok(prior);
        }
        let root = self.envelope(
            Namespace::Conversation,
            conversation,
            ExpectedHeads::default(),
            Record::Root(Root {
                id: conversation,
                next_sequence: 0,
                binding: None,
                created_ms,
                updated_ms: created_ms,
            }),
            vec![],
        )?;
        self.publish(operation, fingerprint, root, vec![], None)
    }
    /// Stage exact bytes before an atomic source-use commit; staging is not visibility.
    pub fn stage_image(&self, media: &Media, reader: impl Read) -> Result<()> {
        media.validate()?;
        self.store.stage_blob(
            &ObjectRef {
                sha256: media.sha256.clone(),
                bytes: media.bytes,
            },
            reader,
        )
    }
    pub fn append(
        &self,
        operation: Uuid,
        expected: ExpectedHeads,
        mut turn: Turn,
        sources: Vec<ImageUse>,
    ) -> Result<WriteResult> {
        let fingerprint = Self::fingerprint(&("append", &turn, &sources))?;
        if let Some(prior) = self.replay(operation, &fingerprint)? {
            return Ok(prior);
        }
        ensure!(
            !turn.id.is_nil() && !turn.exchange.is_nil(),
            "nil turn identity"
        );
        ensure!(sources.len() <= MAX_ITEMS, "too many source uses");
        ensure!(turn.sequence == 0, "caller must not allocate chronology");
        Self::validate_turn(&turn)?;
        let (_, mut root) = self.live_root(turn.conversation, Some(&expected))?;
        turn.sequence = root.next_sequence;
        root.next_sequence = root
            .next_sequence
            .checked_add(1)
            .context("sequence exhausted")?;
        root.updated_ms = turn.updated_ms;
        let source_ids = sources.iter().map(|s| s.id).collect::<BTreeSet<_>>();
        ensure!(
            source_ids.len() == sources.len(),
            "duplicate source use identity"
        );
        ensure!(
            sources
                .iter()
                .map(|source| source.ordinal)
                .collect::<BTreeSet<_>>()
                .len()
                == sources.len(),
            "duplicate image order"
        );
        let links = turn.sources.iter().copied().collect::<BTreeSet<_>>();
        ensure!(
            links.len() == turn.sources.len() && source_ids.is_subset(&links),
            "source links mismatch"
        );
        for linked in links.difference(&source_ids) {
            let e = self
                .store
                .value(Namespace::Source, *linked)?
                .context("existing source reference absent")?;
            let conversation = match Self::decode(&e)? {
                Record::Source(source) => source.conversation,
                Record::Capture(capture) => capture.conversation,
                Record::LegacyCapture(capture) => capture.conversation,
                Record::DevelopmentCapture(capture) => capture.conversation,
                _ => bail!("invalid existing source reference"),
            };
            ensure!(
                conversation == turn.conversation,
                "source outside conversation"
            );
        }
        self.validate_correction_target(&turn)?;
        let mut records = Vec::new();
        for source in sources {
            Self::validate_source(&source, &turn)?;
            let mut media = vec![source.image.clone()];
            if let Some(parent) = &source.parent {
                if parent.sha256 != source.image.sha256 {
                    media.push(parent.clone());
                }
            }
            records.push(self.envelope(
                Namespace::Source,
                source.id,
                ExpectedHeads::default(),
                Record::Source(source),
                media,
            )?);
        }
        records.push(self.envelope(
            Namespace::Conversation,
            turn.id,
            ExpectedHeads::default(),
            Record::Turn(turn),
            vec![],
        )?);
        let binding = root.binding;
        let envelope = self.envelope(
            Namespace::Conversation,
            root.id,
            expected,
            Record::Root(root),
            vec![],
        )?;
        self.publish(operation, fingerprint, envelope, records, binding)
    }
    /// Prepare explicitly synthetic SDK fixture evidence; never grants native authority.
    pub fn prepare_sdk_fixture(
        &self,
        operation: Uuid,
        expected_root: ExpectedHeads,
        mut turn: Turn,
        batch: &remarkable_open_sdk::capture::CapturedBatch,
    ) -> Result<PreparedSdkCapture> {
        ensure!(
            turn.role == Role::User
                && turn.outcome == Outcome::Prepared
                && turn.sequence == 0
                && turn.sources.is_empty(),
            "invalid SDK fixture request"
        );
        Self::validate_turn(&turn)?;
        let facts = capture_facts::HistoricalCapture::from_batch(batch)?;
        let capture = CaptureEvidence {
            id: facts.operation.value(),
            conversation: turn.conversation,
            turn: turn.id,
            facts,
        };
        turn.sources = vec![capture.id];
        let fingerprint = Self::fingerprint(&("prepare-sdk-fixture", &turn, &capture))?;
        if self.replay(operation, &fingerprint)?.is_some() {
            return self.stored_sdk_images(capture.id);
        }
        let (_, mut root) = self.live_root(turn.conversation, Some(&expected_root))?;
        self.validate_correction_target(&turn)?;
        let media = capture.facts.media()?;
        // Original SDK bytes are staged unchanged; no recreated image encoding.
        for image in std::iter::once(batch.native_parent()).chain(batch.images()) {
            let descriptor = Media {
                sha256: image
                    .sha256()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
                bytes: image.bytes().len() as u64,
                media_type: image.mime().into(),
            };
            let [width, height] = image.dimensions();
            Self::validate_image_bytes(&descriptor, image.bytes(), width, height)?;
        }
        for image in std::iter::once(batch.native_parent()).chain(batch.images()) {
            let descriptor = Media {
                sha256: image
                    .sha256()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
                bytes: image.bytes().len() as u64,
                media_type: image.mime().into(),
            };
            self.stage_image(&descriptor, image.bytes())?;
        }
        turn.sequence = root.next_sequence;
        root.next_sequence = root
            .next_sequence
            .checked_add(1)
            .context("sequence exhausted")?;
        root.updated_ms = turn.updated_ms;
        let binding = root.binding;
        let turn_id = turn.id;
        let capture_id = capture.id;
        let records = vec![
            self.envelope(
                Namespace::Conversation,
                turn_id,
                ExpectedHeads::default(),
                Record::Turn(turn),
                vec![],
            )?,
            self.envelope(
                Namespace::Source,
                capture_id,
                ExpectedHeads::default(),
                Record::Capture(Box::new(capture)),
                media,
            )?,
        ];
        let root = self.envelope(
            Namespace::Conversation,
            root.id,
            expected_root,
            Record::Root(root),
            vec![],
        )?;
        self.publish(operation, fingerprint, root, records, binding)?;
        self.stored_sdk_images(capture_id)
    }
    /// Original facts and bytes only; restored records cannot become an SDK guard.
    pub fn stored_sdk_images(&self, capture_id: Uuid) -> Result<PreparedSdkCapture> {
        self.complete_store()?;
        let envelope = self
            .store
            .value(Namespace::Source, capture_id)?
            .context("historical capture absent")?;
        let Record::Capture(capture) = Self::decode(&envelope)? else {
            bail!("not an SDK capture record");
        };
        self.live_root(capture.conversation, None)?;
        let turn = self
            .store
            .value(Namespace::Conversation, capture.turn)?
            .context("capture turn absent")?;
        let Record::Turn(turn) = Self::decode(&turn)? else {
            bail!("invalid capture turn");
        };
        ensure!(
            turn.conversation == capture.conversation
                && turn.sources.contains(&capture.id)
                && turn.role == Role::User
                && matches!(turn.outcome, Outcome::Prepared | Outcome::Interpreted),
            "capture request is unavailable or terminal"
        );
        let load = |image: &capture_facts::HistoricalImage, ordinal| -> Result<PreparedSdkImage> {
            let media = image.media();
            let bytes = self.store.read_object(&ObjectRef {
                sha256: media.sha256.clone(),
                bytes: media.bytes,
            })?;
            Self::validate_image_bytes(&media, &bytes, image.dimensions[0], image.dimensions[1])?;
            Ok(PreparedSdkImage {
                ordinal,
                media,
                bytes,
            })
        };
        let native_parent = load(&capture.facts.native_parent, None)?;
        let images = capture
            .facts
            .images
            .iter()
            .enumerate()
            .map(|(ordinal, image)| load(image, Some(ordinal as u32)))
            .collect::<Result<Vec<_>>>()?;
        Ok(PreparedSdkCapture {
            turn: capture.turn,
            capture: capture.id,
            native_parent,
            images,
        })
    }
    /// Freeze the full inference batch before the caller may dispatch a provider.
    /// Retries use receipt/source records; no recapture or effect is performed.
    pub fn prepare_request(
        &self,
        operation: Uuid,
        expected: ExpectedHeads,
        turn: Turn,
        images: Vec<ImageInput>,
    ) -> Result<PreparedImages> {
        ensure!(
            turn.role == Role::User && turn.outcome == Outcome::Prepared,
            "preparation requires an unverified request"
        );
        ensure!(
            !images.is_empty() && images.len() <= MAX_ITEMS,
            "invalid inference batch bound"
        );
        let sources = images
            .iter()
            .map(|input| input.source.clone())
            .collect::<Vec<_>>();
        let fingerprint = Self::fingerprint(&("append", &turn, &sources))?;
        if self.replay(operation, &fingerprint)?.is_none() {
            self.live_root(turn.conversation, Some(&expected))?;
            for input in &images {
                Self::validate_source(&input.source, &turn)?;
                Self::validate_image_bytes(
                    &input.source.image,
                    &input.bytes,
                    input.source.width,
                    input.source.height,
                )?;
                match (
                    &input.source.parent,
                    input.source.parent_dimensions,
                    &input.parent_bytes,
                ) {
                    (Some(media), Some([width, height]), Some(bytes)) => {
                        Self::validate_image_bytes(media, bytes, width, height)?
                    }
                    (None, None, None) => {}
                    _ => bail!("complete parent bytes required for preparation"),
                }
            }
            for input in &images {
                self.stage_image(&input.source.image, &input.bytes[..])?;
                if let (Some(media), Some(bytes)) = (&input.source.parent, &input.parent_bytes) {
                    self.stage_image(media, &bytes[..])?;
                }
            }
            self.append(operation, expected, turn.clone(), sources)?;
        }
        self.prepared_images(turn.id)
    }
    fn validate_image_bytes(media: &Media, bytes: &[u8], width: u32, height: u32) -> Result<()> {
        media.validate()?;
        ensure!(
            media.bytes == bytes.len() as u64 && media.sha256 == digest(bytes),
            "image content identity mismatch"
        );
        let format = match media.media_type.as_str() {
            "image/png" => image::ImageFormat::Png,
            "image/jpeg" => image::ImageFormat::Jpeg,
            _ => bail!("unsupported inference image MIME"),
        };
        let dimensions = image::ImageReader::with_format(std::io::Cursor::new(bytes), format)
            .into_dimensions()?;
        ensure!(dimensions == (width, height), "image dimensions mismatch");
        const MAX_DECODED_BYTES: u64 = 32 * 1024 * 1024;
        ensure!(
            width <= 8192
                && height <= 8192
                && u64::from(width)
                    .checked_mul(u64::from(height))
                    .and_then(|n| n.checked_mul(4))
                    .is_some_and(|n| n <= MAX_DECODED_BYTES),
            "inference image exceeds decoded bound"
        );
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(8192);
        limits.max_image_height = Some(8192);
        limits.max_alloc = Some(MAX_DECODED_BYTES);
        let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
        reader.limits(limits);
        let decoded = reader
            .decode()
            .context("inference image pixels unavailable")?;
        ensure!(
            (decoded.width(), decoded.height()) == dimensions,
            "decoded image dimensions changed"
        );
        Ok(())
    }
    pub fn prepared_images(&self, turn: Uuid) -> Result<PreparedImages> {
        self.complete_store()?;
        let e = self
            .store
            .value(Namespace::Conversation, turn)?
            .context("prepared request absent")?;
        let Record::Turn(turn) = Self::decode(&e)? else {
            bail!("invalid request record");
        };
        self.live_root(turn.conversation, None)?;
        ensure!(
            turn.role == Role::User
                && matches!(turn.outcome, Outcome::Prepared | Outcome::Interpreted),
            "request batch is terminal"
        );
        let mut ordered = Vec::new();
        let mut ordinals = BTreeSet::new();
        for id in &turn.sources {
            let e = self
                .store
                .value(Namespace::Source, *id)?
                .context("request source absent")?;
            let Record::Source(source) = Self::decode(&e)? else {
                bail!("invalid source record");
            };
            ensure!(
                source.conversation == turn.conversation
                    && source.turn == turn.id
                    && ordinals.insert(source.ordinal),
                "ambiguous request batch"
            );
            let bytes = self.image(*id)?;
            Self::validate_image_bytes(&source.image, &bytes, source.width, source.height)?;
            ordered.push((source.ordinal, *id, bytes));
        }
        ordered.sort_by_key(|(ordinal, _, _)| *ordinal);
        ensure!(!ordered.is_empty(), "empty prepared batch");
        Ok(PreparedImages {
            turn: turn.id,
            images: ordered
                .into_iter()
                .map(|(_, id, bytes)| (id, bytes))
                .collect(),
        })
    }
    fn validate_correction_target(&self, turn: &Turn) -> Result<()> {
        if let Some(corrected) = turn.correction_of {
            let envelope = self
                .store
                .value(Namespace::Conversation, corrected)?
                .context("correction target absent")?;
            let Record::Turn(original) = Self::decode(&envelope)? else {
                bail!("invalid correction target");
            };
            ensure!(
                original.conversation == turn.conversation && turn.role == Role::User,
                "correction outside conversation"
            );
        }
        Ok(())
    }
    fn validate_turn(turn: &Turn) -> Result<()> {
        ensure!(
            !turn.id.is_nil() && !turn.conversation.is_nil() && !turn.exchange.is_nil(),
            "nil turn identity"
        );
        ensure!(
            turn.sources.len() <= MAX_ITEMS
                && turn.sources.iter().all(|id| !id.is_nil())
                && turn.sources.iter().copied().collect::<BTreeSet<_>>().len()
                    == turn.sources.len(),
            "invalid source links"
        );
        ensure!(
            turn.correction_of
                .is_none_or(|id| !id.is_nil() && id != turn.id),
            "invalid correction identity"
        );
        ensure!(
            turn.text.as_ref().is_none_or(|s| !s.is_empty()),
            "empty recorded text"
        );
        match turn.outcome {
            Outcome::Prepared => ensure!(turn.text.is_none(), "prepared text is unverified"),
            Outcome::Interpreted => ensure!(
                turn.role == Role::User && turn.text.is_some(),
                "invalid interpreted request"
            ),
            Outcome::Generated => ensure!(
                turn.role == Role::Assistant && turn.text.is_some(),
                "invalid generated draft"
            ),
            Outcome::Completed => {
                let evidence = turn
                    .completion
                    .as_ref()
                    .context("completed turn lacks verification evidence")?;
                ensure!(
                    turn.role == Role::Assistant
                        && turn.text.as_deref() == Some(evidence.exact_text.as_str())
                        && !evidence.exact_text.is_empty(),
                    "completion text mismatch"
                );
                ensure!(
                    !evidence.native_operation.is_nil() && !evidence.procedure.is_empty(),
                    "incomplete completion evidence"
                );
                Self::validate_observation(&evidence.observation)?;
            }
            _ => {}
        }
        ensure!(
            turn.outcome == Outcome::Completed || turn.completion.is_none(),
            "verification on noncompleted turn"
        );
        Ok(())
    }
    fn validate_source(source: &ImageUse, turn: &Turn) -> Result<()> {
        ensure!(
            !source.id.is_nil()
                && source.conversation == turn.conversation
                && source.turn == turn.id,
            "source identity mismatch"
        );
        Self::validate_source_facts(source)
    }
    fn validate_source_facts(source: &ImageUse) -> Result<()> {
        ensure!(
            !source.id.is_nil() && !source.conversation.is_nil() && !source.turn.is_nil(),
            "nil source use identity"
        );
        Self::validate_observation(&source.observation)?;
        ensure!(
            source.width > 0 && source.height > 0 && !source.purpose.is_empty(),
            "invalid source image geometry"
        );
        ensure!(
            source.transform.iter().all(|v| v.is_finite()),
            "invalid viewport transform"
        );
        let [a, b, c, d, _, _] = source.transform;
        let determinant = a * d - b * c;
        ensure!(
            determinant.is_finite() && determinant != 0.0,
            "singular viewport transform"
        );
        if let Some(target) = source.target {
            ensure!(
                target
                    .iter()
                    .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
                "invalid normalized target"
            );
        }
        ensure!(
            source.crop.is_some() == source.parent.is_some()
                && source.parent_dimensions.is_some() == source.parent.is_some(),
            "crop requires parent provenance"
        );
        if let (Some([x, y, width, height]), Some([parent_width, parent_height])) =
            (source.crop, source.parent_dimensions)
        {
            ensure!(
                width > 0
                    && height > 0
                    && x.checked_add(width)
                        .is_some_and(|right| right <= parent_width)
                    && y.checked_add(height)
                        .is_some_and(|bottom| bottom <= parent_height),
                "crop outside parent"
            );
        }
        source.image.validate()?;
        if let Some(parent) = &source.parent {
            parent.validate()?;
            if parent.sha256 == source.image.sha256 {
                ensure!(
                    parent == &source.image,
                    "inconsistent parent content identity"
                );
            }
        }
        Ok(())
    }
    fn validate_observation(o: &SourceObservation) -> Result<()> {
        ensure!(
            !o.document.is_nil()
                && !o.page.is_nil()
                && !o.session.is_empty()
                && !o.visit.is_empty(),
            "incomplete source identity"
        );
        ensure!(
            !o.capability_revision.is_empty() && !o.evidence_procedure.is_empty(),
            "missing capability provenance"
        );
        Ok(())
    }
    /// Advance a recorded attempt; this records facts and never invokes an effect.
    pub fn advance(
        &self,
        operation: Uuid,
        expected_root: ExpectedHeads,
        expected_turn: ExpectedHeads,
        turn: Turn,
    ) -> Result<WriteResult> {
        self.advance_fact(operation, expected_root, expected_turn, turn, None)
    }
    /// Fact-only outcome recording. Never renders, navigates or replays effects.
    pub fn record_attempt_outcome(
        &self,
        operation: Uuid,
        expected_root: ExpectedHeads,
        expected_turn: ExpectedHeads,
        turn: Turn,
        reason: AttemptReason,
    ) -> Result<WriteResult> {
        ensure!(
            matches!(
                turn.outcome,
                Outcome::Failed | Outcome::Canceled | Outcome::ReconcileRequired
            ),
            "attempt outcome must remain unverified"
        );
        self.advance_fact(operation, expected_root, expected_turn, turn, Some(reason))
    }
    fn advance_fact(
        &self,
        operation: Uuid,
        expected_root: ExpectedHeads,
        expected_turn: ExpectedHeads,
        turn: Turn,
        reason: Option<AttemptReason>,
    ) -> Result<WriteResult> {
        let fingerprint = if let Some(reason) = &reason {
            Self::fingerprint(&("attempt-outcome", &turn, reason))?
        } else {
            Self::fingerprint(&("advance", &turn))?
        };
        if let Some(prior) = self.replay(operation, &fingerprint)? {
            return Ok(prior);
        }
        Self::validate_turn(&turn)?;
        let (_, mut root) = self.live_root(turn.conversation, Some(&expected_root))?;
        let envelope = self
            .store
            .value(Namespace::Conversation, turn.id)?
            .context("turn absent")?;
        ensure!(
            expected_turn.0 == BTreeSet::from([envelope.revision_id]),
            "stale turn heads"
        );
        let Record::Turn(original) = Self::decode(&envelope)? else {
            bail!("invalid turn record");
        };
        ensure!(
            original.id == turn.id
                && original.conversation == turn.conversation
                && original.exchange == turn.exchange
                && original.sequence == turn.sequence
                && original.role == turn.role
                && original.mode == turn.mode
                && original.sources == turn.sources
                && original.correction_of == turn.correction_of
                && original.created_ms == turn.created_ms,
            "immutable turn facts changed"
        );
        let permitted = matches!(
            (&original.outcome, &turn.outcome),
            (
                Outcome::Prepared,
                Outcome::Interpreted
                    | Outcome::Failed
                    | Outcome::Canceled
                    | Outcome::ReconcileRequired,
            ) | (
                Outcome::Generated,
                Outcome::Completed
                    | Outcome::Failed
                    | Outcome::Canceled
                    | Outcome::ReconcileRequired,
            )
        );
        let explicit_reconciliation = original.outcome == Outcome::ReconcileRequired
            && matches!(
                (&turn.outcome, &reason),
                (
                    Outcome::Failed,
                    Some(AttemptReason::NoSuccessor | AttemptReason::InvalidSuccessor),
                ) | (
                    Outcome::ReconcileRequired,
                    Some(AttemptReason::SubmittedUnverified | AttemptReason::DeviceUncertain),
                )
            );
        ensure!(
            permitted || explicit_reconciliation,
            "invalid outcome transition; reconciliation is explicit"
        );
        if matches!(
            original.outcome,
            Outcome::Generated | Outcome::ReconcileRequired
        ) {
            ensure!(original.text == turn.text, "generated draft text changed");
        }
        let outcome = turn.outcome.clone();
        let turn_id = turn.id;
        root.updated_ms = turn.updated_ms;
        let binding = root.binding;
        let updated = self.envelope(
            Namespace::Conversation,
            turn.id,
            expected_turn,
            Record::Turn(turn),
            vec![],
        )?;
        let root = self.envelope(
            Namespace::Conversation,
            root.id,
            expected_root,
            Record::Root(root),
            vec![],
        )?;
        let mut records = vec![updated];
        if let Some(reason) = reason {
            let id = Uuid::new_v5(
                &Uuid::NAMESPACE_URL,
                format!("urn:remarkable-buddies:outcome:v1:{operation}").as_bytes(),
            );
            records.push(self.envelope(
                Namespace::Conversation,
                id,
                ExpectedHeads::default(),
                Record::OutcomeFact(OutcomeFact {
                    id,
                    conversation: root.record_id,
                    turn: turn_id,
                    outcome,
                    reason,
                    settlement: None,
                }),
                vec![],
            )?);
        }
        self.publish(operation, fingerprint, root, records, binding)
    }
    /// Store a REM-25 association receipt. SDK qualification remains a caller gate;
    /// no imported or synthetic record authorizes native reuse.
    pub fn bind(
        &self,
        operation: Uuid,
        expected_root: ExpectedHeads,
        expected_binding: ExpectedHeads,
        receipt: BindingReceipt,
    ) -> Result<WriteResult> {
        let fingerprint = Self::fingerprint(&("bind", &receipt))?;
        if let Some(prior) = self.replay(operation, &fingerprint)? {
            return Ok(prior);
        }
        ensure!(receipt.operation == operation, "receipt operation mismatch");
        Self::validate_observation(&receipt.source)?;
        ensure!(
            receipt.intended_target == Some(receipt.observed_page),
            "target mismatch requires reconciliation"
        );
        ensure!(
            !receipt.observed_document.is_nil()
                && !receipt.observed_page.is_nil()
                && receipt.observed_document == receipt.source.document,
            "invalid target identity"
        );
        ensure!(
            !receipt.request_fingerprint.is_empty()
                && !receipt.adapter_fingerprint.is_empty()
                && !receipt.persisted_revision.is_empty()
                && !receipt.evidence_origin.is_empty(),
            "incomplete binding evidence"
        );
        let unique = receipt
            .expected_order
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        ensure!(
            !unique.is_empty()
                && unique.len() == receipt.expected_order.len()
                && unique.contains(&receipt.source.page)
                && !unique.contains(&receipt.observed_page)
                && !unique.contains(&Uuid::nil()),
            "ambiguous source order"
        );
        let mut permitted = receipt.expected_order.clone();
        let position = permitted
            .iter()
            .position(|id| *id == receipt.source.page)
            .context("source outside order")?;
        permitted.insert(position + 1, receipt.observed_page);
        ensure!(
            permitted == receipt.observed_order,
            "insertion receipt order mismatch"
        );
        let id = Self::binding_id(receipt.observed_document, receipt.observed_page);
        let (_, mut root) = self.live_root(receipt.conversation, Some(&expected_root))?;
        ensure!(
            root.binding.is_none_or(|prior| prior == id),
            "conversation already claims another page"
        );
        let requested = Binding {
            id,
            conversation: receipt.conversation,
            receipt,
        };
        let heads = self.store.heads(Namespace::Conversation, id)?;
        ensure!(
            heads.iter().map(|e| e.revision_id).collect::<BTreeSet<_>>() == expected_binding.0,
            "stale binding heads"
        );
        ensure!(heads.len() <= 1, "conflicted binding");
        if let Some(prior) = heads.first() {
            ensure!(
                Self::decode(prior)? == Record::Binding(requested.clone()),
                "page already claimed or deleted"
            );
        }
        root.binding = Some(id);
        root.updated_ms = requested.receipt.observed_ms;
        let binding = self.envelope(
            Namespace::Conversation,
            id,
            expected_binding,
            Record::Binding(requested),
            vec![],
        )?;
        let root = self.envelope(
            Namespace::Conversation,
            root.id,
            expected_root,
            Record::Root(root),
            vec![],
        )?;
        self.publish(operation, fingerprint, root, vec![binding], Some(id))
    }
    fn tombstone(
        &self,
        namespace: Namespace,
        id: Uuid,
        parents: ExpectedHeads,
    ) -> Result<Envelope> {
        let e = Envelope {
            envelope_version: FORMAT,
            namespace,
            domain_schema_version: SCHEMA,
            record_id: id,
            revision_id: Uuid::new_v4(),
            parents: parents.0,
            operation_id: Uuid::new_v4(),
            actor_id: self.store.actor_id,
            kind: Kind::Tombstone,
            payload: serde_json::Value::Null,
            media_descriptors: vec![],
        };
        e.validate()?;
        Ok(e)
    }
    pub fn delete(
        &self,
        conversation: Uuid,
        operation: Uuid,
        expected_root: ExpectedHeads,
        expected_binding: Option<ExpectedHeads>,
    ) -> Result<WriteResult> {
        let fingerprint = Self::fingerprint(&("delete", conversation))?;
        if let Some(prior) = self.replay(operation, &fingerprint)? {
            return Ok(prior);
        }
        let (_, root) = self.live_root(conversation, Some(&expected_root))?;
        let mut records = vec![];
        match (root.binding, expected_binding) {
            (Some(id), Some(expected)) => {
                let heads = self.store.heads(Namespace::Conversation, id)?;
                ensure!(
                    heads.len() == 1
                        && heads.iter().map(|e| e.revision_id).collect::<BTreeSet<_>>()
                            == expected.0,
                    "stale or conflicted binding"
                );
                ensure!(
                    matches!(Self::decode(&heads[0])?,Record::Binding(b) if b.conversation==conversation),
                    "foreign binding"
                );
                records.push(self.tombstone(Namespace::Conversation, id, expected)?);
            }
            (None, None) => {}
            _ => bail!("complete binding heads required"),
        }
        let deleted = self.tombstone(Namespace::Conversation, conversation, expected_root)?;
        self.publish(operation, fingerprint, deleted, records, root.binding)
    }
    pub fn associate_export(
        &self,
        operation: Uuid,
        expected_root: ExpectedHeads,
        expected_export: ExpectedHeads,
        association: ExportAssociation,
    ) -> Result<WriteResult> {
        let fingerprint = Self::fingerprint(&("export", &association))?;
        if let Some(prior) = self.replay(operation, &fingerprint)? {
            return Ok(prior);
        }
        ensure!(
            association.operation == operation
                && !association.id.is_nil()
                && !association.backend.is_empty()
                && !association.source_scope.is_empty()
                && !association.source_revision.is_empty(),
            "incomplete export association"
        );
        ensure!(
            matches!(
                association.outcome,
                Outcome::Prepared
                    | Outcome::Completed
                    | Outcome::Failed
                    | Outcome::Canceled
                    | Outcome::ReconcileRequired
            ),
            "invalid export outcome"
        );
        if association.outcome == Outcome::Completed {
            ensure!(
                association
                    .native_note
                    .as_ref()
                    .is_some_and(|s| !s.is_empty()),
                "completed export lacks note identity"
            );
        }
        let (_, root) = self.live_root(association.conversation, Some(&expected_root))?;
        let prior = self
            .store
            .value(Namespace::ExportAssociation, association.id)?;
        ensure!(
            prior.iter().map(|e| e.revision_id).collect::<BTreeSet<_>>() == expected_export.0,
            "stale export heads"
        );
        if let Some(prior) = prior {
            let Record::Export(old) = Self::decode(&prior)? else {
                bail!("invalid export record");
            };
            ensure!(
                old.conversation == association.conversation
                    && old.backend == association.backend
                    && old.source_scope == association.source_scope
                    && old.source_revision == association.source_revision,
                "export immutable scope changed"
            );
            ensure!(
                old.outcome == Outcome::Prepared || old.outcome == Outcome::ReconcileRequired,
                "export already terminal"
            );
        }
        let binding = root.binding;
        let exported = self.envelope(
            Namespace::ExportAssociation,
            association.id,
            expected_export,
            Record::Export(association),
            vec![],
        )?;
        let root = self.envelope(
            Namespace::Conversation,
            root.id,
            expected_root,
            Record::Root(root),
            vec![],
        )?;
        self.publish(operation, fingerprint, root, vec![exported], binding)
    }
    pub fn image(&self, source: Uuid) -> Result<Vec<u8>> {
        self.complete_store()?;
        let e = self
            .store
            .value(Namespace::Source, source)?
            .context("source use absent")?;
        let Record::Source(source) = Self::decode(&e)? else {
            bail!("invalid source record");
        };
        self.live_root(source.conversation, None)?;
        let bytes = self.store.read_object(&ObjectRef {
            sha256: source.image.sha256.clone(),
            bytes: source.image.bytes,
        })?;
        Self::validate_image_bytes(&source.image, &bytes, source.width, source.height)?;
        Ok(bytes)
    }
    /// Exact chronological facts. Drafts and machine instructions are not visible turns.
    pub fn inspect(&self, conversation: Uuid, retained: bool) -> Result<Vec<Record>> {
        self.complete_store()?;
        let identity = conversation.to_string();
        let snapshot = self.store.snapshot_heads_matching(
            &[
                Namespace::Conversation,
                Namespace::Source,
                Namespace::ExportAssociation,
            ],
            MAX_ITEMS,
            |e| {
                let payload = &e.payload["record"];
                payload["conversation"].as_str() == Some(&identity)
                    || payload["acknowledgment"]["conversation"].as_str() == Some(&identity)
                    || (e.namespace == Namespace::Conversation && e.record_id == conversation)
            },
        )?;
        let roots: Vec<_> = snapshot
            .iter()
            .filter(|e| e.namespace == Namespace::Conversation && e.record_id == conversation)
            .collect();
        ensure!(roots.len() == 1, "absent or conflicted conversation root");
        if !retained {
            ensure!(
                matches!(Self::decode(roots[0])?,Record::Root(r) if r.id==conversation),
                "invalid root"
            );
        }
        let mut records = Vec::new();
        for e in &snapshot {
            if e.kind == Kind::Tombstone {
                continue;
            }
            let payload = &e.payload["record"];
            let belongs = payload["conversation"].as_str() == Some(&identity)
                || payload["acknowledgment"]["conversation"].as_str() == Some(&identity)
                || (e.record_id == conversation && e.namespace == Namespace::Conversation);
            if !belongs {
                continue;
            }
            ensure!(snapshot.iter().filter(|other|other.namespace==e.namespace && other.record_id==e.record_id).count()==1,"conflicted conversation descendant");
            records.push(Self::decode(e)?);
        }
        let next_sequence = records.iter().find_map(|record| match record {
            Record::Root(root) => Some(root.next_sequence),
            _ => None,
        });
        let mut sequences = BTreeSet::new();
        for record in &records {
            if let Record::Turn(turn) = record {
                ensure!(
                    sequences.insert(turn.sequence),
                    "duplicate conversation chronology"
                );
                if let Some(next) = next_sequence {
                    ensure!(turn.sequence < next, "turn exceeds allocated chronology");
                }
            }
        }
        if let Some(next) = next_sequence {
            ensure!(
                sequences.len() as u64 == next,
                "allocated conversation chronology is incomplete"
            );
        }
        records.sort_by_key(|r| match r {
            Record::Turn(t) => (0, t.sequence),
            _ => (1, 0),
        });
        Ok(records)
    }
    pub fn context(
        &self,
        conversation: Uuid,
        budget: &ContextBudget,
        token_count: impl Fn(&[Turn]) -> Result<usize>,
    ) -> Result<ContextView> {
        self.context_range(conversation, None, budget, token_count)
    }
    /// The caller may choose an explicit sequence range; history is never rewritten.
    pub fn context_range(
        &self,
        conversation: Uuid,
        selection: Option<TurnRange>,
        budget: &ContextBudget,
        token_count: impl Fn(&[Turn]) -> Result<usize>,
    ) -> Result<ContextView> {
        if let Some(range) = selection {
            ensure!(
                range.start < range.end_exclusive,
                "invalid explicit turn range"
            );
        }
        let token_limit = budget
            .provider_token_limit
            .ok_or(ContextRefusal::UnknownProviderBudget)?;
        let records = self.inspect(conversation, false)?;
        let turns: Vec<Turn> = records
            .iter()
            .filter_map(|r| match r {
                Record::Turn(t)
                    if (t.outcome == Outcome::Completed || t.outcome == Outcome::Interpreted)
                        && selection.is_none_or(|range| {
                            t.sequence >= range.start && t.sequence < range.end_exclusive
                        }) =>
                {
                    Some(t.clone())
                }
                _ => None,
            })
            .collect();
        let bytes = turns.iter().try_fold(0usize, |s, t| {
            s.checked_add(t.text.as_ref().map_or(0, String::len))
                .context("context byte overflow")
        })?;
        if turns.len() > budget.max_turns || bytes > budget.max_text_bytes {
            return Err(ContextRefusal::SelectionRequired {
                turns: turns.len(),
                text_bytes: bytes,
                tokens: None,
            }
            .into());
        }
        let tokens = token_count(&turns)?;
        if tokens > token_limit {
            return Err(ContextRefusal::SelectionRequired {
                turns: turns.len(),
                text_bytes: bytes,
                tokens: Some(tokens),
            }
            .into());
        }
        let source_ids = turns
            .iter()
            .flat_map(|t| t.sources.iter())
            .collect::<BTreeSet<_>>();
        let mut sources = Vec::new();
        let mut captures = Vec::new();
        let mut legacy_captures = Vec::new();
        let mut development_captures = Vec::new();
        for record in records {
            match record {
                Record::Source(source) if source_ids.contains(&source.id) => sources.push(source),
                Record::Capture(capture) if source_ids.contains(&capture.id) => {
                    captures.push(*capture)
                }
                Record::DevelopmentCapture(capture) if source_ids.contains(&capture.id) => {
                    development_captures.push(*capture)
                }
                Record::LegacyCapture(capture) if source_ids.contains(&capture.id) => {
                    legacy_captures.push(capture)
                }
                _ => {}
            }
        }
        ensure!(
            sources.len() + captures.len() + legacy_captures.len() + development_captures.len()
                == source_ids.len(),
            "source reference absent"
        );
        let mut missing_media = Vec::new();
        let mut capture_media = Vec::new();
        for capture in &captures {
            capture_media.extend(capture.facts.media()?);
        }
        for capture in &development_captures {
            capture_media.extend(capture.media()?);
        }
        for capture in &legacy_captures {
            capture_media.extend(capture.media()?);
        }
        for media in &capture_media {
            if self
                .store
                .open_object(&ObjectRef {
                    sha256: media.sha256.clone(),
                    bytes: media.bytes,
                })
                .is_err()
                && !missing_media.contains(media)
            {
                missing_media.push(media.clone());
            }
        }
        for source in &sources {
            for media in std::iter::once(&source.image).chain(source.parent.iter()) {
                if self
                    .store
                    .open_object(&ObjectRef {
                        sha256: media.sha256.clone(),
                        bytes: media.bytes,
                    })
                    .is_err()
                    && !missing_media.contains(media)
                {
                    missing_media.push(media.clone());
                }
            }
        }
        Ok(ContextView {
            turns,
            sources,
            captures,
            legacy_captures,
            development_captures,
            missing_media,
            selection,
        })
    }
    /// Explicit retained-history inspection reports bytes; logical delete is not erasure.
    pub fn retained_media(&self, conversation: Uuid) -> Result<Vec<RetainedMedia>> {
        self.inspect(conversation, true)?;
        let identity = conversation.to_string();
        let revisions = self.store.snapshot_revisions_matching(
            &[Namespace::Source],
            MAX_ITEMS,
            |envelope| envelope.payload["record"]["conversation"].as_str() == Some(&identity),
        )?;
        let mut media = BTreeMap::new();
        for envelope in revisions {
            let references = match Self::decode(&envelope)? {
                Record::Source(source) => {
                    std::iter::once(source.image).chain(source.parent).collect()
                }
                Record::Capture(capture) => capture.facts.media()?,
                Record::LegacyCapture(capture) => capture.media()?,
                Record::DevelopmentCapture(capture) => capture.media()?,
                _ => vec![],
            };
            for item in references {
                if let Some(prior) = media.insert(item.sha256.clone(), item.clone()) {
                    ensure!(prior == item, "inconsistent media descriptors");
                }
            }
        }
        media
            .into_values()
            .map(|media| {
                let retained_revision_references = self.store.media_references(&media)?;
                let available = self
                    .store
                    .open_object(&ObjectRef {
                        sha256: media.sha256.clone(),
                        bytes: media.bytes,
                    })
                    .is_ok();
                Ok(RetainedMedia {
                    media,
                    retained_revision_references,
                    available,
                })
            })
            .collect()
    }
    /// Decide from explicit verified discovery, without retrying an external effect.
    pub fn reconcile_export(
        &self,
        association: Uuid,
        matches: &[ExportMatch],
    ) -> Result<ExportReconciliation> {
        ensure!(matches.len() <= MAX_ITEMS, "export discovery exceeds bound");
        let e = self
            .store
            .value(Namespace::ExportAssociation, association)?
            .context("export association absent")?;
        let Record::Export(association) = Self::decode(&e)? else {
            bail!("invalid export association");
        };
        self.live_root(association.conversation, None)?;
        let mut notes = BTreeSet::new();
        for found in matches {
            ensure!(
                found.marker == association.id
                    && found.backend == association.backend
                    && found.scope == association.source_scope
                    && found.revision == association.source_revision
                    && !found.native_note.is_empty(),
                "unverified export discovery identity"
            );
            notes.insert(found.native_note.clone());
        }
        Ok(match notes.len() {
            0 => ExportReconciliation::Uncertain,
            1 => ExportReconciliation::Unique(
                notes.into_iter().next().context("export match absent")?,
            ),
            _ => ExportReconciliation::Conflict(notes.into_iter().collect()),
        })
    }
}

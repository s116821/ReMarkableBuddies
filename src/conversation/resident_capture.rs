//! Distinct resident historical evidence; never native output or binding authority.
use super::capture_facts::ExactU64;
use super::*;
use remarkable_open_sdk::native_adapter::{
    HistoricalCapture, ReaderFacts, RuntimeIdentity, CAPTURE_PROCEDURE, MAX_CAPTURE_PIXELS,
    MAX_CAPTURE_PNG,
};
use remarkable_open_sdk::{OperationId, PageKey, Uuid as SdkUuid};
use serde::{Deserialize, Serialize};
use sha2::Digest;

pub const RESIDENT_SDK_SOURCE: &str = "f03cc7227d6b32213d6d49ae8c61a1cef6d43711";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResidentFacts {
    pub document: Uuid,
    pub page: Uuid,
    pub order: Vec<Uuid>,
}
impl From<&ReaderFacts> for ResidentFacts {
    fn from(facts: &ReaderFacts) -> Self {
        Self {
            document: Uuid::from_bytes(*facts.page().document.as_bytes()),
            page: Uuid::from_bytes(*facts.page().page.as_bytes()),
            order: facts
                .order()
                .iter()
                .map(|id| Uuid::from_bytes(*id.as_bytes()))
                .collect(),
        }
    }
}
impl ResidentFacts {
    fn sdk(&self) -> Result<ReaderFacts> {
        ensure!(
            !self.document.is_nil() && !self.page.is_nil() && self.order.len() <= 17,
            "invalid resident historical facts"
        );
        let id = |value: Uuid| {
            SdkUuid::parse(&value.to_string()).context("invalid resident historical UUID")
        };
        ReaderFacts::new(
            PageKey {
                document: id(self.document)?,
                page: id(self.page)?,
            },
            self.order
                .iter()
                .map(|value| id(*value))
                .collect::<Result<Vec<_>>>()?,
        )
        .map_err(|e| anyhow::anyhow!("{e:?}"))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResidentCorrespondence {
    pub boot: Uuid,
    pub activation: Uuid,
    pub pid: u32,
    pub start_ticks: ExactU64,
    pub profile_sha256: String,
    pub executable_sha256: String,
    pub adapter_sha256: String,
    pub observe_operation: Uuid,
    pub capture_operation: Uuid,
    pub expected: ResidentFacts,
}
impl ResidentCorrespondence {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.boot.is_nil()
                && !self.activation.is_nil()
                && self.pid > 1
                && self.start_ticks.value() > 0
                && !self.observe_operation.is_nil()
                && !self.capture_operation.is_nil()
                && self.observe_operation != self.capture_operation,
            "invalid resident correspondence identity"
        );
        for hash in [
            &self.profile_sha256,
            &self.executable_sha256,
            &self.adapter_sha256,
        ] {
            ensure!(
                crate::storage::valid_digest(hash) && *hash != "00".repeat(32),
                "invalid resident artifact digest"
            );
        }
        self.expected.sdk()?;
        Ok(())
    }
}
/// Constructed only by the in-crate admitted Session source route. Construction
/// verifies correspondence; neither this nor its stored form grants live authority.
pub struct ResidentSourceCapture {
    pub(super) capture: HistoricalCapture,
    pub(super) correspondence: ResidentCorrespondence,
}
impl ResidentSourceCapture {
    pub(crate) fn document(&self) -> Uuid {
        self.correspondence.expected.document
    }

    #[cfg(any(target_os = "linux", test))]
    pub(crate) fn page(&self) -> Uuid {
        self.correspondence.expected.page
    }

    pub(super) fn prepare_history(
        &self,
        conversation: Uuid,
        turn: Uuid,
        evidence_id: Uuid,
    ) -> Result<PreparedResidentCapture> {
        let provider_images = crate::device::screenshot::Screenshot::reader_images_from_owned_png(
            self.capture.png(),
        )?;
        let [w, h] = self.capture.dimensions();
        let th = h * 2 / 5;
        let crops = [
            [0, 0, w, h],
            [0, 0, w, th],
            [0, (h - th) / 2, w, th],
            [0, h - th, w, th],
        ];
        let evidence = ResidentCaptureHistory {
            schema: 1,
            sdk_source: RESIDENT_SDK_SOURCE.into(),
            id: evidence_id,
            conversation,
            turn,
            origin: ResidentOrigin::ResidentHistoricalUnqualified,
            qualification: (),
            correspondence: self.correspondence.clone(),
            before: self.capture.before().into(),
            after: self.capture.after().into(),
            dpr_bits: ExactU64::try_from(self.capture.device_pixel_ratio().to_bits().to_string())
                .map_err(anyhow::Error::msg)?,
            procedure: self.capture.procedure().into(),
            original_png: Media {
                sha256: digest(self.capture.png()),
                bytes: self.capture.png().len() as u64,
                media_type: "image/png".into(),
            },
            original_dimensions: self.capture.dimensions(),
            provider_procedure: "reader-nearest-overview-overlapping-strips-v1".into(),
            images: provider_images
                .iter()
                .enumerate()
                .map(|(ordinal, bytes)| ResidentImage {
                    media: Media {
                        sha256: digest(bytes),
                        bytes: bytes.len() as u64,
                        media_type: "image/png".into(),
                    },
                    dimensions: if ordinal == 0 { [768, 1024] } else { [w, th] },
                    provider_ordinal: ordinal as u32,
                    parent_crop: crops[ordinal],
                })
                .collect(),
        };
        evidence.validate()?;
        Ledger::validate_image_bytes(&evidence.original_png, self.capture.png(), w, h)?;
        for (image, bytes) in evidence.images.iter().zip(&provider_images) {
            Ledger::validate_image_bytes(
                &image.media,
                bytes,
                image.dimensions[0],
                image.dimensions[1],
            )?;
        }
        Ok(PreparedResidentCapture {
            evidence,
            original_png: self.capture.png().to_vec(),
            images: provider_images,
        })
    }

    pub(crate) fn from_session(
        runtime: &RuntimeIdentity,
        observe: OperationId,
        operation: OperationId,
        expected: ReaderFacts,
        capture: HistoricalCapture,
    ) -> Result<Self> {
        ensure!(
            capture.before() == &expected && capture.after() == &expected,
            "resident source samples differ from request"
        );
        let hex = |bytes: &[u8]| {
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let correspondence = ResidentCorrespondence {
            boot: Uuid::from_bytes(*runtime.boot.as_bytes()),
            activation: Uuid::from_bytes(*runtime.activation.as_bytes()),
            pid: runtime.pid,
            start_ticks: ExactU64::try_from(runtime.start_ticks.to_string())
                .map_err(anyhow::Error::msg)?,
            profile_sha256: hex(&runtime.profile_sha256),
            executable_sha256: hex(&runtime.executable_sha256),
            adapter_sha256: hex(&runtime.adapter_sha256),
            observe_operation: Uuid::from_bytes(*observe.0.as_bytes()),
            capture_operation: Uuid::from_bytes(*operation.0.as_bytes()),
            expected: (&expected).into(),
        };
        correspondence.validate()?;
        Ok(Self {
            capture,
            correspondence,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResidentOrigin {
    ResidentHistoricalUnqualified,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResidentImage {
    pub media: Media,
    pub dimensions: [u32; 2],
    pub provider_ordinal: u32,
    /// Locally derived crop in original PNG pixels, never SDK page geometry.
    pub parent_crop: [u32; 4],
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResidentCaptureHistory {
    pub schema: u32,
    pub sdk_source: String,
    pub id: Uuid,
    pub conversation: Uuid,
    pub turn: Uuid,
    pub origin: ResidentOrigin,
    #[serde(deserialize_with = "absent_qualification")]
    pub qualification: (),
    pub correspondence: ResidentCorrespondence,
    pub before: ResidentFacts,
    pub after: ResidentFacts,
    pub dpr_bits: ExactU64,
    pub procedure: String,
    pub original_png: Media,
    pub original_dimensions: [u32; 2],
    pub provider_procedure: String,
    /// Locally derived overview and three strips, in original provider order.
    pub images: Vec<ResidentImage>,
}
fn absent_qualification<'de, D: serde::Deserializer<'de>>(
    value: D,
) -> std::result::Result<(), D::Error> {
    <()>::deserialize(value)
}
pub struct PreparedResidentCapture {
    pub evidence: ResidentCaptureHistory,
    pub original_png: Vec<u8>,
    pub images: Vec<Vec<u8>>,
}
impl ResidentCaptureHistory {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == 1
                && self.sdk_source == RESIDENT_SDK_SOURCE
                && !self.id.is_nil()
                && !self.conversation.is_nil()
                && !self.turn.is_nil(),
            "invalid resident history identity/schema/source"
        );
        ensure!(
            self.images.len() == 4
                && self.provider_procedure == "reader-nearest-overview-overlapping-strips-v1",
            "resident provider order mismatch"
        );
        self.correspondence.validate()?;
        ensure!(
            self.before == self.correspondence.expected
                && self.after == self.before
                && self.procedure == CAPTURE_PROCEDURE,
            "resident historical source mismatch"
        );
        self.before.sdk()?;
        let dpr = f64::from_bits(self.dpr_bits.value());
        ensure!(dpr.is_finite() && dpr > 0.0, "invalid resident DPR");
        self.original_png.validate()?;
        ensure!(
            self.original_png.bytes > 0
                && self.original_png.bytes <= MAX_CAPTURE_PNG as u64
                && self.original_png.media_type == "image/png"
                && self
                    .original_dimensions
                    .iter()
                    .all(|n| *n > 0 && *n <= 1872)
                && u64::from(self.original_dimensions[0]) * u64::from(self.original_dimensions[1])
                    <= MAX_CAPTURE_PIXELS,
            "invalid resident original PNG descriptor"
        );
        let [w, h] = self.original_dimensions;
        let th = h * 2 / 5;
        let crops = [
            [0, 0, w, h],
            [0, 0, w, th],
            [0, (h - th) / 2, w, th],
            [0, h - th, w, th],
        ];
        let mut total = self.original_png.bytes;
        for (ordinal, image) in self.images.iter().enumerate() {
            image.media.validate()?;
            ensure!(
                image.provider_ordinal == ordinal as u32
                    && image.parent_crop == crops[ordinal]
                    && image.dimensions == if ordinal == 0 { [768, 1024] } else { [w, th] }
                    && image.media.bytes > 0
                    && image.media.bytes <= 32 * 1024 * 1024
                    && image.media.media_type == "image/png",
                "invalid resident provider descriptor"
            );
            total = total
                .checked_add(image.media.bytes)
                .context("resident batch overflow")?;
        }
        ensure!(total <= 64 * 1024 * 1024, "resident encoded batch bound");
        self.media()?;
        Ok(())
    }
    pub(super) fn media(&self) -> Result<Vec<Media>> {
        let mut media = BTreeMap::new();
        for item in [&self.original_png]
            .into_iter()
            .chain(self.images.iter().map(|i| &i.media))
        {
            if let Some(prior) = media.insert(item.sha256.clone(), item.clone()) {
                ensure!(prior == *item, "inconsistent resident media descriptors");
            }
        }
        Ok(media.into_values().collect())
    }
}
impl Ledger {
    /// Resolve a stored answer from the freshly observed current page or its
    /// immediate successor. A successor must belong to this exact source.
    pub(crate) fn resident_answer_association(
        &self,
        capture: &ResidentSourceCapture,
    ) -> Result<Option<super::HistoricalPageAssociation>> {
        let facts = &capture.correspondence.expected;
        let index = facts
            .order
            .iter()
            .position(|page| *page == facts.page)
            .context("resident source absent from order")?;
        self.complete_store()?;
        for &page in facts.order.iter().skip(index).take(2) {
            let id = Self::binding_id(facts.document, page);
            let heads = self.store.heads(Namespace::Conversation, id)?;
            if heads.is_empty() {
                continue;
            }
            ensure!(
                heads.len() == 1,
                "resident answer binding has conflicting heads"
            );
            let envelope = &heads[0];
            if envelope.kind != Kind::Value {
                return Ok(None);
            }
            let Record::Binding(binding) = Self::decode(envelope)? else {
                bail!("resident page binding variant mismatch");
            };
            let receipt = &binding.receipt;
            if !matches!(
                receipt.evidence_origin.as_str(),
                "development-native-guarded-v1" | "installed-native-guarded-v1"
            ) || receipt.adapter_fingerprint != capture.correspondence.adapter_sha256
                || receipt.source.capability_revision != capture.correspondence.profile_sha256
                || receipt.observed_order != facts.order
                || (page != facts.page && receipt.source.page != facts.page)
            {
                return Ok(None);
            }
            match self.inspect_page_association(binding.conversation, facts.document, page)? {
                super::HistoricalPageAssociationInspection::HistoricalCandidate(association)
                    if association.binding_revision() == envelope.revision_id =>
                {
                    return Ok(Some(*association));
                }
                _ => bail!("resident answer association changed or is unresolved"),
            }
        }
        Ok(None)
    }

    /// Persist original resident receipt and PNG atomically with the Prepared turn.
    /// No source observation, native binding, selection token or effect context is made.
    pub fn prepare_resident_historical(
        &self,
        operation: Uuid,
        expected_root: ExpectedHeads,
        mut turn: Turn,
        evidence_id: Uuid,
        capture: &ResidentSourceCapture,
    ) -> Result<PreparedResidentCapture> {
        ensure!(
            turn.role == Role::User
                && turn.outcome == Outcome::Prepared
                && turn.sequence == 0
                && turn.sources.is_empty(),
            "invalid resident request"
        );
        Self::validate_turn(&turn)?;
        let PreparedResidentCapture {
            evidence,
            original_png,
            images: provider_images,
        } = capture.prepare_history(turn.conversation, turn.id, evidence_id)?;
        turn.sources = vec![evidence.id];
        let fingerprint = Self::fingerprint(&("prepare-resident", &turn, &evidence))?;
        if self.replay(operation, &fingerprint)?.is_some() {
            return self.stored_resident_images(evidence_id);
        }
        let (root_head, mut root) = self.live_root(turn.conversation, Some(&expected_root))?;
        if let Some(binding_id) = root.binding {
            let association = self
                .resident_answer_association(capture)?
                .context("resident request no longer matches its bound answer page")?;
            let receipt = association.receipt();
            ensure!(
                receipt.conversation == turn.conversation
                    && association.root_revision() == root_head.revision_id
                    && binding_id
                        == Self::binding_id(receipt.observed_document, receipt.observed_page),
                "resident answer association changed before preparing request"
            );
        }
        self.validate_correction_target(&turn)?;
        self.stage_image(&evidence.original_png, original_png.as_slice())?;
        for (image, bytes) in evidence.images.iter().zip(&provider_images) {
            self.stage_image(&image.media, bytes.as_slice())?;
        }
        turn.sequence = root.next_sequence;
        root.next_sequence = root
            .next_sequence
            .checked_add(1)
            .context("sequence exhausted")?;
        root.updated_ms = turn.updated_ms;
        let records = vec![
            self.envelope(
                Namespace::Conversation,
                turn.id,
                ExpectedHeads::default(),
                Record::Turn(turn),
                vec![],
            )?,
            self.envelope(
                Namespace::Source,
                evidence.id,
                ExpectedHeads::default(),
                Record::ResidentCapture(Box::new(evidence.clone())),
                evidence.media()?,
            )?,
        ];
        let root = self.envelope(
            Namespace::Conversation,
            root.id,
            expected_root,
            Record::Root(root),
            vec![],
        )?;
        self.publish(operation, fingerprint, root, records, None)?;
        self.stored_resident_images(evidence_id)
    }
    /// Historical byte retrieval only. Parser consistency cannot prove current ownership.
    pub fn stored_resident_images(&self, id: Uuid) -> Result<PreparedResidentCapture> {
        self.complete_store()?;
        let envelope = self
            .store
            .value(Namespace::Source, id)?
            .context("resident evidence absent")?;
        let Record::ResidentCapture(evidence) = Self::decode(&envelope)? else {
            bail!("not resident evidence")
        };
        self.live_root(evidence.conversation, None)?;
        let turn = self
            .store
            .value(Namespace::Conversation, evidence.turn)?
            .context("resident turn absent")?;
        let Record::Turn(turn) = Self::decode(&turn)? else {
            bail!("invalid resident turn")
        };
        ensure!(
            turn.conversation == evidence.conversation && turn.sources.contains(&id),
            "resident evidence link mismatch"
        );
        Self::resident_images_from_evidence(*evidence, |media| {
            self.store.read_object(&ObjectRef {
                sha256: media.sha256.clone(),
                bytes: media.bytes,
            })
        })
    }
    pub(super) fn resident_images_from_evidence(
        evidence: ResidentCaptureHistory,
        mut load: impl FnMut(&Media) -> Result<Vec<u8>>,
    ) -> Result<PreparedResidentCapture> {
        evidence.validate()?;
        let original_png = load(&evidence.original_png)?;
        let images = evidence
            .images
            .iter()
            .map(|i| {
                let bytes = load(&i.media)?;
                Self::validate_image_bytes(&i.media, &bytes, i.dimensions[0], i.dimensions[1])?;
                Ok(bytes)
            })
            .collect::<Result<Vec<_>>>()?;
        Self::validate_image_bytes(
            &evidence.original_png,
            &original_png,
            evidence.original_dimensions[0],
            evidence.original_dimensions[1],
        )?;
        // Revalidate bounded SDK historical pixels and full source order; never
        // reconstruct a live observation, v11 completion or output permission.
        HistoricalCapture::new(
            evidence.before.sdk()?,
            evidence.after.sdk()?,
            evidence.original_dimensions,
            f64::from_bits(evidence.dpr_bits.value()),
            original_png.clone(),
            sha2::Sha256::digest(&original_png).into(),
        )
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let derived =
            crate::device::screenshot::Screenshot::reader_images_from_owned_png(&original_png)?;
        ensure!(
            derived == images,
            "resident provider bytes differ from frozen recipe"
        );
        Ok(PreparedResidentCapture {
            evidence,
            original_png,
            images,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::storage::StorePaths;
    use std::{
        fs,
        io::Cursor,
        path::{Path, PathBuf},
    };

    fn id() -> SdkUuid {
        SdkUuid::parse(&Uuid::new_v4().to_string()).unwrap()
    }
    fn capture() -> ResidentSourceCapture {
        capture_for(Uuid::new_v4())
    }

    #[cfg(all(
        target_os = "linux",
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    pub(crate) fn fixture_original(capture: &ResidentSourceCapture) -> Vec<u8> {
        capture.capture.png().to_vec()
    }
    pub(crate) fn capture_for(document: Uuid) -> ResidentSourceCapture {
        capture_with_pixels(
            document,
            image::RgbaImage::from_pixel(12, 20, image::Rgba([17, 29, 41, 255])),
        )
    }

    pub(crate) fn capture_with_pixels(
        document: Uuid,
        pixels: image::RgbaImage,
    ) -> ResidentSourceCapture {
        let page = PageKey {
            document: SdkUuid::parse(&document.to_string()).unwrap(),
            page: id(),
        };
        let facts = ReaderFacts::new(page, vec![page.page]).unwrap();
        let dimensions = [pixels.width(), pixels.height()];
        let mut output = Cursor::new(Vec::new());
        pixels
            .write_to(&mut output, image::ImageFormat::Png)
            .unwrap();
        let png = output.into_inner();
        let hash = sha2::Sha256::digest(&png).into();
        ResidentSourceCapture::from_session(
            &RuntimeIdentity {
                profile_sha256: [1; 32],
                executable_sha256: [2; 32],
                adapter_sha256: [3; 32],
                boot: id(),
                activation: id(),
                pid: 123,
                start_ticks: 456,
            },
            OperationId(id()),
            OperationId(id()),
            facts.clone(),
            HistoricalCapture::new(facts.clone(), facts, dimensions, 1.0, png, hash).unwrap(),
        )
        .unwrap()
    }
    fn paths(base: &Path) -> StorePaths {
        StorePaths {
            data: base.join("data"),
            cache: base.join("cache"),
            credentials: base.join("credentials"),
        }
    }
    fn turn(conversation: Uuid) -> Turn {
        Turn {
            id: Uuid::new_v4(),
            conversation,
            exchange: Uuid::new_v4(),
            sequence: 0,
            role: Role::User,
            mode: Mode::Reader,
            outcome: Outcome::Prepared,
            text: None,
            sources: vec![],
            correction_of: None,
            created_ms: 1,
            updated_ms: 1,
            completion: None,
        }
    }
    fn fixture() -> (PathBuf, Arc<Store>, Ledger, Turn, ResidentSourceCapture) {
        let base = std::env::temp_dir().join(format!("resident-history-{}", Uuid::new_v4()));
        let store = Arc::new(Store::open(paths(&base)).unwrap());
        let ledger = Ledger::new(store.clone());
        let conversation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
        (base, store, ledger, turn(conversation), capture())
    }

    #[test]
    fn resident_history_cas_readback_restart_export_and_distinct_source_schema() {
        let (base, store, ledger, turn, captured) = fixture();
        let operation = Uuid::new_v4();
        let evidence = Uuid::new_v4();
        let expected = ledger
            .expected(Namespace::Conversation, turn.conversation)
            .unwrap();
        let prepared = ledger
            .prepare_resident_historical(
                operation,
                expected.clone(),
                turn.clone(),
                evidence,
                &captured,
            )
            .unwrap();
        assert_eq!(prepared.original_png, captured.capture.png());
        assert_eq!(prepared.images.len(), 4);
        assert_eq!(prepared.evidence.correspondence, captured.correspondence);
        let same = ledger
            .prepare_resident_historical(
                operation,
                expected.clone(),
                turn.clone(),
                evidence,
                &captured,
            )
            .unwrap();
        assert_eq!(same.images, prepared.images);
        assert!(ledger
            .prepare_resident_historical(
                Uuid::new_v4(),
                expected,
                turn.clone(),
                Uuid::new_v4(),
                &captured
            )
            .is_err());
        let envelope = store.value(Namespace::Source, evidence).unwrap().unwrap();
        assert_eq!(envelope.domain_schema_version, 2);
        for (namespace, schema) in [(Namespace::Source, 1), (Namespace::Conversation, 2)] {
            let mut wrong = envelope.clone();
            wrong.namespace = namespace;
            wrong.domain_schema_version = schema;
            assert!(Ledger::decode(&wrong).is_err());
        }
        let mut bad = envelope.clone();
        bad.payload["record"]["schema"] = serde_json::json!(7);
        assert!(Ledger::decode(&bad).is_err());
        let archive = base.join("archive");
        assert!(store.export(&archive).unwrap());
        drop(ledger);
        drop(store);
        let store = Arc::new(Store::open(paths(&base)).unwrap());
        let reopened = Ledger::new(store.clone())
            .stored_resident_images(evidence)
            .unwrap();
        assert_eq!(reopened.images, prepared.images);
        assert_eq!(reopened.original_png, prepared.original_png);
        let restored_base = base.join("restored");
        let restored = Arc::new(Store::open(paths(&restored_base)).unwrap());
        restored.restore(&archive).unwrap();
        let reread = Ledger::new(restored.clone())
            .stored_resident_images(evidence)
            .unwrap();
        assert_eq!(reread.images, prepared.images);
        assert_eq!(reread.evidence, prepared.evidence);
        drop(restored);
        drop(store);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn resident_history_refuses_false_derivatives_missing_parent_and_changed_correspondence() {
        let (base, store, ledger, turn, captured) = fixture();
        let prepared = ledger
            .prepare_resident_historical(
                Uuid::new_v4(),
                ledger
                    .expected(Namespace::Conversation, turn.conversation)
                    .unwrap(),
                turn,
                Uuid::new_v4(),
                &captured,
            )
            .unwrap();
        let media = prepared
            .evidence
            .media()
            .unwrap()
            .into_iter()
            .map(|m| {
                let bytes = store
                    .read_object(&ObjectRef {
                        sha256: m.sha256.clone(),
                        bytes: m.bytes,
                    })
                    .unwrap();
                (m.sha256, bytes)
            })
            .collect::<BTreeMap<_, _>>();
        assert!(
            Ledger::resident_images_from_evidence(prepared.evidence.clone(), |m| {
                ensure!(
                    m.sha256 != prepared.evidence.original_png.sha256,
                    "omitted parent"
                );
                Ok(media[&m.sha256].clone())
            })
            .is_err()
        );
        for fault in ["source", "order", "runtime", "dpr", "ordinal", "schema"] {
            let mut bad = prepared.evidence.clone();
            match fault {
                "source" => bad.after.page = Uuid::new_v4(),
                "order" => bad.before.order.push(bad.before.page),
                "runtime" => bad.correspondence.pid = 0,
                "dpr" => bad.dpr_bits = ExactU64::try_from(f64::NAN.to_bits().to_string()).unwrap(),
                "ordinal" => bad.images[0].provider_ordinal = 3,
                "schema" => bad.schema = 2,
                _ => unreachable!(),
            }
            assert!(
                Ledger::resident_images_from_evidence(bad, |m| Ok(media[&m.sha256].clone()))
                    .is_err(),
                "{fault}"
            );
        }
        let mut forged = prepared.evidence.clone();
        // Internally valid hashes/dimensions cannot replace recipe-derived bytes.
        forged.images[0].media = prepared.evidence.images[1].media.clone();
        forged.images[0].dimensions = prepared.evidence.images[1].dimensions;
        assert!(
            Ledger::resident_images_from_evidence(forged, |m| Ok(media[&m.sha256].clone()))
                .is_err()
        );
        let records = ledger
            .inspect(prepared.evidence.conversation, false)
            .unwrap();
        assert!(records
            .iter()
            .any(|r| matches!(r, Record::ResidentCapture(_))));
        assert!(!records.iter().any(|r| matches!(
            r,
            Record::Binding(_)
                | Record::Capture(_)
                | Record::DevelopmentCapture(_)
                | Record::LegacyCapture(_)
        )));
        drop(ledger);
        drop(store);
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn selected_resident_prepare_publishes_once_and_recovers_only_original() {
        use crate::conversation::selected_reader_prepare::ResidentPrepared;
        use crate::storage::selection::{SelectedReadBudget, SelectionChange, SelectionScope};
        use crate::storage::{Manifest, Scope};

        let (base, store, ledger, _, captured) = fixture();
        let ordinary = store.manifests().unwrap();
        let seed = &ordinary[0];
        let objects = seed
            .records
            .iter()
            .map(|reference| {
                (
                    reference.sha256.clone(),
                    store.read_object(reference).unwrap(),
                )
            })
            .collect();
        let admission = SelectedAdmission::new(
            store.clone(),
            SelectionScope {
                group: Uuid::new_v4(),
                key_sha256: crate::memory::catalog::document_key(captured.document()).unwrap(),
                binding_sha256: digest(b"selected resident fixture"),
            },
        )
        .unwrap();
        let predecessor = admission
            .initialize(
                SelectionChange {
                    operation: Uuid::new_v4(),
                    accepted_base_sha256: digest(b"selected resident base"),
                    selected: Manifest {
                        transaction_id: Uuid::new_v4(),
                        scope: Scope::SelectedRecords,
                        ..seed.clone()
                    },
                    retained: vec![],
                },
                objects,
            )
            .unwrap()
            .token;
        let operation = Uuid::new_v4();
        let prepare = || ResidentPrepared::new(&store, &predecessor, &captured, operation, 10);
        let mut reads = SelectedReadBudget::new(64, 4096, 64 * 1024 * 1024, 128 * 1024 * 1024);
        let (token, ack) = SelectedAdmission::with_gates(&[&admission], |store| {
            prepare()?.publish_locked(store, &predecessor, &mut reads)
        })
        .unwrap();
        let SelectedCaptureImages::Resident(images) = admission
            .capture_images(&token, ack.capture.record_id)
            .unwrap()
        else {
            panic!("selected Prepared lost resident image custody");
        };
        assert_eq!(images.original_png, captured.capture.png());
        assert!(prepare()
            .unwrap()
            .original(&store, &predecessor, &mut reads)
            .unwrap());
        let replay = SelectedAdmission::with_gates(&[&admission], |store| {
            prepare()?.publish_locked(store, &predecessor, &mut reads)
        });
        assert!(replay
            .err()
            .unwrap()
            .to_string()
            .contains("already historical"));
        assert_eq!(
            store
                .selected_snapshot(admission.scope(), crate::storage::MAX_ITEMS)
                .unwrap()
                .unwrap()
                .token,
            token
        );
        let ordinary_ids = |manifests: &[Manifest]| {
            manifests
                .iter()
                .filter(|manifest| manifest.scope != Scope::SelectedRecords)
                .map(|manifest| manifest.transaction_id)
                .collect::<BTreeSet<_>>()
        };
        assert_eq!(
            ordinary_ids(&store.manifests().unwrap()),
            ordinary_ids(&ordinary)
        );
        #[cfg(target_os = "linux")]
        {
            let interpretation = crate::conversation::SelectedReaderWrite {
                operation: Uuid::new_v4(),
                root: ack.root,
                user: ack.user,
                updated_ms: 11,
                text: "What is x?".into(),
                kind: crate::conversation::SelectedReaderText::Interpret,
            };
            let publish = |accepted: &crate::storage::selection::SelectionToken,
                           request,
                           reads: &mut SelectedReadBudget| {
                admission.publish_resident_reader_text_joined(
                    accepted,
                    request,
                    &[],
                    reads,
                    |_, _| Ok(()),
                )
            };
            let mut exhausted = SelectedReadBudget::new(0, 0, 0, 0);
            assert!(admission
                .recover_reader_text(&token, &interpretation, &mut exhausted)
                .is_err());
            assert!(publish(&token, interpretation.clone(), &mut exhausted).is_err());
            assert_eq!(
                store
                    .selected_snapshot(admission.scope(), crate::storage::MAX_ITEMS)
                    .unwrap()
                    .unwrap()
                    .token,
                token
            );
            let crate::conversation::SelectedReaderPublication::Published {
                publication,
                acknowledgment,
            } = publish(&token, interpretation.clone(), &mut reads).unwrap()
            else {
                panic!("fresh Reader interpretation was not published");
            };
            let generation = crate::conversation::SelectedReaderWrite {
                operation: Uuid::new_v4(),
                root: acknowledgment.root,
                user: acknowledgment.turn,
                updated_ms: 12,
                text: "x = 7".into(),
                kind: crate::conversation::SelectedReaderText::Generate {
                    assistant: Uuid::new_v4(),
                },
            };
            let crate::conversation::SelectedReaderPublication::Published {
                acknowledgment, ..
            } = publish(&publication.token, generation, &mut reads).unwrap()
            else {
                panic!("fresh Reader draft was not published");
            };
            assert_eq!(acknowledgment.written.outcome, Outcome::Generated);
            assert!(admission
                .recover_reader_text(&token, &interpretation, &mut exhausted)
                .is_err());
            assert!(admission
                .recover_reader_text(&token, &interpretation, &mut reads)
                .unwrap());
            assert_eq!(
                ordinary_ids(&store.manifests().unwrap()),
                ordinary_ids(&ordinary)
            );
        }
        drop((admission, ledger, store));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn resident_selected_snapshot_readback_and_omitted_media_remain_historical() {
        use crate::storage::selection::{SelectionChange, SelectionScope};
        use crate::storage::{Coverage, Manifest, Omission, Scope};
        for omitted in [false, true] {
            let (base, store, ledger, turn, captured) = fixture();
            let evidence = Uuid::new_v4();
            let prepared = ledger
                .prepare_resident_historical(
                    Uuid::new_v4(),
                    ledger
                        .expected(Namespace::Conversation, turn.conversation)
                        .unwrap(),
                    turn,
                    evidence,
                    &captured,
                )
                .unwrap();
            let mut objects = BTreeMap::new();
            let mut records = BTreeMap::new();
            let mut media = BTreeMap::new();
            let mut namespaces = BTreeMap::new();
            for manifest in store.manifests().unwrap() {
                for reference in manifest.records {
                    namespaces.insert(
                        reference.sha256.clone(),
                        manifest.record_namespaces[&reference.sha256],
                    );
                    objects.insert(
                        reference.sha256.clone(),
                        store.read_object(&reference).unwrap(),
                    );
                    records.insert(reference.sha256.clone(), reference);
                }
                for reference in manifest.media {
                    if !omitted {
                        objects.insert(
                            reference.sha256.clone(),
                            store.read_object(&reference).unwrap(),
                        );
                    }
                    media.insert(reference.sha256.clone(), reference);
                }
            }
            let selected = Manifest {
                format: FORMAT,
                transaction_id: Uuid::new_v4(),
                scope: Scope::SelectedRecords,
                records: records.into_values().collect(),
                record_namespaces: namespaces,
                media_coverage: media
                    .keys()
                    .map(|hash| {
                        if omitted {
                            Coverage::Omitted {
                                sha256: hash.clone(),
                                reason: Omission::PolicyDisabled,
                            }
                        } else {
                            Coverage::Included {
                                sha256: hash.clone(),
                            }
                        }
                    })
                    .collect(),
                media: if omitted {
                    vec![]
                } else {
                    media.into_values().collect()
                },
            };
            let admission = SelectedAdmission::new(
                store.clone(),
                SelectionScope {
                    group: Uuid::new_v4(),
                    key_sha256: digest(b"resident source selected"),
                    binding_sha256: digest(b"historical correspondence only"),
                },
            )
            .unwrap();
            let publication = admission
                .initialize(
                    SelectionChange {
                        operation: Uuid::new_v4(),
                        accepted_base_sha256: digest(b"resident fixture base"),
                        selected,
                        retained: vec![],
                    },
                    objects,
                )
                .unwrap();
            let result = admission.capture_images(&publication.token, evidence);
            if omitted {
                assert!(result.err().unwrap().to_string().contains("omitted"));
            } else {
                let SelectedCaptureImages::Resident(actual) = result.unwrap() else {
                    panic!("origin lost")
                };
                assert_eq!(actual.evidence, prepared.evidence);
                assert_eq!(actual.original_png, prepared.original_png);
                assert_eq!(actual.images, prepared.images);
            }
            let archive = base.join("selected-backup");
            store.export_selected(&archive).unwrap();
            // The portable backup retains records/descriptors even when image
            // coverage was explicitly omitted; no live/native authority exists.
            assert!(archive.exists());
            drop(admission);
            drop(ledger);
            drop(store);
            fs::remove_dir_all(base).unwrap();
        }
    }
}

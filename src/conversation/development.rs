//! Separate development history. Saved correspondence cannot restore live authority.
use super::*;
use remarkable_open_sdk::development_capture::{
    ExpectedCaptureBinding, ReadOnlyDevelopmentCapture,
};
use serde::{Deserialize, Serialize};

// Exact SDK revision owning this opt-in development parser.
pub const DEVELOPMENT_SDK_SOURCE: &str = "d01b4bdfbc4dc11673f84ff82bbc62c8dc77bbb1";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HistoricalDevelopmentBinding {
    pub nonce: String,
    pub attempt_pid: String,
    pub attempt_start: String,
    pub root_device: String,
    pub root_inode: String,
    pub document_id: String,
    /// Externally expected fixture order, never an observed native order.
    pub expected_order: Vec<String>,
}
impl From<&ExpectedCaptureBinding> for HistoricalDevelopmentBinding {
    fn from(binding: &ExpectedCaptureBinding) -> Self {
        Self {
            nonce: binding.nonce.clone(),
            attempt_pid: binding.attempt_pid.clone(),
            attempt_start: binding.attempt_start.clone(),
            root_device: binding.root_device.clone(),
            root_inode: binding.root_inode.clone(),
            document_id: binding.document_id.clone(),
            expected_order: binding.expected_order.clone(),
        }
    }
}
impl HistoricalDevelopmentBinding {
    pub(crate) fn expected(&self) -> ExpectedCaptureBinding {
        ExpectedCaptureBinding {
            nonce: self.nonce.clone(),
            attempt_pid: self.attempt_pid.clone(),
            attempt_start: self.attempt_start.clone(),
            root_device: self.root_device.clone(),
            root_inode: self.root_inode.clone(),
            document_id: self.document_id.clone(),
            expected_order: self.expected_order.clone(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum DevelopmentOrigin {
    DevelopmentUnqualified,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentImage {
    pub media: Media,
    pub dimensions: [u32; 2],
    pub provider_ordinal: u32,
    /// Locally derived crop in original PNG pixels, never SDK page geometry.
    pub parent_crop: [u32; 4],
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentCaptureHistory {
    pub schema: u32,
    pub sdk_source: String,
    pub id: Uuid,
    pub conversation: Uuid,
    pub turn: Uuid,
    pub origin: DevelopmentOrigin,
    pub expected_binding: HistoricalDevelopmentBinding,
    pub completion: Media,
    pub original_png: Media,
    pub original_dimensions: [u32; 2],
    pub provider_procedure: String,
    /// Locally derived overview and three strips, in original provider order.
    pub images: Vec<DevelopmentImage>,
}
pub struct PreparedDevelopmentCapture {
    pub evidence: DevelopmentCaptureHistory,
    pub completion: Vec<u8>,
    pub original_png: Vec<u8>,
    pub images: Vec<Vec<u8>>,
}
impl DevelopmentCaptureHistory {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == 1
                && self.sdk_source == DEVELOPMENT_SDK_SOURCE
                && !self.id.is_nil()
                && !self.conversation.is_nil()
                && !self.turn.is_nil(),
            "invalid development history identity/schema/source"
        );
        ensure!(
            self.images.len() == 4
                && self.provider_procedure == "reader-nearest-overview-overlapping-strips-v1",
            "development provider order mismatch"
        );
        self.completion.validate()?;
        ensure!(
            self.completion.bytes > 0
                && self.completion.bytes <= 8192
                && self.completion.media_type == "application/json",
            "invalid development receipt descriptor"
        );
        self.original_png.validate()?;
        ensure!(
            self.original_png.bytes > 0
                && self.original_png.bytes <= 8 * 1024 * 1024
                && self.original_png.media_type == "image/png"
                && self
                    .original_dimensions
                    .iter()
                    .all(|n| *n > 0 && *n <= 8192)
                && u64::from(self.original_dimensions[0]) * u64::from(self.original_dimensions[1])
                    <= 4_194_304,
            "invalid development original PNG descriptor"
        );
        let [w, h] = self.original_dimensions;
        let th = h * 2 / 5;
        let crops = [
            [0, 0, w, h],
            [0, 0, w, th],
            [0, (h - th) / 2, w, th],
            [0, h - th, w, th],
        ];
        let mut total = self.original_png.bytes + self.completion.bytes;
        for (ordinal, image) in self.images.iter().enumerate() {
            image.media.validate()?;
            ensure!(
                image.provider_ordinal == ordinal as u32
                    && image.parent_crop == crops[ordinal]
                    && image.dimensions == if ordinal == 0 { [768, 1024] } else { [w, th] }
                    && image.media.bytes > 0
                    && image.media.bytes <= 32 * 1024 * 1024
                    && image.media.media_type == "image/png",
                "invalid development provider descriptor"
            );
            total = total
                .checked_add(image.media.bytes)
                .context("development batch overflow")?;
        }
        ensure!(total <= 64 * 1024 * 1024, "development encoded batch bound");
        ensure!(
            !self.expected_binding.expected_order.is_empty()
                && self.expected_binding.expected_order.len() <= 256
                && self
                    .expected_binding
                    .expected_order
                    .iter()
                    .all(|id| id.len() <= 36),
            "development expected order bound"
        );
        self.media()?;
        Ok(())
    }
    pub(super) fn media(&self) -> Result<Vec<Media>> {
        let mut media = BTreeMap::new();
        for item in [&self.completion, &self.original_png]
            .into_iter()
            .chain(self.images.iter().map(|i| &i.media))
        {
            if let Some(prior) = media.insert(item.sha256.clone(), item.clone()) {
                ensure!(prior == *item, "inconsistent development media descriptors");
            }
        }
        Ok(media.into_values().collect())
    }
}
impl Ledger {
    /// Persist original development receipt and PNG atomically with the Prepared turn.
    /// No source observation, native binding, selection token or effect context is made.
    pub fn prepare_development(
        &self,
        operation: Uuid,
        expected_root: ExpectedHeads,
        mut turn: Turn,
        evidence_id: Uuid,
        capture: &ReadOnlyDevelopmentCapture,
    ) -> Result<PreparedDevelopmentCapture> {
        ensure!(
            turn.role == Role::User
                && turn.outcome == Outcome::Prepared
                && turn.sequence == 0
                && turn.sources.is_empty(),
            "invalid development request"
        );
        Self::validate_turn(&turn)?;
        let provider_images = crate::device::screenshot::Screenshot::reader_images_from_owned_png(
            capture.png_bytes(),
        )?;
        let [w, h] = capture.dimensions();
        let th = h * 2 / 5;
        let crops = [
            [0, 0, w, h],
            [0, 0, w, th],
            [0, (h - th) / 2, w, th],
            [0, h - th, w, th],
        ];
        let evidence = DevelopmentCaptureHistory {
            schema: 1,
            sdk_source: DEVELOPMENT_SDK_SOURCE.into(),
            id: evidence_id,
            conversation: turn.conversation,
            turn: turn.id,
            origin: DevelopmentOrigin::DevelopmentUnqualified,
            expected_binding: capture.expected_binding().into(),
            completion: Media {
                sha256: digest(capture.completion_bytes()),
                bytes: capture.completion_bytes().len() as u64,
                media_type: "application/json".into(),
            },
            original_png: Media {
                sha256: digest(capture.png_bytes()),
                bytes: capture.png_bytes().len() as u64,
                media_type: "image/png".into(),
            },
            original_dimensions: capture.dimensions(),
            provider_procedure: "reader-nearest-overview-overlapping-strips-v1".into(),
            images: provider_images
                .iter()
                .enumerate()
                .map(|(ordinal, bytes)| DevelopmentImage {
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
        turn.sources = vec![evidence.id];
        let fingerprint = Self::fingerprint(&("prepare-development", &turn, &evidence))?;
        if self.replay(operation, &fingerprint)?.is_some() {
            return self.stored_development_images(evidence_id);
        }
        let (_, mut root) = self.live_root(turn.conversation, Some(&expected_root))?;
        ensure!(
            root.binding.is_none(),
            "development history must remain unbound"
        );
        self.validate_correction_target(&turn)?;
        Self::validate_image_bytes(&evidence.original_png, capture.png_bytes(), w, h)?;
        for (image, bytes) in evidence.images.iter().zip(&provider_images) {
            Self::validate_image_bytes(
                &image.media,
                bytes,
                image.dimensions[0],
                image.dimensions[1],
            )?;
        }
        self.stage_image(&evidence.completion, capture.completion_bytes())?;
        self.stage_image(&evidence.original_png, capture.png_bytes())?;
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
                Record::DevelopmentCapture(Box::new(evidence.clone())),
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
        self.stored_development_images(evidence_id)
    }
    /// Historical byte retrieval only. Parser consistency cannot prove current ownership.
    pub fn stored_development_images(&self, id: Uuid) -> Result<PreparedDevelopmentCapture> {
        self.complete_store()?;
        let envelope = self
            .store
            .value(Namespace::Source, id)?
            .context("development evidence absent")?;
        let Record::DevelopmentCapture(evidence) = Self::decode(&envelope)? else {
            bail!("not development evidence")
        };
        self.live_root(evidence.conversation, None)?;
        let turn = self
            .store
            .value(Namespace::Conversation, evidence.turn)?
            .context("development turn absent")?;
        let Record::Turn(turn) = Self::decode(&turn)? else {
            bail!("invalid development turn")
        };
        ensure!(
            turn.conversation == evidence.conversation && turn.sources.contains(&id),
            "development evidence link mismatch"
        );
        Self::development_images_from_evidence(*evidence, |media| {
            self.store.read_object(&ObjectRef {
                sha256: media.sha256.clone(),
                bytes: media.bytes,
            })
        })
    }
    pub(super) fn development_images_from_evidence(
        evidence: DevelopmentCaptureHistory,
        mut load: impl FnMut(&Media) -> Result<Vec<u8>>,
    ) -> Result<PreparedDevelopmentCapture> {
        evidence.validate()?;
        let completion = load(&evidence.completion)?;
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
        // Revalidate the raw saved correspondence, never reconstruct native authority.
        let verified = ReadOnlyDevelopmentCapture::from_collected_v11(
            &evidence.expected_binding.expected(),
            completion.clone(),
            original_png.clone(),
        )?;
        ensure!(
            verified.dimensions() == evidence.original_dimensions,
            "development dimensions mismatch"
        );
        Ok(PreparedDevelopmentCapture {
            evidence,
            completion,
            original_png,
            images,
        })
    }
}

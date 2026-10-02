//! Explicitly unbound, exact-byte consumer history; no native source facts.
use super::*;

impl LegacyCapture {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == 1
                && !self.id.is_nil()
                && !self.turn.is_nil()
                && !self.conversation.is_nil(),
            "invalid legacy capture identity/schema"
        );
        ensure!(
            (1..=15).contains(&self.images.len()),
            "legacy provider-image bound"
        );
        let mut total = 0u64;
        for (i, image) in self.images.iter().enumerate() {
            ensure!(
                image.provider_ordinal == Some(i as u32)
                    && image.role
                        == if i == 0 {
                            LegacyImageRole::Overview
                        } else {
                            LegacyImageRole::Detail
                        },
                "legacy provider order/role mismatch"
            );
        }
        if let Some(parent) = &self.parent {
            ensure!(
                parent.role == LegacyImageRole::AcquisitionParent
                    && parent.provider_ordinal.is_none(),
                "invalid legacy acquisition parent"
            );
        }
        for image in self.parent.iter().chain(&self.images) {
            ensure!(
                crate::storage::valid_digest(&image.media.sha256)
                    && image.media.bytes > 0
                    && image.media.bytes <= 32 * 1024 * 1024
                    && matches!(image.media.media_type.as_str(), "image/png" | "image/jpeg")
                    && image.dimensions.iter().all(|n| *n > 0 && *n <= 8192),
                "invalid legacy image descriptor"
            );
            total = total
                .checked_add(image.media.bytes)
                .context("legacy byte overflow")?;
        }
        ensure!(total <= 64 * 1024 * 1024, "legacy encoded batch bound");
        self.media()?;
        Ok(())
    }
    pub(super) fn media(&self) -> Result<Vec<Media>> {
        let mut media = BTreeMap::new();
        for image in self.parent.iter().chain(&self.images) {
            if let Some(prior) = media.insert(image.media.sha256.clone(), image.media.clone()) {
                ensure!(
                    prior == image.media,
                    "inconsistent legacy media descriptors"
                );
            }
        }
        Ok(media.into_values().collect())
    }
}

impl Ledger {
    /// Explicit legacy selection only; this function never attempts SDK acquisition.
    pub fn prepare_legacy(
        &self,
        operation: Uuid,
        expected_root: ExpectedHeads,
        mut turn: Turn,
        evidence_id: Uuid,
        parent: Option<LegacyImageInput>,
        images: Vec<LegacyImageInput>,
    ) -> Result<PreparedLegacyCapture> {
        ensure!(
            turn.role == Role::User
                && turn.outcome == Outcome::Prepared
                && turn.sequence == 0
                && turn.sources.is_empty(),
            "invalid legacy request"
        );
        Self::validate_turn(&turn)?;
        let evidence = LegacyCapture {
            schema: 1,
            id: evidence_id,
            conversation: turn.conversation,
            turn: turn.id,
            origin: LegacyOrigin::LegacyUnqualified,
            identity: (),
            qualification: (),
            parent: parent.as_ref().map(|p| p.descriptor.clone()),
            images: images.iter().map(|i| i.descriptor.clone()).collect(),
        };
        evidence.validate()?;
        turn.sources = vec![evidence.id];
        let fingerprint = Self::fingerprint(&("prepare-legacy", &turn, &evidence))?;
        if self.replay(operation, &fingerprint)?.is_some() {
            return self.stored_legacy_images(evidence_id);
        }
        let (_, mut root) = self.live_root(turn.conversation, Some(&expected_root))?;
        ensure!(root.binding.is_none(), "legacy history must remain unbound");
        self.validate_correction_target(&turn)?;
        for image in parent.iter().chain(&images) {
            let [width, height] = image.descriptor.dimensions;
            Self::validate_image_bytes(&image.descriptor.media, &image.bytes, width, height)?;
        }
        for image in parent.iter().chain(&images) {
            self.stage_image(&image.descriptor.media, image.bytes.as_slice())?;
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
                Record::LegacyCapture(evidence.clone()),
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
        self.stored_legacy_images(evidence_id)
    }

    /// Historical bytes only, with no reconstructed device capability.
    pub fn stored_legacy_images(&self, id: Uuid) -> Result<PreparedLegacyCapture> {
        self.complete_store()?;
        let envelope = self
            .store
            .value(Namespace::Source, id)?
            .context("legacy evidence absent")?;
        let Record::LegacyCapture(evidence) = Self::decode(&envelope)? else {
            bail!("not legacy evidence");
        };
        self.live_root(evidence.conversation, None)?;
        let turn = self
            .store
            .value(Namespace::Conversation, evidence.turn)?
            .context("legacy turn absent")?;
        let Record::Turn(turn) = Self::decode(&turn)? else {
            bail!("invalid legacy turn")
        };
        ensure!(
            turn.conversation == evidence.conversation && turn.sources.contains(&id),
            "legacy evidence link mismatch"
        );
        let load = |image: &LegacyImage| -> Result<Vec<u8>> {
            let bytes = self.store.read_object(&ObjectRef {
                sha256: image.media.sha256.clone(),
                bytes: image.media.bytes,
            })?;
            let [width, height] = image.dimensions;
            Self::validate_image_bytes(&image.media, &bytes, width, height)?;
            Ok(bytes)
        };
        let parent = evidence.parent.as_ref().map(load).transpose()?;
        let images = evidence.images.iter().map(load).collect::<Result<_>>()?;
        Ok(PreparedLegacyCapture {
            evidence,
            parent,
            images,
        })
    }
}

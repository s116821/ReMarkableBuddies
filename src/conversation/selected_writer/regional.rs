//! Closed selected regional source. No caller-supplied mask can construct this proof.
use super::*;
use target::{RecordRef, SourceTarget, TargetState, Understanding};

pub(crate) struct RegionalImage {
    pub(crate) target: Box<SourceTarget>,
    pub(crate) reference: RecordRef,
    state: IntentRecordRef,
    pub(crate) document: Uuid,
    pub(crate) png: Vec<u8>,
}
impl RegionalImage {
    pub(crate) fn admit(&self, region: target::RegionSource) -> Result<SelectedWriterSource> {
        region.validate()?;
        ensure!(
            region.target == self.reference
                && region.original == self.target.source.original
                && region.derived.sha256 == digest(&self.png)
                && region.derived.bytes == self.png.len() as u64
                && region.dimensions == self.target.source.dimensions
                && region.about == self.target.candidate.about()?,
            "regional admitted source differs"
        );
        Ok(SelectedWriterSource::GenerateRegion {
            regional: SelectedRegionalSource {
                region: Box::new(region),
            },
        })
    }
    pub(crate) fn matches(&self, other: &Self) -> Result<()> {
        ensure!(
            self.reference == other.reference
                && self.state == other.state
                && self.document == other.document
                && self.png == other.png,
            "regional Writer target/source changed"
        );
        Ok(())
    }
}
impl SelectedAdmission {
    pub(crate) fn selected_writer_region_locked(
        &self,
        store: &Store,
        accepted: &SelectionToken,
        root_ref: &IntentRecordRef,
        reference: &RecordRef,
        reads: &mut SelectedReadBudget,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<RegionalImage> {
        ensure!(
            std::ptr::eq(store, self.writer_store().as_ref())
                && store.source_target_generation()?.is_some()
                && !store.ordinary_reader_family()?,
            "regional Writer requires selected target family"
        );
        let mut snapshot = self.writer_snapshot(store, Some(reads))?;
        ensure!(snapshot.token == *accepted, "regional selection changed");
        let p = SelectedDomainProjection::from_snapshot(&snapshot)?;
        let root = root(&p, root_ref)?;
        let pin = |r: &RecordRef| IntentRecordRef {
            namespace: r.namespace,
            record_id: r.record,
            revision_id: r.revision,
            object: r.object.clone(),
        };
        let Record::SourceTarget(target) = current(&p, &pin(reference))? else {
            bail!("regional target variant")
        };
        ensure!(
            target.conversation == root.id
                && matches!(target.candidate.understanding, Understanding::Interpreted),
            "regional target unavailable"
        );
        let state = p.current_reference(Namespace::Conversation, TargetState::id(root.id))?;
        let Record::TargetState(value) = current(&p, &state)? else {
            bail!("regional state variant")
        };
        let Record::TargetUse(used) = current(&p, &pin(&value.use_reference))? else {
            bail!("regional use variant")
        };
        ensure!(
            used.target.as_ref() == Some(reference),
            "regional target is no longer current"
        );
        let source = current(&p, &pin(&target.source.record))?.clone();
        let (descriptor, conversation) =
            target_source::descriptor(target.source.record.clone(), &source)?;
        ensure!(
            descriptor == target.source && conversation == root.id,
            "regional source correspondence"
        );
        let Some((_, Some(Record::Turn(user)))) = p.head(Namespace::Conversation, descriptor.turn)
        else {
            bail!("regional source user absent")
        };
        ensure!(
            user.role == Role::User
                && user.conversation == root.id
                && user.sources.contains(&descriptor.record.record),
            "regional source user differs"
        );
        let document = match p.document_ownership()?.get(&root.id) {
            Some(DocumentOwnership::Document(document)) => *document,
            _ => bail!("regional document unbound"),
        };
        let source_document = descriptor
            .validate_record(&source, root.id)?
            .unwrap_or_else(|| {
                Uuid::new_v5(
                    &root.id,
                    format!("historical-unbound:{}", descriptor.record.record).as_bytes(),
                )
            });
        ensure!(
            document == source_document,
            "regional historical document differs"
        );
        let target = target.clone();
        drop(p);
        let mut load = |media: &crate::storage::Media| {
            progress()?;
            super::super::notebook::selected::read_selected_media(
                &mut snapshot,
                &crate::storage::ObjectRef {
                    sha256: media.sha256.clone(),
                    bytes: media.bytes,
                },
                reads,
            )
        };
        let original = match source {
            Record::ResidentCapture(c) => {
                Ledger::resident_images_from_evidence(*c, &mut load)?.original_png
            }
            Record::Capture(c) => {
                Ledger::sdk_images_from_evidence(*c, &mut load)?
                    .native_parent
                    .bytes
            }
            Record::DevelopmentCapture(c) => {
                Ledger::development_images_from_evidence(*c, &mut load)?.original_png
            }
            Record::LegacyCapture(c) => {
                let v = Ledger::legacy_images_from_evidence(c, &mut load)?;
                v.parent.unwrap_or_else(|| v.images[0].clone())
            }
            Record::Source(_) => load(&descriptor.original)?,
            _ => bail!("regional source variant"),
        };
        Ledger::validate_image_bytes(
            &descriptor.original,
            &original,
            descriptor.dimensions[0],
            descriptor.dimensions[1],
        )?;
        let png = target
            .candidate
            .geometry
            .as_ref()
            .context("regional geometry absent")?
            .mask_png(&original, descriptor.dimensions, progress)?;
        Ok(RegionalImage {
            target,
            reference: reference.clone(),
            state,
            document,
            png,
        })
    }

    pub(crate) fn prepare_regional_writer_locked(
        &self,
        store: &Store,
        accepted: &SelectionToken,
        req: &SelectedWriterPrepare,
        artifact: (&[u8], u64),
        reads: &mut SelectedReadBudget,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<SelectedWriterPreparation> {
        let (masked, ms) = artifact;
        let SelectedWriterSource::GenerateRegion { regional } = &req.source else {
            bail!("regional preparation variant")
        };
        let regional = &regional.region;
        ensure!(
            regional.derived.sha256 == digest(masked)
                && regional.derived.bytes == masked.len() as u64,
            "regional mask differs"
        );
        ensure!(
            store
                .selected_receipt_with_budget(self.scope(), req.operation, reads)?
                .is_none(),
            "regional preparation already historical"
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
            WriterPublication {
                reads: Some(reads),
                objects: BTreeMap::from([(regional.derived.sha256.clone(), masked.to_vec())]),
                check: Some(progress),
            },
        )
    }
}

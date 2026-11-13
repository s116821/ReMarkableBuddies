//! Stored-target input shares the capture runner without impersonating a capture.
use super::*;
use crate::conversation::{
    target::{self, RecordRef, RegionSource},
    RegionalImage,
};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriterTargetRequest {
    pub operation: Uuid,
    pub request: String,
    pub target: RecordRef,
    pub guidance: Option<String>,
    pub model_profile: String,
}
impl WriterTargetRequest {
    pub(crate) fn validate(&self, root: &IntentRecordRef) -> Result<()> {
        Input::Regional(self).validate(root)
    }
    pub(crate) fn preparation(&self) -> Uuid {
        Input::Regional(self).preparation()
    }
    pub(crate) fn preparation_fingerprint(
        &self,
        root: &IntentRecordRef,
        selection: &IntentSelectionEvidence,
    ) -> Result<String> {
        Input::Regional(self).preparation_fingerprint(root, selection)
    }
}
#[derive(Clone, Copy)]
pub(super) enum Input<'a> {
    Capture(&'a WriterImageRequest),
    Regional(&'a WriterTargetRequest),
}
pub(super) struct Fields {
    pub operation: Uuid,
    pub request: String,
    pub guidance: Option<String>,
    pub model_profile: String,
}
pub(super) enum SourceProof<'a> {
    Capture {
        image: &'a SelectedWriterImage,
        reference: ObjectRef,
        document: Uuid,
        png: Vec<u8>,
    },
    Regional(Box<RegionalImage>),
}
impl<'a> Input<'a> {
    pub fn fields(&self) -> Fields {
        match self {
            Self::Capture(r) => Fields {
                operation: r.operation,
                request: r.request.clone(),
                guidance: r.guidance.clone(),
                model_profile: r.model_profile.clone(),
            },
            Self::Regional(r) => Fields {
                operation: r.operation,
                request: r.request.clone(),
                guidance: r.guidance.clone(),
                model_profile: r.model_profile.clone(),
            },
        }
    }
    pub fn regional(&self) -> bool {
        matches!(self, Self::Regional(_))
    }
    pub fn validate(&self, root: &IntentRecordRef) -> Result<()> {
        if let Self::Capture(r) = self {
            return r.validate(root);
        }
        let Self::Regional(r) = self else {
            unreachable!()
        };
        let reference = IntentRecordRef {
            namespace: r.target.namespace,
            record_id: r.target.record,
            revision_id: r.target.revision,
            object: r.target.object.clone(),
        };
        reference.validate()?;
        root.validate()?;
        validate_request(&r.request)?;
        ensure!(
            reference.namespace == Namespace::Source
                && root.namespace == Namespace::Conversation
                && !r.operation.is_nil()
                && r.operation != root.record_id
                && r.guidance
                    .as_ref()
                    .is_none_or(|s| !s.trim().is_empty() && s.len() <= 4096 && !s.contains('\0'))
                && !r.model_profile.trim().is_empty()
                && r.model_profile.len() <= 128
                && !r.model_profile.chars().any(char::is_control),
            "invalid regional Writer input"
        );
        Ok(())
    }
    pub fn preparation(&self) -> Uuid {
        match self {
            Self::Capture(r) => r.preparation(),
            Self::Regional(r) => stage(r.operation, "target-normalization-admission"),
        }
    }
    pub fn preparation_fingerprint(
        &self,
        root: &IntentRecordRef,
        selection: &IntentSelectionEvidence,
    ) -> Result<String> {
        match self {
            Self::Capture(r) => r.preparation_fingerprint(root, selection),
            Self::Regional(r) => Ok(digest(&serde_json::to_vec(&(
                "selected-writer-region-v1",
                r,
                root,
                selection,
            ))?)),
        }
    }
    pub fn resolve(
        &self,
        writer: &SelectedAdmission,
        store: &Store,
        token: &SelectionToken,
        root: &IntentRecordRef,
        reads: &mut SelectedReadBudget,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<SourceProof<'a>> {
        match self {
            Self::Capture(r) => {
                let value =
                    writer.selected_writer_image_locked(store, token, root, &r.image, reads)?;
                Ok(SourceProof::Capture {
                    image: &r.image,
                    reference: value.reference,
                    document: value.document,
                    png: value.png,
                })
            }
            Self::Regional(r) => Ok(SourceProof::Regional(Box::new(
                writer.selected_writer_region_locked(
                    store, token, root, &r.target, reads, progress,
                )?,
            ))),
        }
    }
}
impl SourceProof<'_> {
    pub fn regional(&self) -> bool {
        matches!(self, Self::Regional(_))
    }
    pub fn document(&self) -> Uuid {
        match self {
            Self::Capture { document, .. } => *document,
            Self::Regional(r) => r.document,
        }
    }
    pub fn page(&self) -> u32 {
        match self {
            Self::Capture { image, .. } => image.page,
            Self::Regional(_) => 1,
        }
    }
    pub fn png(&self) -> &[u8] {
        match self {
            Self::Capture { png, .. } => png,
            Self::Regional(r) => &r.png,
        }
    }
    pub fn image_sha(&self) -> String {
        digest(self.png())
    }
    pub fn exclusions(&self) -> BTreeSet<String> {
        let mut hashes = BTreeSet::from([self.image_sha()]);
        if let Self::Regional(r) = self {
            hashes.insert(r.target.source.original.sha256.clone());
        }
        hashes
    }
    pub fn check(
        &self,
        writer: &SelectedAdmission,
        store: &Store,
        token: &SelectionToken,
        root: &IntentRecordRef,
        reads: &mut SelectedReadBudget,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        match self {
            Self::Capture {
                image,
                reference,
                document,
                ..
            } => {
                let current =
                    writer.selected_writer_image_locked(store, token, root, image, reads)?;
                ensure!(
                    current.document == *document && current.reference == *reference,
                    "selected Writer normalization source changed"
                );
            }
            Self::Regional(r) => r.matches(&writer.selected_writer_region_locked(
                store,
                token,
                root,
                &r.reference,
                reads,
                progress,
            )?)?,
        }
        Ok(())
    }
    pub fn writer_source(
        &self,
        normalized: NormalizedTextSource,
        page: &notebook::NotebookText,
        extractor: &str,
        profile: &str,
    ) -> Result<SelectedWriterSource> {
        let Self::Regional(r) = self else {
            return Ok(SelectedWriterSource::Generate { normalized });
        };
        let region = RegionSource {
            id: normalized.id,
            conversation: normalized.conversation,
            turn: normalized.turn,
            target: r.reference.clone(),
            original: r.target.source.original.clone(),
            derived: crate::storage::Media {
                sha256: self.image_sha(),
                bytes: r.png.len() as u64,
                media_type: "image/png".into(),
            },
            dimensions: r.target.source.dimensions,
            recipe: target::geometry::RECIPE.into(),
            normalization_profile: profile.into(),
            normalized_sha256: digest(&serde_json::to_vec(&normalized.document.units)?),
            about: r.target.candidate.about()?,
            normalized,
            normalization_proof: target::NormalizationProof {
                profile: serde_json::to_value(&page.profile)?,
                prompt_sha256: page.prompt_sha256.clone(),
                response_sha256: page.response_sha256.clone(),
                source_sha256: page.document.source_sha256.clone(),
                extractor_revision: extractor.into(),
            },
        };
        r.admit(region)
    }
}

pub(crate) fn run_target<E: LLMEngine>(
    writer: &SelectedAdmission,
    selection: &Selection,
    input: &WriterTargetRequest,
    engine: &mut E,
    preferences: RequestPreferences,
    memory: Option<&dyn crate::memory::Provider>,
    canceled: &mut dyn FnMut() -> bool,
) -> Result<SelectedReport> {
    run_input(
        writer,
        selection,
        Input::Regional(input),
        engine,
        preferences,
        memory,
        canceled,
    )
}

pub(super) fn now() -> Instant {
    #[cfg(test)]
    {
        AT.with(Cell::get)
            .unwrap_or_else(|| Instant::now() + OFFSET.with(Cell::get))
    }
    #[cfg(not(test))]
    {
        Instant::now()
    }
}
#[cfg(test)]
thread_local! {
    static OFFSET:Cell<Duration>=const{Cell::new(Duration::ZERO)};
    static AT:Cell<Option<Instant>>=const{Cell::new(None)};
    static ADMISSION_RECHECK:Cell<Option<Instant>>=const{Cell::new(None)};
}
#[cfg(test)]
pub(crate) fn advance_for_test(duration: Duration) {
    AT.with(|v| v.set(None));
    OFFSET.with(|v| v.set(duration));
}

/// Advance the deterministic clock during the next locked admission source reread.
#[cfg(test)]
pub(crate) fn advance_at_admission_recheck_for_test(instant: Instant) {
    ADMISSION_RECHECK.with(|v| v.set(Some(instant)));
}
#[cfg(test)]
pub(super) fn admission_recheck_for_test() {
    if let Some(instant) = ADMISSION_RECHECK.with(|v| v.take()) {
        AT.with(|v| v.set(Some(instant)));
    }
}

#[cfg(test)]
pub(crate) struct ClockReset;
#[cfg(test)]
impl ClockReset {
    pub(crate) fn hold_at(&self, instant: Instant) {
        AT.with(|v| v.set(Some(instant)));
    }
    pub(crate) fn admission_recheck_pending(&self) -> bool {
        ADMISSION_RECHECK.with(|v| v.get().is_some())
    }
}
#[cfg(test)]
impl Drop for ClockReset {
    fn drop(&mut self) {
        advance_for_test(Duration::ZERO);
        ADMISSION_RECHECK.with(|v| v.set(None));
    }
}
#[cfg(test)]
pub(crate) fn clock_for_test() -> ClockReset {
    advance_for_test(Duration::ZERO);
    ClockReset
}

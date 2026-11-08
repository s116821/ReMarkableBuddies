//! Persisted historical subjects. No record grants model, native or export authority.
#[path = "target_geometry.rs"]
pub mod geometry;
pub use crate::storage::captured::RecordRef;
use crate::storage::{valid_digest, Media, Namespace, Uuid, MAX_RECORD};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
pub const SCHEMA: u32 = 4;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureKind {
    ImageUse,
    Resident,
    Sdk,
    Legacy,
    Development,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RetainedSourceRef {
    pub kind: CaptureKind,
    pub record: RecordRef,
    pub turn: Uuid,
    pub original: Media,
    pub dimensions: [u32; 2],
    pub overview: Media,
    pub overview_dimensions: [u32; 2],
}
fn reference(value: &RecordRef, namespace: Namespace) -> Result<()> {
    ensure!(
        value.namespace == namespace
            && !value.record.is_nil()
            && !value.revision.is_nil()
            && valid_digest(&value.object.sha256)
            && value.object.bytes > 0
            && value.object.bytes <= MAX_RECORD as u64,
        "invalid target reference"
    );
    Ok(())
}
impl RetainedSourceRef {
    pub fn validate(&self) -> Result<()> {
        reference(&self.record, Namespace::Source)?;
        ensure!(!self.turn.is_nil(), "nil target origin");
        for (media, dimensions) in [
            (&self.original, self.dimensions),
            (&self.overview, self.overview_dimensions),
        ] {
            media.validate()?;
            ensure!(
                media.media_type == "image/png"
                    && media.bytes > 0
                    && media.bytes <= geometry::MAX_PNG_BYTES as u64,
                "target PNG descriptor bound"
            );
            ensure!(
                dimensions[0] > 0
                    && dimensions[1] > 0
                    && u64::from(dimensions[0]) * u64::from(dimensions[1]) <= geometry::MAX_PIXELS,
                "target frame pixel bound"
            );
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", tag = "status", deny_unknown_fields)]
pub enum Understanding {
    Interpreted,
    Uncertain { reason: String },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub subject: String,
    pub geometry: Option<geometry::Geometry>,
    pub understanding: Understanding,
}
impl Candidate {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.subject.trim().is_empty()
                && self.subject.len() <= 512
                && !self.subject.chars().any(char::is_control),
            "invalid target subject"
        );
        if let Some(geometry) = &self.geometry {
            geometry.validate()?;
        }
        match &self.understanding {
            Understanding::Interpreted => ensure!(
                self.geometry.is_some(),
                "interpreted target requires explicit geometry"
            ),
            Understanding::Uncertain { reason } => ensure!(
                !reason.trim().is_empty()
                    && reason.len() <= 128
                    && !reason.chars().any(char::is_control),
                "invalid target uncertainty"
            ),
        }
        ensure!(
            serde_json::to_vec(self)?.len() <= 32 * 1024,
            "target metadata bound"
        );
        Ok(())
    }
    pub fn about(&self) -> Result<String> {
        self.validate()?;
        let mut subject = String::new();
        for c in self.subject.chars() {
            if "\\`*_[]<>".contains(c) {
                subject.push('\\');
            }
            subject.push(c);
        }
        Ok(format!(
            "About: {}{}",
            if matches!(self.understanding, Understanding::Uncertain { .. }) {
                "Uncertain - "
            } else {
                ""
            },
            subject
        ))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceTarget {
    pub id: Uuid,
    pub conversation: Uuid,
    pub introduced_by: Uuid,
    pub source: RetainedSourceRef,
    pub candidate: Candidate,
    pub previous: Option<RecordRef>,
}
impl SourceTarget {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.id.is_nil() && !self.conversation.is_nil() && !self.introduced_by.is_nil(),
            "invalid target identity"
        );
        self.source.validate()?;
        self.candidate.validate()?;
        if let Some(previous) = &self.previous {
            reference(previous, Namespace::Source)?;
            ensure!(
                previous.record == self.id,
                "subject correction changed target scope"
            );
        }
        ensure!(
            serde_json::to_vec(self)?.len() <= 32 * 1024,
            "target metadata bound"
        );
        Ok(())
    }
    pub fn media(&self) -> Vec<Media> {
        let mut result = vec![self.source.original.clone(), self.source.overview.clone()];
        result.sort_by(|a, b| a.sha256.cmp(&b.sha256));
        result.dedup();
        result
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Introduce,
    Inherit,
    CorrectSubject,
    Retarget,
    None,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetUse {
    pub id: Uuid,
    pub conversation: Uuid,
    pub turn: Uuid,
    pub action: Action,
    pub target: Option<RecordRef>,
    pub about: Option<String>,
    pub inline_body_sha256: Option<String>,
}
impl TargetUse {
    pub fn id(turn: Uuid) -> Uuid {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("urn:rmb:target-use:v1:{turn}").as_bytes(),
        )
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.conversation.is_nil() && !self.turn.is_nil() && self.id == Self::id(self.turn),
            "target use identity"
        );
        ensure!(
            (self.action == Action::None) == self.target.is_none(),
            "target use action/reference differs"
        );
        if let Some(target) = &self.target {
            reference(target, Namespace::Source)?;
        }
        ensure!(
            (self.action == Action::None || self.action == Action::Inherit) == self.about.is_none(),
            "target About/action differs"
        );
        if let Some(about) = &self.about {
            ensure!(
                about.starts_with("About: ")
                    && about.len() <= 1200
                    && !about.chars().any(char::is_control),
                "invalid target About presentation"
            );
        }
        ensure!(
            self.inline_body_sha256
                .as_ref()
                .is_none_or(|s| valid_digest(s) && self.about.is_some()),
            "invalid target inline body identity"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetState {
    pub id: Uuid,
    pub conversation: Uuid,
    pub use_reference: RecordRef,
}
impl TargetState {
    pub fn id(conversation: Uuid) -> Uuid {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("urn:rmb:target-state:v1:{conversation}").as_bytes(),
        )
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.conversation.is_nil() && self.id == Self::id(self.conversation),
            "target state identity"
        );
        reference(&self.use_reference, Namespace::Conversation)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RegionSource {
    pub id: Uuid,
    pub conversation: Uuid,
    pub turn: Uuid,
    pub target: RecordRef,
    pub original: Media,
    pub derived: Media,
    pub dimensions: [u32; 2],
    pub recipe: String,
    pub normalization_profile: String,
    pub normalized_sha256: String,
}
impl RegionSource {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.id.is_nil() && !self.conversation.is_nil() && !self.turn.is_nil(),
            "region source identity"
        );
        reference(&self.target, Namespace::Source)?;
        self.original.validate()?;
        self.derived.validate()?;
        ensure!(
            self.recipe == geometry::RECIPE
                && valid_digest(&self.normalized_sha256)
                && !self.normalization_profile.is_empty()
                && self.normalization_profile.len() <= 256,
            "region recipe/normalization identity"
        );
        ensure!(
            self.original.media_type == "image/png"
                && self.derived.media_type == "image/png"
                && self.original.bytes <= geometry::MAX_PNG_BYTES as u64
                && self.derived.bytes <= geometry::MAX_PNG_BYTES as u64,
            "region PNG bound"
        );
        ensure!(
            self.dimensions[0] > 0
                && self.dimensions[1] > 0
                && u64::from(self.dimensions[0]) * u64::from(self.dimensions[1])
                    <= geometry::MAX_PIXELS,
            "region canvas bound"
        );
        Ok(())
    }
    pub fn media(&self) -> Vec<Media> {
        let mut result = vec![self.original.clone(), self.derived.clone()];
        result.sort_by(|a, b| a.sha256.cmp(&b.sha256));
        result.dedup();
        result
    }
}

impl RetainedSourceRef {
    pub(crate) fn validate_record(
        &self,
        record: &super::Record,
        conversation: Uuid,
    ) -> Result<Option<Uuid>> {
        use super::Record;
        self.validate()?;
        let (id, owner, turn, original, dimensions, overview, overview_dimensions, document) =
            match (&self.kind, record) {
                (CaptureKind::ImageUse, Record::Source(value)) => {
                    let (original, dimensions) = if let Some(parent) = &value.parent {
                        let dimensions = value
                            .parent_dimensions
                            .ok_or_else(|| anyhow::anyhow!("target parent frame absent"))?;
                        ensure!(
                            value.crop == Some([0, 0, dimensions[0], dimensions[1]]),
                            "detail crop cannot become original target frame"
                        );
                        (parent.clone(), dimensions)
                    } else {
                        (value.image.clone(), [value.width, value.height])
                    };
                    // The same full-frame normalization used by ImageUse is explicit;
                    // arbitrary affine/strip mappings do not establish region custody.
                    ensure!(
                        value.transform
                            == [
                                1.0 / f64::from(value.width),
                                0.0,
                                0.0,
                                0.0,
                                1.0 / f64::from(value.height),
                                0.0
                            ],
                        "target image mapping is not full-frame normalized"
                    );
                    (
                        value.id,
                        value.conversation,
                        value.turn,
                        original,
                        dimensions,
                        value.image.clone(),
                        [value.width, value.height],
                        Some(value.observation.document),
                    )
                }
                (CaptureKind::Resident, Record::ResidentCapture(value)) => {
                    let overview = value
                        .images
                        .first()
                        .ok_or_else(|| anyhow::anyhow!("target resident overview absent"))?;
                    ensure!(
                        overview.provider_ordinal == 0
                            && overview.parent_crop
                                == [
                                    0,
                                    0,
                                    value.original_dimensions[0],
                                    value.original_dimensions[1]
                                ],
                        "target resident overview mapping differs"
                    );
                    (
                        value.id,
                        value.conversation,
                        value.turn,
                        value.original_png.clone(),
                        value.original_dimensions,
                        overview.media.clone(),
                        overview.dimensions,
                        Some(value.before.document),
                    )
                }
                (CaptureKind::Development, Record::DevelopmentCapture(value)) => {
                    let overview = value
                        .images
                        .first()
                        .ok_or_else(|| anyhow::anyhow!("target development overview absent"))?;
                    ensure!(
                        overview.provider_ordinal == 0
                            && overview.parent_crop
                                == [
                                    0,
                                    0,
                                    value.original_dimensions[0],
                                    value.original_dimensions[1]
                                ],
                        "target development overview mapping differs"
                    );
                    (
                        value.id,
                        value.conversation,
                        value.turn,
                        value.original_png.clone(),
                        value.original_dimensions,
                        overview.media.clone(),
                        overview.dimensions,
                        None,
                    )
                }
                (CaptureKind::Legacy, Record::LegacyCapture(value)) => {
                    let overview = value
                        .images
                        .iter()
                        .find(|i| i.role == super::LegacyImageRole::Overview)
                        .ok_or_else(|| anyhow::anyhow!("legacy target overview absent"))?;
                    let parent = value.parent.as_ref().unwrap_or(overview);
                    // Legacy has no derivation mapping. Only the exact parent itself
                    // can serve as its target frame; unknown resized evidence refuses.
                    ensure!(
                        parent.media == overview.media && parent.dimensions == overview.dimensions,
                        "legacy target overview mapping unproved"
                    );
                    (
                        value.id,
                        value.conversation,
                        value.turn,
                        parent.media.clone(),
                        parent.dimensions,
                        overview.media.clone(),
                        overview.dimensions,
                        None,
                    )
                }
                (CaptureKind::Sdk, Record::Capture(value)) => {
                    let parent = &value.facts.native_parent;
                    let overview = value
                        .facts
                        .images
                        .iter()
                        .find(|i| i.role == super::capture_facts::Role::Overview)
                        .ok_or_else(|| anyhow::anyhow!("SDK target overview absent"))?;
                    // Initial support requires the exact retained parent frame, not
                    // an arbitrary historical derivation or a synthetic native lease.
                    ensure!(
                        overview.full_frame_of(parent),
                        "SDK target overview mapping unproved"
                    );
                    (
                        value.id,
                        value.conversation,
                        value.turn,
                        parent.media(),
                        parent.dimensions,
                        overview.media(),
                        overview.dimensions,
                        Some(value.facts.source.document.value()),
                    )
                }
                _ => anyhow::bail!("target retained source kind differs"),
            };
        ensure!(
            id == self.record.record
                && owner == conversation
                && turn == self.turn
                && original == self.original
                && dimensions == self.dimensions
                && overview == self.overview
                && overview_dimensions == self.overview_dimensions,
            "target original/source correspondence differs"
        );
        Ok(document)
    }
}

/// Common closure validation over exact immutable references. The caller chooses
/// ordinary committed records or the selected included closure, never a fallback.
pub(crate) fn validate_links(
    record: &super::Record,
    exact: &impl Fn(&RecordRef) -> Result<super::Record>,
    turn: &impl Fn(Uuid) -> Result<super::Turn>,
) -> Result<Option<Uuid>> {
    use super::Record;
    match record {
        Record::SourceTarget(value) => {
            value.validate()?;
            let source = exact(&value.source.record)?;
            let document = value.source.validate_record(&source, value.conversation)?;
            let origin = turn(value.source.turn)?;
            let introduced = turn(value.introduced_by)?;
            ensure!(
                origin.role == super::Role::User
                    && origin.conversation == value.conversation
                    && origin.sources.contains(&value.source.record.record)
                    && introduced.role == super::Role::User
                    && introduced.conversation == value.conversation
                    && origin.sequence <= introduced.sequence,
                "target original/introducing turn differs"
            );
            if let Some(previous) = &value.previous {
                let Record::SourceTarget(prior) = exact(previous)? else {
                    anyhow::bail!("target correction predecessor differs")
                };
                ensure!(
                    prior.id == value.id
                        && prior.conversation == value.conversation
                        && prior.source == value.source
                        && prior.candidate.geometry == value.candidate.geometry,
                    "subject correction changed original region"
                );
            }
            Ok(document)
        }
        Record::TargetUse(value) => {
            value.validate()?;
            let user = turn(value.turn)?;
            ensure!(
                user.role == super::Role::User && user.conversation == value.conversation,
                "target use user differs"
            );
            if let Some(reference) = &value.target {
                let Record::SourceTarget(target) = exact(reference)? else {
                    anyhow::bail!("target use exact target absent")
                };
                ensure!(
                    target.conversation == value.conversation,
                    "foreign target use"
                );
                if let Some(about) = &value.about {
                    ensure!(
                        *about == target.candidate.about()?,
                        "target About differs from exact target revision"
                    );
                }
            }
            Ok(None)
        }
        Record::TargetState(value) => {
            value.validate()?;
            let Record::TargetUse(used) = exact(&value.use_reference)? else {
                anyhow::bail!("target state exact use absent")
            };
            ensure!(
                used.conversation == value.conversation && used.action != Action::Inherit,
                "target state update requires explicit target action"
            );
            Ok(None)
        }
        Record::RegionSource(value) => {
            value.validate()?;
            let Record::SourceTarget(target) = exact(&value.target)? else {
                anyhow::bail!("region target exact revision absent")
            };
            let user = turn(value.turn)?;
            ensure!(
                target.conversation == value.conversation
                    && user.conversation == value.conversation
                    && user.role == super::Role::User
                    && target.source.original == value.original
                    && target.source.dimensions == value.dimensions
                    && matches!(target.candidate.understanding, Understanding::Interpreted)
                    && target.candidate.geometry.is_some(),
                "region original/target custody differs"
            );
            Ok(None)
        }
        _ => Ok(None),
    }
}

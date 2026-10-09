//! Historical storage containers; these cannot reconstruct SDK live observations.
use crate::storage::{valid_digest, Media, Uuid};
use anyhow::{ensure, Context, Result};
use remarkable_open_sdk::{
    capture::{CapturedBatch, ImageRole, ResizeFilter},
    evidence::{self, CaptureFacts, DerivationFacts, ImageFacts, ObservationFacts},
};
use serde::{Deserialize, Serialize};

pub const SDK_SOURCE: &str = "2d473f0954120889f8a04c6294151241576ff8c3";

fn required_option<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// JSON strings preserve integers across consumers that use IEEE-754 JSON numbers.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "String", into = "String")]
pub struct ExactU64(u64);
impl ExactU64 {
    pub fn value(self) -> u64 {
        self.0
    }
}
impl From<ExactU64> for String {
    fn from(value: ExactU64) -> Self {
        value.0.to_string()
    }
}
impl TryFrom<String> for ExactU64 {
    type Error = &'static str;
    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        let number = value
            .parse::<u64>()
            .map_err(|_| "invalid exact unsigned integer")?;
        if number.to_string() != value {
            return Err("noncanonical exact unsigned integer");
        }
        Ok(Self(number))
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "String", into = "String")]
pub struct CanonicalUuid(Uuid);
impl CanonicalUuid {
    pub fn value(self) -> Uuid {
        self.0
    }
}
impl From<CanonicalUuid> for String {
    fn from(value: CanonicalUuid) -> Self {
        value.0.to_string()
    }
}
impl TryFrom<String> for CanonicalUuid {
    type Error = &'static str;
    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        let id = Uuid::parse_str(&value).map_err(|_| "invalid historical UUID")?;
        if id.is_nil() || id.to_string() != value {
            return Err("noncanonical historical UUID");
        }
        Ok(Self(id))
    }
}
fn uuid(value: remarkable_open_sdk::Uuid) -> CanonicalUuid {
    CanonicalUuid(Uuid::from_bytes(*value.as_bytes()))
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Origin {
    #[serde(rename = "Synthetic")]
    Synthetic,
}
impl From<remarkable_open_sdk::EvidenceOrigin> for Origin {
    fn from(value: remarkable_open_sdk::EvidenceOrigin) -> Self {
        match value {
            remarkable_open_sdk::EvidenceOrigin::Synthetic => Self::Synthetic,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Qualification {
    #[serde(rename = "UnqualifiedSyntheticModel")]
    UnqualifiedSyntheticModel,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Clock {
    #[serde(rename = "SyntheticInjectedDuration")]
    SyntheticInjectedDuration,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum RenderBinding {
    #[serde(rename = "SyntheticEqualityAssertion")]
    SyntheticEqualityAssertion,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum Role {
    #[serde(rename = "NativeParent")]
    NativeParent,
    #[serde(rename = "Overview")]
    Overview,
    #[serde(rename = "Detail")]
    Detail { ordinal: u32 },
}
impl From<ImageRole> for Role {
    fn from(value: ImageRole) -> Self {
        match value {
            ImageRole::NativeParent => Self::NativeParent,
            ImageRole::Overview => Self::Overview,
            ImageRole::Detail(ordinal) => Self::Detail { ordinal },
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Filter {
    #[serde(rename = "Nearest")]
    Nearest,
    #[serde(rename = "Triangle")]
    Triangle,
}
impl From<ResizeFilter> for Filter {
    fn from(value: ResizeFilter) -> Self {
        match value {
            ResizeFilter::Nearest => Self::Nearest,
            ResizeFilter::Triangle => Self::Triangle,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HistoricalObservation {
    pub(crate) device: CanonicalUuid,
    pub(crate) instance: ExactU64,
    pub(crate) session: ExactU64,
    pub(crate) visit: ExactU64,
    pub(crate) revision: ExactU64,
    pub(crate) input_epoch: ExactU64,
    pub(crate) document: CanonicalUuid,
    pub(crate) page: CanonicalUuid,
    pub(crate) order: Vec<CanonicalUuid>,
    pub(crate) origin: Origin,
}
impl From<&ObservationFacts> for HistoricalObservation {
    fn from(facts: &ObservationFacts) -> Self {
        Self {
            device: uuid(facts.device()),
            instance: ExactU64(facts.instance()),
            session: ExactU64(facts.session()),
            visit: ExactU64(facts.visit()),
            revision: ExactU64(facts.revision()),
            input_epoch: ExactU64(facts.input_epoch()),
            document: uuid(facts.page().document),
            page: uuid(facts.page().page),
            order: facts.order().iter().copied().map(uuid).collect(),
            origin: facts.origin().into(),
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HistoricalDuration {
    seconds: ExactU64,
    nanoseconds: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HistoricalDerivation {
    parent_digest: String,
    parent_dimensions: [u32; 2],
    crop: [u32; 4],
    output_dimensions: [u32; 2],
    filter: Filter,
    procedure: String,
}
fn hex(digest: [u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
impl From<&DerivationFacts> for HistoricalDerivation {
    fn from(facts: &DerivationFacts) -> Self {
        Self {
            parent_digest: hex(facts.parent_digest()),
            parent_dimensions: facts.parent_dimensions(),
            crop: facts.crop(),
            output_dimensions: facts.output_dimensions(),
            filter: facts.filter().into(),
            procedure: facts.procedure().to_owned(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HistoricalImage {
    pub(crate) role: Role,
    pub(crate) bytes: ExactU64,
    pub(crate) sha256: String,
    pub(crate) mime: String,
    pub(crate) dimensions: [u32; 2],
    pub(crate) affine_bits: [ExactU64; 6],
    pub(crate) valid_region_bits: [ExactU64; 4],
    #[serde(deserialize_with = "required_option")]
    pub(crate) derivation: Option<HistoricalDerivation>,
}
impl From<&ImageFacts> for HistoricalImage {
    fn from(facts: &ImageFacts) -> Self {
        Self {
            role: facts.role().into(),
            bytes: ExactU64(facts.bytes()),
            sha256: hex(facts.sha256()),
            mime: facts.mime().to_owned(),
            dimensions: facts.dimensions(),
            affine_bits: facts.affine_bits().map(ExactU64),
            valid_region_bits: facts.valid_region_bits().map(ExactU64),
            derivation: facts.derivation().map(HistoricalDerivation::from),
        }
    }
}
impl HistoricalImage {
    pub fn media(&self) -> Media {
        Media {
            sha256: self.sha256.clone(),
            bytes: self.bytes.value(),
            media_type: self.mime.clone(),
        }
    }
    fn validate(&self) -> Result<()> {
        self.media().validate()?;
        ensure!(
            self.mime == "image/png" && self.dimensions.iter().all(|n| *n > 0 && *n <= 32768),
            "invalid historical image descriptor"
        );
        let affine = self.affine_bits.map(|n| f64::from_bits(n.value()));
        let affine = remarkable_open_sdk::capture::Affine::new(affine)?;
        let [x, y, width, height] = self.valid_region_bits.map(|n| f64::from_bits(n.value()));
        ensure!(
            [x, y, width, height].iter().all(|n| n.is_finite())
                && x >= 0.0
                && y >= 0.0
                && width > 0.0
                && height > 0.0
                && x + width <= f64::from(self.dimensions[0])
                && y + height <= f64::from(self.dimensions[1]),
            "invalid historical source region"
        );
        for point in [
            [x, y],
            [x + width, y],
            [x, y + height],
            [x + width, y + height],
        ] {
            ensure!(
                affine
                    .map(point)?
                    .iter()
                    .all(|coordinate| *coordinate >= -1e-10 && *coordinate <= 1.0 + 1e-10),
                "historical source region exceeds page plane"
            );
        }
        if let Some(derivation) = &self.derivation {
            ensure!(
                valid_digest(&derivation.parent_digest)
                    && derivation.output_dimensions == self.dimensions
                    && derivation.parent_dimensions.iter().all(|n| *n > 0),
                "invalid historical lineage"
            );
            let [x, y, width, height] = derivation.crop;
            ensure!(
                width > 0
                    && height > 0
                    && x.checked_add(width)
                        .is_some_and(|right| right <= derivation.parent_dimensions[0])
                    && y.checked_add(height)
                        .is_some_and(|bottom| bottom <= derivation.parent_dimensions[1]),
                "invalid historical crop"
            );
        }
        Ok(())
    }
}

/// Restored historical data has no method/conversion returning an SDK live guard.
/// ```compile_fail
/// use remarkable_reader_buddy::conversation::capture_facts::HistoricalCapture;
/// use remarkable_open_sdk::Platform;
/// fn restored_capture(platform: &mut impl Platform, saved: HistoricalCapture) {
///     platform.capture(&saved);
/// }
/// ```
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HistoricalCapture {
    schema: String,
    sdk_source: String,
    pub(crate) operation: CanonicalUuid,
    pub(crate) source: HistoricalObservation,
    qualification: Qualification,
    clock: Clock,
    #[serde(deserialize_with = "required_option")]
    global_clock_scope: Option<String>,
    #[serde(deserialize_with = "required_option")]
    native_profile: Option<String>,
    interval: [HistoricalDuration; 2],
    render_binding: RenderBinding,
    render_generation: ExactU64,
    buffer_generation: ExactU64,
    pub(crate) viewport_revision: String,
    pub(crate) conversion_procedure: String,
    pub(crate) procedure_revision: String,
    #[serde(deserialize_with = "required_option")]
    pub(crate) target_bits: Option<[ExactU64; 2]>,
    pub(crate) native_parent: HistoricalImage,
    pub(crate) images: Vec<HistoricalImage>,
}
impl HistoricalCapture {
    pub fn from_batch(batch: &CapturedBatch) -> Result<Self> {
        let facts = batch.export_facts(evidence::ExportLimits::default())?;
        ensure!(
            facts.qualification() == "UnqualifiedSyntheticModel"
                && facts.clock_kind() == "SyntheticInjectedDuration"
                && facts.render_binding() == "SyntheticEqualityAssertion",
            "unsupported historical SDK qualification/clock/render meaning"
        );
        let result = Self::from_facts(&facts);
        result.validate()?;
        Ok(result)
    }
    fn from_facts(facts: &CaptureFacts) -> Self {
        Self {
            schema: facts.schema().to_owned(),
            sdk_source: SDK_SOURCE.to_owned(),
            operation: uuid(facts.operation()),
            source: HistoricalObservation::from(facts.source()),
            qualification: Qualification::UnqualifiedSyntheticModel,
            clock: Clock::SyntheticInjectedDuration,
            global_clock_scope: None,
            native_profile: None,
            interval: facts.interval().map(|value| HistoricalDuration {
                seconds: ExactU64(value.seconds()),
                nanoseconds: value.nanoseconds(),
            }),
            render_binding: RenderBinding::SyntheticEqualityAssertion,
            render_generation: ExactU64(facts.render_generation()),
            buffer_generation: ExactU64(facts.buffer_generation()),
            viewport_revision: facts.viewport_revision().to_owned(),
            conversion_procedure: facts.conversion_procedure().to_owned(),
            procedure_revision: facts.procedure_revision().to_owned(),
            target_bits: facts.target_bits().map(|values| values.map(ExactU64)),
            native_parent: HistoricalImage::from(facts.native_parent()),
            images: facts.images().iter().map(HistoricalImage::from).collect(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == evidence::SCHEMA && self.sdk_source == SDK_SOURCE,
            "unsupported historical SDK schema/source"
        );
        ensure!(
            self.global_clock_scope.is_none() && self.native_profile.is_none(),
            "synthetic evidence has no native profile or global clock"
        );
        ensure!(
            self.render_generation == self.buffer_generation && self.buffer_generation.value() != 0,
            "invalid synthetic render assertion"
        );
        ensure!(
            self.interval
                .iter()
                .all(|value| value.nanoseconds < 1_000_000_000)
                && (
                    self.interval[0].seconds.value(),
                    self.interval[0].nanoseconds
                ) <= (
                    self.interval[1].seconds.value(),
                    self.interval[1].nanoseconds
                ),
            "invalid historical acquisition interval"
        );
        ensure!(
            !self.source.order.is_empty()
                && self.source.order.len() <= 4096
                && self.source.order.contains(&self.source.page),
            "invalid historical source order"
        );
        ensure!(
            self.source
                .order
                .iter()
                .map(|id| id.value())
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.source.order.len(),
            "duplicate historical order identity"
        );
        ensure!(
            !self.images.is_empty() && self.images.len() < 16,
            "historical image count exceeds bound"
        );
        ensure!(
            self.native_parent.role == Role::NativeParent
                && self.native_parent.derivation.is_none(),
            "invalid historical native parent"
        );
        self.native_parent.validate()?;
        let mut variable_bytes = self
            .source
            .order
            .len()
            .checked_mul(16)
            .context("historical metadata overflow")?;
        let mut strings = vec![
            self.viewport_revision.as_str(),
            self.conversion_procedure.as_str(),
            self.procedure_revision.as_str(),
        ];
        let mut overview = false;
        let mut details = std::collections::BTreeSet::new();
        for image in &self.images {
            image.validate()?;
            match image.role {
                Role::Overview => {
                    ensure!(!overview, "duplicate historical overview");
                    overview = true;
                }
                Role::Detail { ordinal } => {
                    ensure!(details.insert(ordinal), "duplicate historical detail")
                }
                Role::NativeParent => anyhow::bail!("native parent in provider image list"),
            }
            let derivation = image
                .derivation
                .as_ref()
                .context("historical derivative lineage absent")?;
            ensure!(
                derivation.parent_digest == self.native_parent.sha256
                    && derivation.parent_dimensions == self.native_parent.dimensions,
                "historical derivative parent mismatch"
            );
            ensure!(
                derivation.procedure == "image-0.25.10/crop-resize-8bit-v1",
                "unsupported historical derivative procedure"
            );
            let [x, y, width, height] = self
                .native_parent
                .valid_region_bits
                .map(|value| f64::from_bits(value.value()));
            let [crop_x, crop_y, crop_width, crop_height] = derivation.crop;
            let geometry = remarkable_open_sdk::capture::derive_geometry(
                self.native_parent.dimensions,
                remarkable_open_sdk::capture::Affine::new(
                    self.native_parent
                        .affine_bits
                        .map(|value| f64::from_bits(value.value())),
                )?,
                remarkable_open_sdk::capture::SourceRegion {
                    x,
                    y,
                    width,
                    height,
                },
                remarkable_open_sdk::capture::PixelRect {
                    x: crop_x,
                    y: crop_y,
                    width: crop_width,
                    height: crop_height,
                },
                derivation.output_dimensions,
            )?;
            ensure!(
                geometry.affine().coefficients().map(f64::to_bits)
                    == image.affine_bits.map(ExactU64::value),
                "historical derivative affine does not compose with parent"
            );
            let region = geometry.valid_source_region();
            ensure!(
                [region.x, region.y, region.width, region.height].map(f64::to_bits)
                    == image.valid_region_bits.map(ExactU64::value),
                "historical derivative valid region does not match parent/crop"
            );
            if image.role == Role::Overview {
                ensure!(
                    derivation.crop
                        == [
                            0,
                            0,
                            self.native_parent.dimensions[0],
                            self.native_parent.dimensions[1]
                        ],
                    "historical overview must cover parent"
                );
            }
            strings.push(&derivation.procedure);
        }
        ensure!(overview, "historical overview absent");
        for string in strings {
            ensure!(
                !string.is_empty() && string.len() <= 4096,
                "invalid historical procedure/viewport string"
            );
            variable_bytes = variable_bytes
                .checked_add(string.len())
                .context("historical metadata overflow")?;
        }
        ensure!(
            variable_bytes <= 64 * 1024,
            "historical variable metadata exceeds bound"
        );
        if let Some(target) = self.target_bits {
            ensure!(
                target.iter().all(|n| {
                    let value = f64::from_bits(n.value());
                    value.is_finite() && (0.0..=1.0).contains(&value)
                }),
                "invalid historical normalized target"
            );
            let affine = remarkable_open_sdk::capture::Affine::new(
                self.native_parent
                    .affine_bits
                    .map(|value| f64::from_bits(value.value())),
            )?;
            let point = affine.inverse(target.map(|value| f64::from_bits(value.value())))?;
            let [x, y, width, height] = self
                .native_parent
                .valid_region_bits
                .map(|value| f64::from_bits(value.value()));
            ensure!(
                point[0] >= x && point[0] <= x + width && point[1] >= y && point[1] <= y + height,
                "historical target is outside source viewport"
            );
        }
        Ok(())
    }
    pub fn media(&self) -> Result<Vec<Media>> {
        self.validate()?;
        let mut media = std::collections::BTreeMap::new();
        for image in std::iter::once(&self.native_parent).chain(&self.images) {
            let descriptor = image.media();
            if let Some(prior) = media.insert(descriptor.sha256.clone(), descriptor.clone()) {
                ensure!(
                    prior == descriptor,
                    "inconsistent historical media descriptors"
                );
            }
        }
        Ok(media.into_values().collect())
    }
}

//! Explicit confirmed sample data. No recognition, native authority or model training.
use crate::storage::{digest, valid_digest, Envelope, Kind, Media, Namespace, Uuid};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{de::Error, Deserialize, Deserializer, Serialize};
use std::{
    collections::BTreeSet,
    io::{Cursor, Read},
};

pub const MAX_INPUT: usize = 768 * 1024;
pub const MAX_SAMPLE: usize = 512 * 1024;
pub const MAX_TEXT: usize = 4096;

/// Closed failure vocabulary; never carries source text, image bytes or OS paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Failure {
    InvalidJson,
    InvalidVersion,
    InvalidIdentity,
    InvalidConfirmation,
    InvalidText,
    InvalidProvenance,
    InvalidSample,
    InputLimit,
    ImageLimit,
    InvalidBudget,
    OutputLimit,
    UnsupportedMode,
    UnsupportedRecord,
    StoreUnavailable,
    OwnershipBusy,
    HeadsConflict,
    OperationConflict,
    ImmutableChange,
    MediaUnavailable,
    CommitUnknown,
    StorageFailure,
}
pub type Result<T> = std::result::Result<T, Failure>;

fn parse_id(value: &str) -> std::result::Result<Uuid, &'static str> {
    let id = Uuid::parse_str(value).map_err(|_| "invalid identity")?;
    if id.is_nil() || id.to_string() != value {
        return Err("invalid identity");
    }
    Ok(id)
}
fn id<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Uuid, D::Error> {
    parse_id(&String::deserialize(d)?).map_err(D::Error::custom)
}
fn heads<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Vec<Uuid>, D::Error> {
    let values = Vec::<String>::deserialize(d)?;
    if values.len() > crate::storage::MAX_ITEMS {
        return Err(D::Error::custom("head limit"));
    }
    let values: Vec<_> = values
        .iter()
        .map(|s| parse_id(s).map_err(D::Error::custom))
        .collect::<std::result::Result<_, _>>()?;
    if values.iter().collect::<BTreeSet<_>>().len() != values.len() {
        return Err(D::Error::custom("duplicate heads"));
    }
    Ok(values)
}
fn source<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<[Uuid; 3]>, D::Error> {
    let values = Option::<[String; 3]>::deserialize(d)?;
    values
        .map(|v| {
            Ok([
                parse_id(&v[0]).map_err(D::Error::custom)?,
                parse_id(&v[1]).map_err(D::Error::custom)?,
                parse_id(&v[2]).map_err(D::Error::custom)?,
            ])
        })
        .transpose()
}
fn text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= limit
        && !value
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
}
fn nullable_text<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProvenanceKind {
    CallerConfirmedSample,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub kind: ProvenanceKind,
    pub description: String,
    #[serde(deserialize_with = "source")]
    pub source: Option<[Uuid; 3]>,
}
impl Provenance {
    fn validate(&self) -> Result<()> {
        if !text(&self.description, 512)
            || self
                .source
                .as_ref()
                .is_some_and(|s| s.iter().any(Uuid::is_nil))
        {
            return Err(Failure::InvalidProvenance);
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CorrectionKind {
    ConfirmedCorrection,
}

/// Field order is part of the version-one digest recipe. No content-bearing Debug.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Correction {
    pub kind: CorrectionKind,
    #[serde(deserialize_with = "id")]
    pub profile: Uuid,
    #[serde(deserialize_with = "id")]
    pub correction: Uuid,
    pub original_text: String,
    pub corrected_text: String,
    pub confirmed_ms: u64,
    pub provenance: Provenance,
    pub sample: Media,
    pub width: u32,
    pub height: u32,
}
impl Correction {
    pub fn validate(&self) -> Result<()> {
        if self.profile.is_nil() || self.correction.is_nil() {
            return Err(Failure::InvalidIdentity);
        }
        if !text(&self.original_text, MAX_TEXT)
            || !text(&self.corrected_text, MAX_TEXT)
            || self.confirmed_ms == 0
        {
            return Err(Failure::InvalidText);
        }
        self.provenance.validate()?;
        if !valid_digest(&self.sample.sha256)
            || self.sample.bytes == 0
            || self.sample.bytes > MAX_SAMPLE as u64
            || self.sample.media_type != "image/png"
        {
            return Err(Failure::InvalidSample);
        }
        dimensions(self.width, self.height)?;
        Ok(())
    }
    pub fn record_id(&self) -> Result<Uuid> {
        record_id(self.profile, self.correction)
    }
    pub fn value_digest(&self) -> Result<String> {
        self.validate()?;
        Ok(digest(
            &serde_json::to_vec(self).map_err(|_| Failure::StorageFailure)?,
        ))
    }
    pub fn same_original(&self, other: &Self) -> bool {
        self.profile == other.profile
            && self.correction == other.correction
            && self.original_text == other.original_text
            && self.provenance.source == other.provenance.source
            && self.sample == other.sample
            && self.width == other.width
            && self.height == other.height
    }
    pub fn verify_sample(&self, bytes: &[u8]) -> Result<()> {
        self.validate()?;
        if bytes.len() as u64 != self.sample.bytes || digest(bytes) != self.sample.sha256 {
            return Err(Failure::MediaUnavailable);
        }
        if png_dimensions(bytes).map_err(|_| Failure::MediaUnavailable)?
            != (self.width, self.height)
        {
            return Err(Failure::MediaUnavailable);
        }
        Ok(())
    }
}
pub fn record_id(profile: Uuid, correction: Uuid) -> Result<Uuid> {
    if profile.is_nil() || correction.is_nil() {
        return Err(Failure::InvalidIdentity);
    }
    Ok(Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("urn:rmb:handwriting-correction:v1:{profile}:{correction}").as_bytes(),
    ))
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Confirm,
    Update,
    Forget,
}
pub fn mutation_identity(
    action: Action,
    operation: Uuid,
    record: Uuid,
    expected: &BTreeSet<Uuid>,
    value_digest: Option<&str>,
) -> Result<(String, Uuid)> {
    if operation.is_nil()
        || record.is_nil()
        || expected.len() > crate::storage::MAX_ITEMS
        || expected.iter().any(Uuid::is_nil)
        || value_digest.is_some_and(|s| !valid_digest(s))
        || (action == Action::Forget) != value_digest.is_none()
    {
        return Err(Failure::InvalidIdentity);
    }
    let fingerprint = digest(
        &serde_json::to_vec(&(
            "handwriting-mutation-v1",
            action,
            record,
            expected,
            value_digest,
        ))
        .map_err(|_| Failure::StorageFailure)?,
    );
    let revision = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("urn:rmb:handwriting-revision:v1:{operation}:{fingerprint}").as_bytes(),
    );
    Ok((fingerprint, revision))
}

/// Strict metadata only; sample usability must be verified separately.
pub fn decode(envelope: &Envelope) -> Result<Option<Correction>> {
    envelope
        .validate()
        .map_err(|_| Failure::UnsupportedRecord)?;
    if envelope.namespace != Namespace::Handwriting || envelope.domain_schema_version != 1 {
        return Err(Failure::UnsupportedRecord);
    }
    if envelope.kind == Kind::Tombstone {
        return Ok(None);
    }
    let value: Correction =
        serde_json::from_value(envelope.payload.clone()).map_err(|_| Failure::UnsupportedRecord)?;
    value.validate().map_err(|_| Failure::UnsupportedRecord)?;
    if value.record_id()? != envelope.record_id
        || envelope.media_descriptors != [value.sample.clone()]
    {
        return Err(Failure::UnsupportedRecord);
    }
    Ok(Some(value))
}

fn dimensions(width: u32, height: u32) -> Result<()> {
    if width == 0
        || height == 0
        || width > 2048
        || height > 2048
        || u64::from(width) * u64::from(height) > 4_194_304
    {
        return Err(Failure::ImageLimit);
    }
    Ok(())
}
pub fn png_dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    if bytes.is_empty() || bytes.len() > MAX_SAMPLE {
        return Err(Failure::ImageLimit);
    }
    if image::guess_format(bytes).map_err(|_| Failure::InvalidSample)? != image::ImageFormat::Png {
        return Err(Failure::InvalidSample);
    }
    let size = image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png)
        .into_dimensions()
        .map_err(|_| Failure::InvalidSample)?;
    dimensions(size.0, size.1)?;
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().map_err(|_| Failure::InvalidSample)?;
    Ok(size)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmInput {
    pub handwriting_version: u32,
    #[serde(deserialize_with = "id")]
    pub profile: Uuid,
    #[serde(deserialize_with = "id")]
    pub correction: Uuid,
    #[serde(deserialize_with = "id")]
    pub operation: Uuid,
    #[serde(deserialize_with = "heads")]
    pub expected_heads: Vec<Uuid>,
    pub confirmation: String,
    pub original_text: String,
    pub corrected_text: String,
    pub confirmed_ms: u64,
    pub provenance: Provenance,
    pub sample_png_base64: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyInput {
    pub handwriting_version: u32,
    #[serde(deserialize_with = "id")]
    pub profile: Uuid,
    #[serde(deserialize_with = "id")]
    pub correction: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationInput {
    pub handwriting_version: u32,
    #[serde(deserialize_with = "id")]
    pub profile: Uuid,
    #[serde(deserialize_with = "id")]
    pub correction: Uuid,
    #[serde(deserialize_with = "id")]
    pub operation: Uuid,
    #[serde(deserialize_with = "heads")]
    pub expected_heads: Vec<Uuid>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusInput {
    pub handwriting_version: u32,
    #[serde(deserialize_with = "id")]
    pub profile: Uuid,
    #[serde(deserialize_with = "id")]
    pub correction: Uuid,
    #[serde(deserialize_with = "id")]
    pub operation: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryInput {
    pub handwriting_version: u32,
    #[serde(deserialize_with = "id")]
    pub profile: Uuid,
    #[serde(deserialize_with = "nullable_text")]
    pub query_text: Option<String>,
    pub max_matches: usize,
    pub max_encoded_bytes: usize,
}
#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    Confirm(Box<ConfirmInput>),
    Update(Box<ConfirmInput>),
    Inspect(KeyInput),
    Query(QueryInput),
    Forget(MutationInput),
    Status(StatusInput),
}
pub struct ValidatedInput {
    request: Request,
    correction: Option<Correction>,
    sample: Option<Vec<u8>>,
}
impl ValidatedInput {
    pub fn request(&self) -> &Request {
        &self.request
    }
    pub fn correction(&self) -> Option<&Correction> {
        self.correction.as_ref()
    }
    pub fn sample(&self) -> Option<&[u8]> {
        self.sample.as_deref()
    }
}
pub fn parse(input: &mut impl Read) -> Result<ValidatedInput> {
    let mut bytes = Vec::new();
    input
        .take(MAX_INPUT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Failure::InvalidJson)?;
    if bytes.len() > MAX_INPUT {
        return Err(Failure::InputLimit);
    }
    let request: Request = serde_json::from_slice(&bytes).map_err(|error| {
        if error.to_string().starts_with("invalid identity") {
            Failure::InvalidIdentity
        } else {
            Failure::InvalidJson
        }
    })?;
    let version = match &request {
        Request::Confirm(v) | Request::Update(v) => v.handwriting_version,
        Request::Inspect(v) => v.handwriting_version,
        Request::Query(v) => v.handwriting_version,
        Request::Forget(v) => v.handwriting_version,
        Request::Status(v) => v.handwriting_version,
    };
    if version != 1 {
        return Err(Failure::InvalidVersion);
    }
    let (correction, sample) = match &request {
        Request::Confirm(v) | Request::Update(v) => {
            if v.confirmation != "user-confirmed" {
                return Err(Failure::InvalidConfirmation);
            }
            if (matches!(&request, Request::Confirm(_)) && !v.expected_heads.is_empty())
                || (matches!(&request, Request::Update(_)) && v.expected_heads.is_empty())
            {
                return Err(Failure::HeadsConflict);
            }
            if v.sample_png_base64.len() > MAX_SAMPLE.div_ceil(3) * 4 {
                return Err(Failure::ImageLimit);
            }
            let sample = STANDARD
                .decode(&v.sample_png_base64)
                .map_err(|_| Failure::InvalidSample)?;
            if STANDARD.encode(&sample) != v.sample_png_base64 {
                return Err(Failure::InvalidSample);
            }
            let (width, height) = png_dimensions(&sample)?;
            let value = Correction {
                kind: CorrectionKind::ConfirmedCorrection,
                profile: v.profile,
                correction: v.correction,
                original_text: v.original_text.clone(),
                corrected_text: v.corrected_text.clone(),
                confirmed_ms: v.confirmed_ms,
                provenance: v.provenance.clone(),
                sample: Media {
                    sha256: digest(&sample),
                    bytes: sample.len() as u64,
                    media_type: "image/png".into(),
                },
                width,
                height,
            };
            value.validate()?;
            (Some(value), Some(sample))
        }
        Request::Forget(v) if v.expected_heads.is_empty() => return Err(Failure::HeadsConflict),
        Request::Query(v) => {
            if !(1..=16).contains(&v.max_matches) || !(512..=65536).contains(&v.max_encoded_bytes) {
                return Err(Failure::InvalidBudget);
            }
            if v.query_text.as_ref().is_some_and(|q| !text(q, 2048)) {
                return Err(Failure::InvalidText);
            }
            (None, None)
        }
        _ => (None, None),
    };
    Ok(ValidatedInput {
        request,
        correction,
        sample,
    })
}

#[cfg(test)]
mod tests;

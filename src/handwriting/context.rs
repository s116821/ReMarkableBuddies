//! Owned, explicitly selected confirmed examples; no implicit search or mutation.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExampleSelection {
    #[serde(deserialize_with = "id")]
    pub correction: Uuid,
    #[serde(deserialize_with = "id")]
    pub revision: Uuid,
}
#[derive(Clone, Serialize)]
pub struct ExampleIdentity {
    pub correction: Uuid,
    pub record: Uuid,
    pub revision: Uuid,
    pub sample_sha256: String,
    pub original_text_sha256: Option<String>,
    pub corrected_text_sha256: String,
}
#[derive(Clone, Serialize)]
pub struct ContextIdentity {
    pub profile: Uuid,
    pub examples: Vec<ExampleIdentity>,
}
pub(crate) struct ConfirmedExample {
    pub identity: ExampleIdentity,
    pub png: Vec<u8>,
    pub original_text: Option<String>,
    pub corrected_text: String,
}
/// Private fields prevent unchecked text/images from impersonating a retained capture.
pub struct ConfirmedContext {
    pub(crate) identity: ContextIdentity,
    pub(crate) examples: Vec<ConfirmedExample>,
    pub(crate) _owner: LocalCorrections,
}
impl ConfirmedContext {
    pub fn identity(&self) -> &ContextIdentity {
        &self.identity
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextFailure {
    Canceled,
    Invalid,
    Stale,
    Conflict,
    Unavailable,
    Storage,
}

pub(crate) const ABSENT_ORIGINAL_INSTRUCTION: &str = "A null original_text means no machine reading exists. The corrected_text is the transcription explicitly supplied by the user for that example image.";
impl ConfirmedContext {
    pub(crate) fn has_absent_original(&self) -> bool {
        self.examples
            .iter()
            .any(|example| example.original_text.is_none())
    }
}

//! Owned, explicitly selected confirmed examples; no implicit search or mutation.
use super::*;

#[derive(Clone, Deserialize)]
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
    pub original_text_sha256: String,
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
    pub original_text: String,
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

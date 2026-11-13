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
impl ConfirmedExample {
    pub(crate) fn from_verified(value: Correction, revision: Uuid, png: Vec<u8>) -> Result<Self> {
        Ok(Self {
            identity: ExampleIdentity {
                correction: value.correction,
                record: value.record_id()?,
                revision,
                sample_sha256: value.sample.sha256.clone(),
                original_text_sha256: value
                    .original_text
                    .as_deref()
                    .map(|text| digest(text.as_bytes())),
                corrected_text_sha256: digest(value.corrected_text.as_bytes()),
            },
            png,
            original_text: value.original_text,
            corrected_text: value.corrected_text,
        })
    }
}

pub(crate) enum ContextOwner {
    Local { _corrections: LocalCorrections },
    Selected(crate::storage::sync::selected::SelectedContextOwner),
}

#[cfg(any(target_os = "linux", test))]
pub(crate) struct SelectedCapture {
    pub(crate) context: ConfirmedContext,
    pub(crate) selection: super::runtime::Selection,
}
/// Private fields prevent unchecked text/images from impersonating a retained capture.
pub struct ConfirmedContext {
    pub(crate) identity: ContextIdentity,
    pub(crate) examples: Vec<ConfirmedExample>,
    pub(crate) owner: ContextOwner,
}
impl ConfirmedContext {
    pub fn identity(&self) -> &ContextIdentity {
        &self.identity
    }

    #[cfg(any(target_os = "linux", test))]
    pub(crate) fn selected_owner(
        &self,
    ) -> anyhow::Result<&crate::storage::sync::selected::SelectedContextOwner> {
        match &self.owner {
            ContextOwner::Selected(owner) => Ok(owner),
            ContextOwner::Local { .. } => anyhow::bail!("selected handwriting context required"),
        }
    }

    pub(crate) fn from_examples(
        profile: Uuid,
        examples: Vec<ConfirmedExample>,
        owner: ContextOwner,
    ) -> Self {
        Self {
            identity: ContextIdentity {
                profile,
                examples: examples
                    .iter()
                    .map(|example| example.identity.clone())
                    .collect(),
            },
            examples,
            owner,
        }
    }

    /// Capture exact historical examples from a registered shared correction.
    /// The selected phase must recheck ownership before using this context.
    pub fn capture_selected(
        store: std::sync::Arc<crate::storage::Store>,
        profile: Uuid,
        requested: &[ExampleSelection],
        targets: &BTreeSet<String>,
        reads: &mut crate::storage::selection::SelectedReadBudget,
    ) -> std::result::Result<Self, ContextFailure> {
        crate::storage::sync::selected::SelectedContextOwner::capture(
            store, profile, requested, targets, reads,
        )
    }

    /// Join existing document, memory or preference gates and revalidate the
    /// selected capture before invoking a synchronous submission or publication.
    pub fn with_selected_phase<T>(
        &self,
        gates: &[&crate::conversation::SelectedAdmission],
        reads: &mut crate::storage::selection::SelectedReadBudget,
        run: impl FnOnce(&crate::storage::Store) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let ContextOwner::Selected(owner) = &self.owner else {
            anyhow::bail!("selected handwriting context required");
        };
        owner.with_phase(gates, reads, run)
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

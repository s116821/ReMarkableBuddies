//! Buddy-owned persisted facts. None of these records authorizes a native mutation.
use crate::storage::{Media, Uuid};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const SCHEMA: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CaptureEvidence {
    pub id: Uuid,
    pub conversation: Uuid,
    pub turn: Uuid,
    pub facts: super::capture_facts::HistoricalCapture,
}
#[derive(Clone, Debug)]
pub struct PreparedSdkImage {
    /// Native parent has no provider ordinal; derivatives keep original order.
    pub ordinal: Option<u32>,
    pub media: Media,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct PreparedSdkCapture {
    pub turn: Uuid,
    pub capture: Uuid,
    pub native_parent: PreparedSdkImage,
    pub images: Vec<PreparedSdkImage>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Reader,
    Writer,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    User,
    Assistant,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Prepared,
    Interpreted,
    Generated,
    Completed,
    Failed,
    Canceled,
    ReconcileRequired,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Root {
    pub id: Uuid,
    pub next_sequence: u64,
    pub binding: Option<Uuid>,
    pub created_ms: u64,
    pub updated_ms: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Turn {
    pub id: Uuid,
    pub conversation: Uuid,
    pub exchange: Uuid,
    pub sequence: u64,
    pub role: Role,
    pub mode: Mode,
    pub outcome: Outcome,
    pub text: Option<String>,
    pub sources: Vec<Uuid>,
    pub correction_of: Option<Uuid>,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub completion: Option<CompletionEvidence>,
}
/// Adapter-reported verification facts, not permission to render or repeat output.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CompletionEvidence {
    pub native_operation: Uuid,
    pub exact_text: String,
    pub observation: SourceObservation,
    pub procedure: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourceObservation {
    pub document: Uuid,
    pub page: Uuid,
    pub session: String,
    pub visit: String,
    pub content_revision: Option<String>,
    pub order_revision: Option<String>,
    pub capability_revision: String,
    /// Records evidence strength; absent qualification is never inferred from IDs.
    pub evidence_procedure: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ImageUse {
    pub id: Uuid,
    pub conversation: Uuid,
    pub turn: Uuid,
    pub image: Media,
    pub observation: SourceObservation,
    pub width: u32,
    pub height: u32,
    pub purpose: String,
    pub ordinal: u32,
    pub parent: Option<Media>,
    pub parent_dimensions: Option<[u32; 2]>,
    /// Crop in parent pixels: x, y, width, height.
    pub crop: Option<[u32; 4]>,
    /// Affine mapping from image pixels into normalized source coordinates.
    pub transform: [f64; 6],
    pub target: Option<[f64; 2]>,
    pub captured_ms: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BindingReceipt {
    pub operation: Uuid,
    pub conversation: Uuid,
    pub request_fingerprint: String,
    pub adapter_fingerprint: String,
    pub source: SourceObservation,
    pub intended_target: Option<Uuid>,
    pub observed_document: Uuid,
    pub observed_page: Uuid,
    pub expected_order: Vec<Uuid>,
    pub observed_order: Vec<Uuid>,
    pub persisted_revision: String,
    pub observed_ms: u64,
    /// Domain fixtures are synthetic; SDK native qualification is a separate gate.
    pub evidence_origin: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub id: Uuid,
    pub conversation: Uuid,
    pub receipt: BindingReceipt,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExportAssociation {
    pub id: Uuid,
    pub conversation: Uuid,
    pub backend: String,
    pub native_note: Option<String>,
    pub source_scope: String,
    pub source_revision: String,
    pub operation: Uuid,
    pub outcome: Outcome,
}
impl ExportAssociation {
    pub fn title_marker(&self) -> String {
        format!("[RMB:{}]", self.id)
    }
    pub const METADATA_KEY: &'static str = "remarkable_buddies_export_id";
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Acknowledgment {
    pub operation: Uuid,
    pub conversation: Uuid,
    pub records: Vec<Uuid>,
    pub applied_root_revision: Uuid,
    pub binding: Option<Uuid>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub fingerprint: String,
    pub acknowledgment: Acknowledgment,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "record_kind",
    content = "record",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum Record {
    Root(Root),
    Turn(Turn),
    Source(ImageUse),
    Capture(Box<CaptureEvidence>),
    Binding(Binding),
    Export(ExportAssociation),
    Receipt(Receipt),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExpectedHeads(pub BTreeSet<Uuid>);
#[derive(Debug, PartialEq, Eq)]
pub struct IncompleteStore {
    pub unavailable_commits: usize,
}
impl std::fmt::Display for IncompleteStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "conversation storage is incomplete: {} unavailable committed transactions",
            self.unavailable_commits
        )
    }
}
impl std::error::Error for IncompleteStore {}
#[derive(Clone)]
pub struct WriteResult {
    pub historical: Acknowledgment,
    /// A historical acknowledgment does not restore a deleted or conflicted root.
    pub current_root: Vec<crate::storage::Envelope>,
    pub current_binding: Vec<crate::storage::Envelope>,
}
#[derive(Clone, Debug)]
pub struct ContextBudget {
    pub max_text_bytes: usize,
    pub max_turns: usize,
    /// Supplied by the provider adapter; None refuses provider context assembly.
    pub provider_token_limit: Option<usize>,
}
#[derive(Clone, Debug)]
pub struct ContextView {
    pub turns: Vec<Turn>,
    pub sources: Vec<ImageUse>,
    pub captures: Vec<CaptureEvidence>,
    pub missing_media: Vec<Media>,
    pub selection: Option<TurnRange>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TurnRange {
    pub start: u64,
    pub end_exclusive: u64,
}
#[derive(Debug, PartialEq, Eq)]
pub enum ContextRefusal {
    UnknownProviderBudget,
    SelectionRequired {
        turns: usize,
        text_bytes: usize,
        tokens: Option<usize>,
    },
}
impl std::fmt::Display for ContextRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownProviderBudget=>f.write_str("provider token bound unknown"),
            Self::SelectionRequired {turns,text_bytes,tokens}=>write!(f,"explicit context selection required: {turns} turns, {text_bytes} text bytes, tokens {tokens:?}"),
        }
    }
}
impl std::error::Error for ContextRefusal {}

#[derive(Clone, Debug)]
pub struct RetainedMedia {
    pub media: Media,
    pub retained_revision_references: usize,
    pub available: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportReconciliation {
    Unique(String),
    Conflict(Vec<String>),
    Uncertain,
}
/// Backend-verified discovery result. No write or create operation is provided.
#[derive(Clone, Debug)]
pub struct ExportMatch {
    pub marker: Uuid,
    pub backend: String,
    pub scope: String,
    pub revision: String,
    pub native_note: String,
}

pub struct ImageInput {
    pub source: ImageUse,
    pub bytes: Vec<u8>,
    pub parent_bytes: Option<Vec<u8>>,
}
#[derive(Clone, Debug)]
pub struct PreparedImages {
    pub turn: Uuid,
    pub images: Vec<(Uuid, Vec<u8>)>,
}

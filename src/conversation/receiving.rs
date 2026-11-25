//! Immutable ordinary historical data, never selected/native or model admission.
pub mod answer;
pub(crate) mod images;
pub mod memory;
use super::{
    resolution::{self, SourceDirective},
    ContextBudget, ContextRefusal, ContextView, Ledger, Mode, Outcome, Record, Role, Turn,
    TurnRange,
};
use crate::{
    retrieval::{IndexKey, Provider, Retrieved},
    storage::{valid_digest, Media, ObjectRef, Uuid},
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fmt,
    io::{self, Write},
};

const FRAME_BYTES: usize = 512 * 1024;
const TEXT_BYTES: usize = 64 * 1024;
const RESULT_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub enum RequestSource {
    /// Explicit caller declaration, not an inferred blank-page observation.
    NoDocument,
    CapturedPage {
        origin_turn: Uuid,
        source_record: Uuid,
    },
    Original {
        origin_turn: Uuid,
        source_record: Uuid,
        expected: IndexKey,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceivingBudget {
    /// Eligible context through the request, including the request itself.
    pub context: ContextBudget,
    pub max_encoded_bytes: usize,
    pub max_passages: usize,
    pub max_passage_bytes: usize,
    pub neighbors: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SourceAssociation {
    NoDocument,
    CapturedPage {
        origin_turn: Uuid,
        source_record: Box<Record>,
    },
    Original {
        origin_turn: Uuid,
        source_record: Box<Record>,
        expected: IndexKey,
    },
}

impl SourceAssociation {
    pub(super) fn source_record(&self) -> Option<&Record> {
        match self {
            Self::NoDocument => None,
            Self::CapturedPage { source_record, .. } | Self::Original { source_record, .. } => {
                Some(source_record)
            }
        }
    }

    pub(super) fn request_source(&self) -> Result<RequestSource> {
        let record_id = |record: &Record| -> Result<Uuid> {
            source_id(record).ok_or_else(|| ReceivingRefusal::OriginalUnavailable.into())
        };
        Ok(match self {
            Self::NoDocument => RequestSource::NoDocument,
            Self::CapturedPage {
                origin_turn,
                source_record,
            } => RequestSource::CapturedPage {
                origin_turn: *origin_turn,
                source_record: record_id(source_record)?,
            },
            Self::Original {
                origin_turn,
                source_record,
                expected,
            } => RequestSource::Original {
                origin_turn: *origin_turn,
                source_record: record_id(source_record)?,
                expected: expected.clone(),
            },
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReceivingRefusal {
    InvalidBudget,
    InvalidRequest,
    InvalidHistory,
    InvalidSelection,
    OriginalUnavailable,
    AssociationMismatch,
    InvalidKey,
    RetrievalUnavailable,
    InvalidRetrieval,
    EncodedLimit,
    CounterUnavailable,
    TokenLimit { tokens: usize, limit: usize },
}
impl fmt::Display for ReceivingRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No request, history, source text, provider error or filesystem path.
        match self {
            Self::InvalidBudget => f.write_str("invalid receiving budget"),
            Self::InvalidRequest => f.write_str("invalid stored Reader request"),
            Self::InvalidHistory => f.write_str("receiving history unavailable or incomplete"),
            Self::InvalidSelection => f.write_str("invalid through-request history selection"),
            Self::OriginalUnavailable => f.write_str("original source identity unavailable"),
            Self::AssociationMismatch => f.write_str("original source association mismatch"),
            Self::InvalidKey => f.write_str("invalid independent source key"),
            Self::RetrievalUnavailable => {
                f.write_str("local source retrieval refused or unavailable")
            }
            Self::InvalidRetrieval => f.write_str("invalid local retrieval result"),
            Self::EncodedLimit => f.write_str("receiving encoded byte limit exhausted"),
            Self::CounterUnavailable => f.write_str("receiving token counter unavailable"),
            Self::TokenLimit { tokens, limit } => {
                write!(f, "receiving token limit exhausted: {tokens}/{limit}")
            }
        }
    }
}
impl std::error::Error for ReceivingRefusal {}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FollowupMedia {
    All,
    BoundPage,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Frame {
    schema: u32,
    scope: String,
    conversation: Uuid,
    request: Turn,
    history: Vec<Turn>,
    selection: Option<TurnRange>,
    source: SourceAssociation,
    // This field is always present. Budget arithmetic replaces exactly `null`.
    retrieval: Option<Retrieved>,
    missing_media: Vec<Media>,
}

/// Only use after the ordinary historical evidence validator has admitted the frame.
pub(super) fn recorded_source(json: &str) -> Result<SourceAssociation> {
    Ok(serde_json::from_str::<Frame>(json)?.source)
}

pub struct FrozenRequestContext {
    frame: Frame,
    media_view: Option<ContextView>,
    encoded: String,
    tokens: usize,
}
impl fmt::Debug for FrozenRequestContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrozenRequestContext")
            .field("frame", &self.frame)
            .field("tokens", &self.tokens)
            .finish_non_exhaustive()
    }
}
impl FrozenRequestContext {
    pub(crate) fn media_view(&self) -> Result<&ContextView> {
        self.media_view
            .as_ref()
            .ok_or_else(|| ReceivingRefusal::InvalidHistory.into())
    }
    pub fn request(&self) -> &Turn {
        &self.frame.request
    }
    pub fn history(&self) -> &[Turn] {
        &self.frame.history
    }
    pub fn selection(&self) -> Option<TurnRange> {
        self.frame.selection
    }
    pub fn source(&self) -> &SourceAssociation {
        &self.frame.source
    }
    pub fn retrieval(&self) -> Option<&Retrieved> {
        self.frame.retrieval.as_ref()
    }
    pub fn missing_media(&self) -> &[Media] {
        &self.frame.missing_media
    }
    pub fn encoded_json(&self) -> &str {
        &self.encoded
    }
    /// Supplied caller count of the complete encoded frame, not a model window.
    pub fn counted_tokens(&self) -> usize {
        self.tokens
    }
}

struct Capped {
    bytes: Vec<u8>,
    limit: usize,
    exhausted: bool,
}
impl Write for Capped {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exhausted = true;
            return Err(io::Error::other("receiving output bound"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub(super) fn encoded(value: &(impl Serialize + ?Sized), limit: usize) -> Result<String> {
    let mut out = Capped {
        bytes: Vec::new(),
        limit,
        exhausted: false,
    };
    if serde_json::to_writer(&mut out, value).is_err() {
        return Err(if out.exhausted {
            ReceivingRefusal::EncodedLimit
        } else {
            ReceivingRefusal::InvalidHistory
        }
        .into());
    }
    String::from_utf8(out.bytes).map_err(|_| ReceivingRefusal::InvalidHistory.into())
}
fn count(text: &str, counter: &impl Fn(&str) -> Result<usize>) -> Result<usize> {
    let count = counter(text).map_err(|_| ReceivingRefusal::CounterUnavailable)?;
    ensure!(count > 0, ReceivingRefusal::CounterUnavailable);
    Ok(count)
}

/// Structural replay only: no Provider and no invented historical tokenizer.
pub(super) fn validate_historical(
    json: &str,
    conversation: Uuid,
    request_turn: Uuid,
    budget: &ReceivingBudget,
    records: &[Record],
) -> Result<Vec<Media>> {
    validate_budget(budget)?;
    let mut frame: Frame = serde_json::from_str(json)?;
    ensure!(
        encoded(&frame, budget.max_encoded_bytes)? == json
            && frame.schema == 1
            && frame.scope == "ordinary-historical"
            && frame.conversation == conversation,
        "invalid historical frame encoding"
    );
    let request = records
        .iter()
        .find_map(|r| match r {
            Record::Turn(t) if t.id == request_turn => Some(t),
            _ => None,
        })
        .ok_or(ReceivingRefusal::InvalidRequest)?;
    ensure!(
        &frame.request == request
            && request.conversation == conversation
            && request.mode == Mode::Reader
            && request.role == Role::User
            && request.outcome == Outcome::Interpreted
            && request
                .text
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty() && s.len() <= budget.context.max_text_bytes),
        ReceivingRefusal::InvalidRequest
    );
    if let Some(range) = frame.selection {
        ensure!(
            range.start <= request.sequence
                && request.sequence.checked_add(1) == Some(range.end_exclusive),
            ReceivingRefusal::InvalidSelection
        );
    }
    let allow_notebook = matches!(
        &frame.source,
        SourceAssociation::Original { source_record, .. }
            if matches!(source_record.as_ref(), Record::ArchivedNormalizedSource(_))
    );
    let choice = frame.source.request_source()?;
    ensure!(
        original(
            records,
            request,
            choice,
            budget.max_encoded_bytes,
            allow_notebook
        )? == frame.source,
        ReceivingRefusal::AssociationMismatch
    );
    let mut view = Ledger::context_records(
        records.to_vec(),
        frame.selection,
        &budget.context,
        None,
        |m| !frame.missing_media.contains(m),
    )?;
    ensure!(
        view.turns.last() == Some(request),
        ReceivingRefusal::InvalidHistory
    );
    view.turns.pop();
    ensure!(
        view.turns == frame.history,
        ReceivingRefusal::InvalidHistory
    );
    let mut source_ids = frame
        .history
        .iter()
        .chain(std::iter::once(request))
        .flat_map(|t| t.sources.iter().copied())
        .collect::<BTreeSet<_>>();
    if let Some(source_record) = frame.source.source_record() {
        source_ids.insert(source_id(source_record).ok_or(ReceivingRefusal::OriginalUnavailable)?);
    }
    let supplied = records
        .iter()
        .filter_map(source_id)
        .collect::<BTreeSet<_>>();
    ensure!(source_ids == supplied, "extra or absent historical source");
    let mut media = Vec::new();
    for record in records.iter().filter(|r| source_id(r).is_some()) {
        // Also verify source ownership against its referenced origin turn.
        let (owner, turn) = match record {
            Record::Source(s) => (s.conversation, s.turn),
            Record::Capture(s) => (s.conversation, s.turn),
            Record::LegacyCapture(s) => (s.conversation, s.turn),
            Record::DevelopmentCapture(s) => (s.conversation, s.turn),
            Record::ResidentCapture(s) => (s.conversation, s.turn),
            Record::ArchivedNormalizedSource(s) => (s.conversation, s.turn),
            _ => unreachable!(),
        };
        ensure!(owner == conversation && records.iter().any(|r| matches!(r, Record::Turn(t) if t.id == turn && t.sources.contains(&source_id(record).unwrap()))), "foreign historical source");
        for m in source_media(record)? {
            if !media.contains(&m) {
                media.push(m);
            }
        }
    }
    if let Some(source_record) = frame.source.source_record() {
        for m in source_media(source_record)? {
            if frame.missing_media.contains(&m) && !view.missing_media.contains(&m) {
                view.missing_media.push(m);
            }
        }
    }
    ensure!(
        view.missing_media == frame.missing_media,
        "contradictory historical missing media"
    );
    let retrieval = frame.retrieval.take();
    let base = encoded(&frame, budget.max_encoded_bytes)?;
    match (&frame.source, retrieval) {
        (SourceAssociation::NoDocument | SourceAssociation::CapturedPage { .. }, None) => {}
        (
            SourceAssociation::Original {
                expected,
                source_record,
                ..
            },
            Some(result),
        ) => {
            let text = request
                .text
                .as_deref()
                .ok_or(ReceivingRefusal::InvalidRequest)?;
            let words = crate::retrieval::terms(text);
            ensure!(
                text.len() <= 2048
                    && !text.contains('\0')
                    && !words.is_empty()
                    && words.len() <= 64,
                ReceivingRefusal::InvalidRequest
            );
            let room = budget
                .max_encoded_bytes
                .checked_sub(base.len())
                .and_then(|n| n.checked_add(4))
                .ok_or(ReceivingRefusal::EncodedLimit)?
                .min(budget.max_passage_bytes);
            ensure!(room >= 512, ReceivingRefusal::EncodedLimit);
            validate_retrieval(&result, expected, budget.max_passages, room)?;
            validate_notebook_locators(source_record, &result)?;
        }
        _ => return Err(ReceivingRefusal::InvalidRetrieval.into()),
    }
    Ok(media)
}
pub(super) fn validate_budget(budget: &ReceivingBudget) -> Result<usize> {
    let tokens = budget
        .context
        .provider_token_limit
        .ok_or(ContextRefusal::UnknownProviderBudget)?;
    ensure!(
        tokens > 0
            && (1..=64).contains(&budget.context.max_turns)
            && (1..=TEXT_BYTES).contains(&budget.context.max_text_bytes)
            && (512..=FRAME_BYTES).contains(&budget.max_encoded_bytes)
            && (1..=16).contains(&budget.max_passages)
            && (512..=RESULT_BYTES).contains(&budget.max_passage_bytes),
        ReceivingRefusal::InvalidBudget
    );
    Ok(tokens)
}
fn validate_key(key: &IndexKey) -> Result<()> {
    ensure!(
        !key.document.is_nil()
            && valid_digest(&key.source_sha256)
            && valid_digest(&key.normalized_sha256)
            && !key.extractor_revision.trim().is_empty()
            && key.extractor_revision.len() <= 128
            && !key.extractor_revision.chars().any(char::is_control),
        ReceivingRefusal::InvalidKey
    );
    Ok(())
}
pub(super) fn source_id(record: &Record) -> Option<Uuid> {
    match record {
        Record::ArchivedNormalizedSource(source) => Some(source.id),
        Record::Source(source) => Some(source.id),
        Record::Capture(source) => Some(source.id),
        Record::LegacyCapture(source) => Some(source.id),
        Record::DevelopmentCapture(source) => Some(source.id),
        Record::ResidentCapture(source) => Some(source.id),
        _ => None,
    }
}
pub(super) fn source_media(record: &Record) -> Result<Vec<Media>> {
    match record {
        Record::ArchivedNormalizedSource(source) => Ok(vec![source.media.clone()]),
        Record::Source(source) => Ok(std::iter::once(source.image.clone())
            .chain(source.parent.clone())
            .collect()),
        Record::Capture(source) => source.facts.media(),
        Record::LegacyCapture(source) => source.media(),
        Record::DevelopmentCapture(source) => source.media(),
        Record::ResidentCapture(source) => source.media(),
        _ => Err(ReceivingRefusal::OriginalUnavailable.into()),
    }
}
pub(super) fn original(
    records: &[Record],
    request: &Turn,
    choice: RequestSource,
    cap: usize,
    allow_notebook: bool,
) -> Result<SourceAssociation> {
    let (origin_turn, source_record, expected) = match choice {
        RequestSource::NoDocument => return Ok(SourceAssociation::NoDocument),
        RequestSource::CapturedPage {
            origin_turn,
            source_record,
        } => (origin_turn, source_record, None),
        RequestSource::Original {
            origin_turn,
            source_record,
            expected,
        } => {
            validate_key(&expected)?;
            (origin_turn, source_record, Some(expected))
        }
    };
    let origin = records
        .iter()
        .find_map(|record| match record {
            Record::Turn(turn) if turn.id == origin_turn => Some(turn),
            _ => None,
        })
        .ok_or(ReceivingRefusal::OriginalUnavailable)?;
    ensure!(
        origin.conversation == request.conversation
            && origin.role == Role::User
            && origin.sequence <= request.sequence
            && origin.sources.contains(&source_record),
        ReceivingRefusal::AssociationMismatch
    );
    let record = records
        .iter()
        .find(|record| source_id(record) == Some(source_record))
        .ok_or(ReceivingRefusal::OriginalUnavailable)?;
    let Some(expected) = expected else {
        let Record::ResidentCapture(capture) = record else {
            return Err(ReceivingRefusal::OriginalUnavailable.into());
        };
        capture.validate()?;
        ensure!(
            capture.conversation == request.conversation && capture.turn == origin.id,
            ReceivingRefusal::AssociationMismatch
        );
        encoded(record, cap)?;
        return Ok(SourceAssociation::CapturedPage {
            origin_turn,
            source_record: Box::new(record.clone()),
        });
    };
    let (conversation, turn, document) = match record {
        Record::Source(source) => (
            source.conversation,
            source.turn,
            source.observation.document,
        ),
        Record::Capture(source) => (
            source.conversation,
            source.turn,
            source.facts.source.document.value(),
        ),
        Record::ResidentCapture(source) => {
            (source.conversation, source.turn, source.before.document)
        }
        Record::ArchivedNormalizedSource(source) if allow_notebook => {
            source.validate()?;
            ensure!(
                source.format == crate::retrieval::SourceFormat::Notebook
                    && source.selection == super::TextScope::SuppliedNotebook
                    && notebook_key_matches(source, &expected),
                ReceivingRefusal::AssociationMismatch
            );
            (source.conversation, source.turn, source.document)
        }
        Record::DevelopmentCapture(source) => {
            let label = &source.expected_binding.document_id;
            let id = Uuid::parse_str(label).map_err(|_| ReceivingRefusal::OriginalUnavailable)?;
            ensure!(
                !id.is_nil() && id.to_string() == *label,
                ReceivingRefusal::OriginalUnavailable
            );
            (source.conversation, source.turn, id)
        }
        _ => return Err(ReceivingRefusal::OriginalUnavailable.into()),
    };
    ensure!(
        conversation == request.conversation && turn == origin.id && document == expected.document,
        ReceivingRefusal::AssociationMismatch
    );
    // Bound output before cloning potentially large caller/Store-owned metadata.
    encoded(record, cap)?;
    Ok(SourceAssociation::Original {
        origin_turn,
        source_record: Box::new(record.clone()),
        expected,
    })
}

fn validate_notebook_locators(source: &Record, result: &Retrieved) -> Result<()> {
    if matches!(source, Record::ArchivedNormalizedSource(_)) {
        ensure!(
            result
                .passages
                .iter()
                .all(|passage| passage.page.is_some() && passage.section.is_none()),
            ReceivingRefusal::InvalidRetrieval
        );
    }
    Ok(())
}

pub(super) fn notebook_key_matches(
    source: &super::ArchivedNormalizedSource,
    key: &IndexKey,
) -> bool {
    key.document == source.document
        && key.format == source.format
        && key.source_sha256 == source.source_sha256
        && key.extractor_revision == source.extractor_revision
        && key.normalized_sha256 == source.units_sha256
}

pub(super) fn validate_retrieval(
    result: &Retrieved,
    key: &IndexKey,
    count: usize,
    bytes: usize,
) -> Result<()> {
    ensure!(
        &result.source == key
            && result.passages.len() <= count
            && (result.passages.is_empty()
                || result.passages.iter().any(|passage| passage.direct_match))
            && (!result.passages.is_empty() || !result.budget_limited),
        ReceivingRefusal::InvalidRetrieval
    );
    let mut seen = BTreeSet::new();
    for passage in &result.passages {
        ensure!(
            passage.unit < 4096
                && passage.range[0] < passage.range[1]
                && passage.range[1] <= 512 * 1024
                && passage.range[1] - passage.range[0] == passage.text.len()
                && !passage.text.is_empty()
                && passage.text.len() <= bytes
                && !passage
                    .text
                    .chars()
                    .any(|ch| ch.is_control() && !matches!(ch, '\t' | '\n' | '\r'))
                && (passage.page.is_some() || passage.section.is_some())
                && passage.page.is_none_or(|page| page > 0)
                && passage
                    .section
                    .as_ref()
                    .is_none_or(|section| !section.trim().is_empty()
                        && section.len() <= 512
                        && !section.chars().any(char::is_control))
                && seen.insert((passage.unit, passage.range)),
            ReceivingRefusal::InvalidRetrieval
        );
        ensure!(
            result
                .passages
                .iter()
                .filter(|other| other.unit == passage.unit)
                .all(|other| other.page == passage.page && other.section == passage.section),
            ReceivingRefusal::InvalidRetrieval
        );
    }
    encoded(result, bytes)?;
    Ok(())
}

impl Ledger {
    /// One ordinary historical snapshot. No Source/Turn publication or dispatch.
    #[allow(clippy::too_many_arguments)]
    pub fn source_request_context(
        &self,
        conversation: Uuid,
        request_turn: Uuid,
        selection: Option<TurnRange>,
        source: RequestSource,
        budget: ReceivingBudget,
        retrieval: Option<&dyn Provider>,
        token_count: impl Fn(&str) -> Result<usize>,
    ) -> Result<FrozenRequestContext> {
        validate_budget(&budget)?;
        let records = self
            .inspect(conversation, false)
            .map_err(|_| ReceivingRefusal::InvalidHistory)?;
        self.assemble_request_context(
            conversation,
            request_turn,
            selection,
            source,
            budget,
            retrieval,
            token_count,
            records,
        )
    }
    /// Resolve a declaration inside the same validated records used for assembly.
    /// The returned frame is historical, not permission or unchanged current state.
    #[allow(clippy::too_many_arguments)]
    pub fn source_request_context_with_directive(
        &self,
        conversation: Uuid,
        request_turn: Uuid,
        selection: Option<TurnRange>,
        directive: SourceDirective,
        budget: ReceivingBudget,
        retrieval: Option<&dyn Provider>,
        token_count: impl Fn(&str) -> Result<usize>,
    ) -> Result<FrozenRequestContext> {
        validate_budget(&budget)?;
        let records = self
            .inspect(conversation, false)
            .map_err(|_| ReceivingRefusal::InvalidHistory)?;
        let source = resolution::for_frame(
            &records,
            conversation,
            request_turn,
            directive,
            false,
            false,
        )?;
        self.assemble_request_context(
            conversation,
            request_turn,
            selection,
            source,
            budget,
            retrieval,
            token_count,
            records,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn assemble_request_context(
        &self,
        conversation: Uuid,
        request_turn: Uuid,
        selection: Option<TurnRange>,
        source: RequestSource,
        budget: ReceivingBudget,
        retrieval: Option<&dyn Provider>,
        token_count: impl Fn(&str) -> Result<usize>,
        records: Vec<Record>,
    ) -> Result<FrozenRequestContext> {
        self.assemble_ordinary_request_context(
            conversation,
            request_turn,
            selection,
            source,
            budget,
            retrieval,
            token_count,
            records,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn assemble_ordinary_request_context(
        &self,
        conversation: Uuid,
        request_turn: Uuid,
        selection: Option<TurnRange>,
        source: RequestSource,
        budget: ReceivingBudget,
        retrieval: Option<&dyn Provider>,
        token_count: impl Fn(&str) -> Result<usize>,
        records: Vec<Record>,
        followup_media: Option<FollowupMedia>,
        verified_notebook: Option<&Media>,
    ) -> Result<FrozenRequestContext> {
        let bound_page = followup_media == Some(FollowupMedia::BoundPage);
        let bound_media = if bound_page {
            match &source {
                RequestSource::NoDocument => Vec::new(),
                RequestSource::CapturedPage { source_record, .. } => {
                    let record = records
                        .iter()
                        .find(|record| source_id(record) == Some(*source_record))
                        .ok_or(ReceivingRefusal::OriginalUnavailable)?;
                    source_media(record)?
                }
                RequestSource::Original { .. } => {
                    return Err(ReceivingRefusal::AssociationMismatch.into());
                }
            }
        } else {
            Vec::new()
        };
        let mut context = Self::assemble_request_data(
            conversation,
            request_turn,
            selection,
            source,
            budget,
            retrieval,
            token_count,
            records,
            "ordinary-historical",
            followup_media == Some(FollowupMedia::All),
            verified_notebook.is_some(),
            |media| {
                // Keep the existing historical frame for replay. Its
                // missing-media list records failures only within the
                // media this answer actually needs to open.
                (bound_page && !bound_media.contains(media))
                    || verified_notebook == Some(media)
                    || self
                        .store
                        .open_object(&ObjectRef {
                            sha256: media.sha256.clone(),
                            bytes: media.bytes,
                        })
                        .is_ok()
            },
        )?;
        if let Some(view) = context.media_view.as_mut() {
            // The target uses are frozen with the frame. Resolve only their
            // exact historical objects, as the ordinary image path does.
            self.complete_target_view(view)?;
        }
        Ok(context)
    }

    /// Only the live Attempt can construct this admitted data view. It carries
    /// no authority to submit a model request or publish a result.
    #[cfg(target_os = "linux")]
    pub(crate) fn selected_researched_context(
        admitted: &crate::workflow::ResearchContext,
        budget: ReceivingBudget,
        retrieval: &dyn Provider,
        token_count: impl Fn(&str) -> Result<usize>,
    ) -> Result<FrozenRequestContext> {
        Self::assemble_request_data(
            admitted.conversation(),
            admitted.request_turn(),
            None,
            RequestSource::Original {
                origin_turn: admitted.request_turn(),
                source_record: admitted.capture(),
                expected: admitted.index_key().clone(),
            },
            budget,
            Some(retrieval),
            token_count,
            admitted.records().to_vec(),
            "selected-historical",
            false,
            false,
            |media| admitted.media_available(media),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn assemble_request_data(
        conversation: Uuid,
        request_turn: Uuid,
        selection: Option<TurnRange>,
        source: RequestSource,
        budget: ReceivingBudget,
        retrieval: Option<&dyn Provider>,
        token_count: impl Fn(&str) -> Result<usize>,
        records: Vec<Record>,
        scope: &str,
        keep_media: bool,
        allow_notebook: bool,
        media_available: impl Fn(&Media) -> bool,
    ) -> Result<FrozenRequestContext> {
        let token_limit = validate_budget(&budget)?;
        let request = records
            .iter()
            .find_map(|record| match record {
                Record::Turn(turn) if turn.id == request_turn => Some(turn),
                _ => None,
            })
            .ok_or(ReceivingRefusal::InvalidRequest)?;
        ensure!(
            request.conversation == conversation
                && request.mode == Mode::Reader
                && request.role == Role::User
                && request.outcome == Outcome::Interpreted
                && request
                    .text
                    .as_ref()
                    .is_some_and(|text| !text.trim().is_empty()
                        && text.len() <= budget.context.max_text_bytes),
            ReceivingRefusal::InvalidRequest
        );
        if let Some(range) = selection {
            ensure!(
                range.start <= request.sequence
                    && request.sequence.checked_add(1) == Some(range.end_exclusive),
                ReceivingRefusal::InvalidSelection
            );
        }
        let association = original(
            &records,
            request,
            source,
            budget.max_encoded_bytes,
            allow_notebook,
        )?;
        if matches!(association, SourceAssociation::Original { .. }) {
            let text = request
                .text
                .as_ref()
                .ok_or(ReceivingRefusal::InvalidRequest)?;
            ensure!(
                text.len() <= 2048 && !text.contains('\0'),
                ReceivingRefusal::InvalidRequest
            );
            let words = crate::retrieval::terms(text);
            ensure!(
                !words.is_empty() && words.len() <= 64,
                ReceivingRefusal::InvalidRequest
            );
            ensure!(retrieval.is_some(), ReceivingRefusal::RetrievalUnavailable);
        }
        let request = request.clone();
        let through = records
            .into_iter()
            .filter(
                |record| !matches!(record, Record::Turn(turn) if turn.sequence > request.sequence),
            )
            .collect();
        let mut view = Self::context_from_records(
            through,
            selection,
            &budget.context,
            |turns| count(&encoded(turns, budget.max_encoded_bytes)?, &token_count),
            &media_available,
        )?;
        ensure!(
            view.turns.last().is_some_and(|turn| turn == &request),
            ReceivingRefusal::InvalidHistory
        );
        view.turns.pop();
        if let Some(source_record) = association.source_record() {
            if let Record::ArchivedNormalizedSource(archive) = source_record {
                if !view
                    .archived_sources
                    .iter()
                    .any(|source| source.id == archive.id)
                {
                    view.archived_sources.push((**archive).clone());
                }
            }
            for media in source_media(source_record)? {
                if !media_available(&media) && !view.missing_media.contains(&media) {
                    view.missing_media.push(media);
                }
            }
        }
        let mut frame = Frame {
            schema: 1,
            scope: scope.into(),
            conversation,
            request,
            history: if keep_media {
                view.turns.clone()
            } else {
                std::mem::take(&mut view.turns)
            },
            selection,
            source: association,
            retrieval: None,
            missing_media: if keep_media {
                view.missing_media.clone()
            } else {
                std::mem::take(&mut view.missing_media)
            },
        };
        let base = encoded(&frame, budget.max_encoded_bytes)?;
        let serialized = if let SourceAssociation::Original {
            expected,
            source_record,
            ..
        } = &frame.source
        {
            let room = budget
                .max_encoded_bytes
                .checked_sub(base.len())
                .and_then(|room| room.checked_add(4))
                .ok_or(ReceivingRefusal::EncodedLimit)?;
            let room = room.min(budget.max_passage_bytes);
            ensure!(room >= 512, ReceivingRefusal::EncodedLimit);
            let result = retrieval
                .ok_or(ReceivingRefusal::RetrievalUnavailable)?
                .query(
                    expected,
                    frame
                        .request
                        .text
                        .as_deref()
                        .ok_or(ReceivingRefusal::InvalidRequest)?,
                    budget.max_passages,
                    room,
                    budget.neighbors,
                )
                .map_err(|_| ReceivingRefusal::RetrievalUnavailable)?;
            validate_retrieval(&result, expected, budget.max_passages, room)?;
            validate_notebook_locators(source_record, &result)?;
            frame.retrieval = Some(result);
            encoded(&frame, budget.max_encoded_bytes)?
        } else {
            base
        };
        let tokens = count(&serialized, &token_count)?;
        ensure!(
            tokens <= token_limit,
            ReceivingRefusal::TokenLimit {
                tokens,
                limit: token_limit
            }
        );
        Ok(FrozenRequestContext {
            frame,
            media_view: keep_media.then_some(view),
            encoded: serialized,
            tokens,
        })
    }
}

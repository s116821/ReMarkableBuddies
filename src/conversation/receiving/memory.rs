//! Transient ordinary historical data composition, without model/effect admission.
pub mod research;
use super::{encoded, Frame, FrozenRequestContext, SourceAssociation, FRAME_BYTES};
use crate::{memory, storage::Uuid};
use anyhow::{ensure, Result};
use serde::Serialize;
use std::{collections::BTreeSet, fmt};

#[derive(Clone, Debug)]
pub struct CombinedBudget {
    pub max_encoded_bytes: usize,
    pub max_tokens: usize,
}

#[derive(Clone, Debug)]
pub struct MemoryOptions {
    pub now_ms: u64,
    pub current_claims: Vec<String>,
    pub max_matches: usize,
    pub max_context_bytes: usize,
}

pub enum MemoryInput<'a> {
    Disabled,
    Query {
        provider: &'a dyn memory::Provider,
        profile: Uuid,
        options: MemoryOptions,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum CompositionRefusal {
    InvalidBudget,
    InvalidMemoryOptions,
    IncompatibleRequest,
    MemoryUnavailable,
    SelectedMemory,
    InvalidMemory,
    EncodedLimit,
    CounterUnavailable,
    TokenLimit { tokens: usize, limit: usize },
}
impl fmt::Display for CompositionRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBudget => f.write_str("invalid combined context budget"),
            Self::InvalidMemoryOptions => f.write_str("invalid ordinary memory options"),
            Self::IncompatibleRequest => f.write_str("request incompatible with memory query"),
            Self::MemoryUnavailable => f.write_str("ordinary memory query unavailable"),
            Self::SelectedMemory => {
                f.write_str("selected memory cannot enter ordinary composition")
            }
            Self::InvalidMemory => f.write_str("invalid ordinary memory result"),
            Self::EncodedLimit => f.write_str("combined context encoded limit exhausted"),
            Self::CounterUnavailable => f.write_str("combined context counter unavailable"),
            Self::TokenLimit { tokens, limit } => {
                write!(
                    f,
                    "combined context token limit exhausted: {tokens}/{limit}"
                )
            }
        }
    }
}
impl std::error::Error for CompositionRefusal {}

/// An observation, not verified source evidence or admission for later consumption.
#[derive(Debug, Serialize)]
pub struct MemoryReport {
    profile: Option<Uuid>,
    query: Option<memory::Query>,
    status: memory::Status,
    diagnostics: memory::Diagnostics,
    context: Option<memory::ContextData>,
}
impl MemoryReport {
    pub fn profile(&self) -> Option<Uuid> {
        self.profile
    }
    pub fn query(&self) -> Option<&memory::Query> {
        self.query.as_ref()
    }
    pub fn status(&self) -> memory::Status {
        self.status
    }
    pub fn diagnostics(&self) -> &memory::Diagnostics {
        &self.diagnostics
    }
    pub fn context(&self) -> Option<&memory::ContextData> {
        self.context.as_ref()
    }
}

#[derive(Serialize)]
struct CombinedFrame<'a> {
    schema: u32,
    scope: &'static str,
    request_context: &'a Frame,
    memory: Option<&'a MemoryReport>,
}

#[derive(Debug)]
pub struct FrozenSourceMemoryContext {
    source: FrozenRequestContext,
    memory: MemoryReport,
    encoded: String,
    tokens: usize,
}
impl FrozenSourceMemoryContext {
    pub fn source_context(&self) -> &FrozenRequestContext {
        &self.source
    }
    pub fn memory(&self) -> &MemoryReport {
        &self.memory
    }
    pub fn encoded_json(&self) -> &str {
        &self.encoded
    }
    /// Caller count of the complete wrapper, not a model window or token sum.
    pub fn counted_tokens(&self) -> usize {
        self.tokens
    }
}

fn validate_result(
    result: &memory::QueryResult,
    profile: Uuid,
    query: &memory::Query,
) -> Result<()> {
    use memory::Status;
    ensure!(!result.selected(), CompositionRefusal::SelectedMemory);
    ensure!(
        result.status != Status::Unavailable && !result.diagnostics.snapshot_unavailable,
        CompositionRefusal::MemoryUnavailable
    );
    let incomplete = result.diagnostics.conflicted_keys > 0
        || result.diagnostics.contradicted_claims > 0
        || result.diagnostics.unsupported > 0
        || result.diagnostics.budget_omitted > 0;
    let facts = &result.context.facts;
    // Validate bounded existing domain values before any returned-data clone/encoding.
    ensure!(
        facts.len() <= query.max_matches
            && result.context.kind == "associative-memory-data"
            && result.context.provenance_authority == "author-asserted-not-source-verified"
            && match result.status {
                Status::CompleteEmpty => facts.is_empty() && !incomplete,
                Status::Complete => !facts.is_empty() && !incomplete,
                Status::Partial => incomplete,
                _ => false,
            },
        CompositionRefusal::InvalidMemory
    );
    let mut records = BTreeSet::new();
    let mut claims = BTreeSet::new();
    for item in facts {
        item.fact
            .validate()
            .map_err(|_| CompositionRefusal::InvalidMemory)?;
        ensure!(
            !item.revision.is_nil()
                && item.fact.key.record_id().ok() == Some(item.record)
                && item.fact.key.profile == profile
                && match item.fact.key.scope {
                    memory::Scope::Global => true,
                    memory::Scope::Document { document } => Some(document) == query.document,
                }
                && item
                    .fact
                    .expires_ms
                    .is_none_or(|expiry| expiry > query.now_ms)
                && !query.current_claims.contains(&item.fact.claim)
                && records.insert(item.record)
                && claims.insert(&item.fact.claim),
            CompositionRefusal::InvalidMemory
        );
    }
    // Counts and individual string bounds above make this serialization bounded.
    ensure!(
        result
            .encoded()
            .map_err(|_| CompositionRefusal::InvalidMemory)?
            .len()
            <= query.max_context_bytes,
        CompositionRefusal::InvalidMemory
    );
    Ok(())
}

impl FrozenRequestContext {
    /// Consumes captured data only. No Ledger read, source query, write or dispatch.
    pub fn with_memory(
        self,
        input: MemoryInput<'_>,
        budget: CombinedBudget,
        token_count: impl Fn(&str) -> Result<usize>,
    ) -> Result<FrozenSourceMemoryContext> {
        ensure!(
            (512..=FRAME_BYTES).contains(&budget.max_encoded_bytes) && budget.max_tokens > 0,
            CompositionRefusal::InvalidBudget
        );
        let mut frame = CombinedFrame {
            schema: 1,
            scope: "ordinary-historical",
            request_context: &self.frame,
            memory: None,
        };
        let query_input = match input {
            MemoryInput::Disabled => None,
            MemoryInput::Query {
                provider,
                profile,
                options,
            } => {
                ensure!(!profile.is_nil(), CompositionRefusal::InvalidMemoryOptions);
                let text = self.request().text.as_deref().unwrap_or_default();
                // Preserve source-only support for larger NoDocument requests.
                ensure!(
                    !text.trim().is_empty() && text.len() <= 2048 && !text.contains('\0'),
                    CompositionRefusal::IncompatibleRequest
                );
                let query = memory::Query {
                    text: text.to_owned(),
                    document: match self.source() {
                        SourceAssociation::NoDocument => None,
                        SourceAssociation::Original { expected, .. } => Some(expected.document),
                    },
                    now_ms: options.now_ms,
                    current_claims: options.current_claims,
                    max_matches: options.max_matches,
                    max_context_bytes: options.max_context_bytes,
                };
                query
                    .validate()
                    .map_err(|_| CompositionRefusal::InvalidMemoryOptions)?;
                Some((provider, profile, query))
            }
        };
        // null is strictly smaller than either complete report. Token counts do not
        // have this monotonic property, so only count the actual final encoding.
        encoded(&frame, budget.max_encoded_bytes).map_err(|_| CompositionRefusal::EncodedLimit)?;
        let report = if let Some((provider, profile, query)) = query_input {
            let result = provider
                .query(&query)
                .map_err(|_| CompositionRefusal::MemoryUnavailable)?;
            validate_result(&result, profile, &query)?;
            MemoryReport {
                profile: Some(profile),
                query: Some(query),
                status: result.status,
                diagnostics: result.diagnostics,
                context: Some(result.context),
            }
        } else {
            MemoryReport {
                profile: None,
                query: None,
                status: memory::Status::Disabled,
                diagnostics: memory::Diagnostics::default(),
                context: None,
            }
        };
        frame.memory = Some(&report);
        let encoded = encoded(&frame, budget.max_encoded_bytes)
            .map_err(|_| CompositionRefusal::EncodedLimit)?;
        let tokens = token_count(&encoded).map_err(|_| CompositionRefusal::CounterUnavailable)?;
        ensure!(tokens > 0, CompositionRefusal::CounterUnavailable);
        ensure!(
            tokens <= budget.max_tokens,
            CompositionRefusal::TokenLimit {
                tokens,
                limit: budget.max_tokens,
            }
        );
        Ok(FrozenSourceMemoryContext {
            source: self,
            memory: report,
            encoded,
            tokens,
        })
    }
}

//! Declared historical research composition; acquisition is explicitly selected.
pub mod openai;
use super::{CombinedBudget, CombinedFrame, FrozenSourceMemoryContext};
use crate::{
    conversation::receiving::encoded,
    storage::{digest, Uuid},
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResearchPolicy {
    #[default]
    Required,
    Disabled,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResearchDirective {
    #[default]
    UsePolicy,
    NoInternet,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Disabled,
    CompleteEmpty,
    Complete,
    Partial,
    Unavailable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DisabledReason {
    RequestNoInternet,
    ConfiguredDisabled,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceBasis {
    Synthetic,
    ProviderAsserted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Diagnostic {
    BudgetLimited,
    SourceUnavailable,
    ProviderFailure,
}

#[derive(Clone, Copy, Debug)]
pub struct ResearchOptions {
    pub now_ms: u64,
    pub max_queries: usize,
    pub max_sources: usize,
    pub max_evidence_bytes: usize,
}
pub struct ResearchInput<'a> {
    pub policy: ResearchPolicy,
    pub directive: ResearchDirective,
    pub provider: Option<&'a dyn Provider>,
    pub options: ResearchOptions,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchIdentity {
    pub conversation: Uuid,
    pub request_turn: Uuid,
    pub context_sha256: String,
    pub observed_ms: u64,
}
pub struct ResearchRequest<'a> {
    pub identity: ResearchIdentity,
    pub context: &'a FrozenSourceMemoryContext,
    pub options: ResearchOptions,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Citation {
    pub url: String,
    pub title: String,
    pub excerpt: String,
    pub published_ms: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchResult {
    pub identity: ResearchIdentity,
    pub status: Status,
    pub basis: EvidenceBasis,
    pub diagnostic: Option<Diagnostic>,
    pub queries: Vec<String>,
    pub sources: Vec<Citation>,
}
/// Implementations own acquisition. This synchronous seam adds no timeout or client.
pub trait Provider {
    fn research(&self, request: &ResearchRequest<'_>) -> Result<ResearchResult>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResearchRefusal {
    InvalidBudget,
    InvalidOptions,
    Unavailable,
    InvalidEvidence,
    EncodedLimit,
    CounterUnavailable,
    TokenLimit { tokens: usize, limit: usize },
}
impl fmt::Display for ResearchRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBudget => f.write_str("invalid research context budget"),
            Self::InvalidOptions => f.write_str("invalid declared research options"),
            Self::Unavailable => f.write_str("declared research unavailable"),
            Self::InvalidEvidence => f.write_str("invalid declared research evidence"),
            Self::EncodedLimit => f.write_str("research context encoded limit exhausted"),
            Self::CounterUnavailable => f.write_str("research context counter unavailable"),
            Self::TokenLimit { tokens, limit } => write!(
                f,
                "research context token limit exhausted: {tokens}/{limit}"
            ),
        }
    }
}
impl std::error::Error for ResearchRefusal {}

#[derive(Debug, Serialize)]
pub struct ResearchReport {
    policy: ResearchPolicy,
    directive: ResearchDirective,
    status: Status,
    disabled_reason: Option<DisabledReason>,
    identity: Option<ResearchIdentity>,
    basis: Option<EvidenceBasis>,
    diagnostic: Option<Diagnostic>,
    queries: Vec<String>,
    sources: Vec<Citation>,
    authority: &'static str,
}
impl ResearchReport {
    pub fn policy(&self) -> ResearchPolicy {
        self.policy
    }
    pub fn directive(&self) -> ResearchDirective {
        self.directive
    }
    pub fn status(&self) -> Status {
        self.status
    }
    pub fn disabled_reason(&self) -> Option<DisabledReason> {
        self.disabled_reason
    }
    pub fn identity(&self) -> Option<&ResearchIdentity> {
        self.identity.as_ref()
    }
    pub fn basis(&self) -> Option<EvidenceBasis> {
        self.basis
    }
    pub fn diagnostic(&self) -> Option<Diagnostic> {
        self.diagnostic
    }
    pub fn queries(&self) -> &[String] {
        &self.queries
    }
    pub fn sources(&self) -> &[Citation] {
        &self.sources
    }
}
#[derive(Serialize)]
struct ResearchFrame<'a> {
    schema: u32,
    scope: &'static str,
    context: CombinedFrame<'a>,
    research: Option<&'a ResearchReport>,
}
#[derive(Debug)]
pub struct FrozenResearchContext {
    prior: FrozenSourceMemoryContext,
    research: ResearchReport,
    encoded: String,
    tokens: usize,
}
impl FrozenResearchContext {
    pub fn source_memory_context(&self) -> &FrozenSourceMemoryContext {
        &self.prior
    }
    pub fn research(&self) -> &ResearchReport {
        &self.research
    }
    pub fn encoded_json(&self) -> &str {
        &self.encoded
    }
    /// One caller count of the whole output; not a model window or component sum.
    pub fn counted_tokens(&self) -> usize {
        self.tokens
    }
}

fn text(value: &str, max: usize) -> bool {
    value.len() <= max
        && !value.trim().is_empty()
        && !value
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\r' | '\n' | '\t'))
}
fn validate(
    result: &ResearchResult,
    identity: &ResearchIdentity,
    options: ResearchOptions,
) -> Result<()> {
    ensure!(
        result.status != Status::Unavailable,
        ResearchRefusal::Unavailable
    );
    ensure!(
        &result.identity == identity
            && !result.queries.is_empty()
            && result.queries.len() <= options.max_queries
            && result.sources.len() <= options.max_sources
            && match result.status {
                Status::CompleteEmpty => result.sources.is_empty() && result.diagnostic.is_none(),
                Status::Complete => !result.sources.is_empty() && result.diagnostic.is_none(),
                Status::Partial => result.diagnostic.is_some(),
                _ => false,
            },
        ResearchRefusal::InvalidEvidence
    );
    let mut queries = BTreeSet::new();
    for query in &result.queries {
        ensure!(
            text(query, 2048) && queries.insert(query),
            ResearchRefusal::InvalidEvidence
        );
    }
    let mut urls = BTreeSet::new();
    for source in &result.sources {
        ensure!(
            text(&source.title, 512)
                && text(&source.excerpt, 4096)
                && source.url.len() <= 2048
                && !source.url.chars().any(char::is_whitespace)
                && !source.url.chars().any(char::is_control)
                && source
                    .published_ms
                    .is_none_or(|t| t > 0 && t <= identity.observed_ms),
            ResearchRefusal::InvalidEvidence
        );
        let url = reqwest::Url::parse(&source.url).map_err(|_| ResearchRefusal::InvalidEvidence)?;
        ensure!(
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none()
                && urls.insert(url.to_string()),
            ResearchRefusal::InvalidEvidence
        );
    }
    Ok(())
}

impl FrozenSourceMemoryContext {
    /// Consumes only captured ordinary data. Does not resolve installed policy.
    pub fn with_research(
        self,
        input: ResearchInput<'_>,
        budget: CombinedBudget,
        token_count: impl Fn(&str) -> Result<usize>,
    ) -> Result<FrozenResearchContext> {
        ensure!(
            (512..=524288).contains(&budget.max_encoded_bytes) && budget.max_tokens > 0,
            ResearchRefusal::InvalidBudget
        );
        let disabled_reason = if input.directive == ResearchDirective::NoInternet {
            Some(DisabledReason::RequestNoInternet)
        } else if input.policy == ResearchPolicy::Disabled {
            Some(DisabledReason::ConfiguredDisabled)
        } else {
            None
        };
        if disabled_reason.is_none() {
            ensure!(
                input.options.now_ms > 0
                    && (1..=4).contains(&input.options.max_queries)
                    && (1..=8).contains(&input.options.max_sources)
                    && (256..=16384).contains(&input.options.max_evidence_bytes),
                ResearchRefusal::InvalidOptions
            );
        }
        let mut frame = ResearchFrame {
            schema: 1,
            scope: "ordinary-historical",
            context: CombinedFrame {
                schema: 1,
                scope: "ordinary-historical",
                request_context: &self.source.frame,
                memory: Some(&self.memory),
            },
            research: None,
        };
        // Byte lower bound only. A counter need not be monotonic on nested frames.
        encoded(&frame, budget.max_encoded_bytes).map_err(|_| ResearchRefusal::EncodedLimit)?;
        let mut report = ResearchReport {
            policy: input.policy,
            directive: input.directive,
            status: Status::Disabled,
            disabled_reason,
            identity: None,
            basis: None,
            diagnostic: None,
            queries: vec![],
            sources: vec![],
            authority: "provider-asserted-unverified",
        };
        if disabled_reason.is_none() {
            let provider = input.provider.ok_or(ResearchRefusal::Unavailable)?;
            let request = ResearchRequest {
                identity: ResearchIdentity {
                    conversation: self.source.request().conversation,
                    request_turn: self.source.request().id,
                    context_sha256: digest(self.encoded_json().as_bytes()),
                    observed_ms: input.options.now_ms,
                },
                context: &self,
                options: input.options,
            };
            let result = provider
                .research(&request)
                .map_err(|_| ResearchRefusal::Unavailable)?;
            validate(&result, &request.identity, input.options)?;
            report.status = result.status;
            report.identity = Some(result.identity);
            report.basis = Some(result.basis);
            report.diagnostic = result.diagnostic;
            report.queries = result.queries;
            report.sources = result.sources;
            encoded(&report, input.options.max_evidence_bytes)
                .map_err(|_| ResearchRefusal::InvalidEvidence)?;
        }
        frame.research = Some(&report);
        let encoded =
            encoded(&frame, budget.max_encoded_bytes).map_err(|_| ResearchRefusal::EncodedLimit)?;
        let tokens = token_count(&encoded).map_err(|_| ResearchRefusal::CounterUnavailable)?;
        ensure!(tokens > 0, ResearchRefusal::CounterUnavailable);
        ensure!(
            tokens <= budget.max_tokens,
            ResearchRefusal::TokenLimit {
                tokens,
                limit: budget.max_tokens
            }
        );
        Ok(FrozenResearchContext {
            prior: self,
            research: report,
            encoded,
            tokens,
        })
    }
}

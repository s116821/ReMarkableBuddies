//! Explicit prepared-content exports. Historical retries never dispatch again.
pub mod command;
pub mod mem;
mod run;
use crate::{conversation::ExportAssociation, storage::Uuid};
pub use run::{export, reconcile};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    future::Future,
    time::{Duration, Instant},
};

pub const MAX_CONTENT: usize = 200_000;
pub const MAX_INPUT: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CorrelationLocation {
    Metadata,
    Title,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Failure {
    InvalidInput,
    InvalidResponse,
    Transport,
    Deadline,
    Storage,
    Conflict,
    MissingCredentials,
    Authentication,
    RateLimited,
    Unsupported,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "invalid prepared export input",
            Self::InvalidResponse => "destination response could not be verified",
            Self::Transport => "destination request failed",
            Self::Deadline => "export deadline exceeded",
            Self::Storage => "export state could not be committed",
            Self::Conflict => "export identity or remote content conflict",
            Self::MissingCredentials => "Mem credential is unavailable",
            Self::Authentication => "destination authentication was refused",
            Self::RateLimited => "destination request limit was reached",
            Self::Unsupported => "destination operation is unsupported",
        })
    }
}
impl std::error::Error for Failure {}
pub type Result<T> = std::result::Result<T, Failure>;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedNote {
    pub conversation: Uuid,
    pub correlation: Uuid,
    pub effect_operation: Uuid,
    pub source_scope: String,
    pub source_revision: String,
    pub title: String,
    pub markdown: String,
}

impl PreparedNote {
    pub fn render(&self, location: CorrelationLocation) -> Result<String> {
        if self.conversation.is_nil()
            || self.correlation.is_nil()
            || self.effect_operation.is_nil()
            || self.conversation == self.correlation
            || self.title.trim().is_empty()
            || self.title.len() > 256
            || self.title.chars().any(char::is_control)
            || self.title.contains("[RMB:")
            || !(1..=1024).contains(&self.source_scope.len())
            || !(1..=1024).contains(&self.source_revision.len())
            || self.markdown.len() > MAX_CONTENT
        {
            return Err(Failure::InvalidInput);
        }
        let marker = if location == CorrelationLocation::Title {
            format!(" [RMB:{}]", self.correlation)
        } else {
            String::new()
        };
        let body = format!("# {}{marker}\n\n{}", self.title, self.markdown);
        if body.len() > MAX_CONTENT {
            return Err(Failure::InvalidInput);
        }
        Ok(body)
    }
}

#[derive(Clone)]
pub struct RemoteNote {
    pub id: String,
    pub title: String,
    pub content: String,
    pub version: u64,
    pub trashed: bool,
    pub metadata: BTreeMap<String, String>,
}
pub enum Discovery {
    Missing,
    Unique(RemoteNote),
    Duplicate,
    Incomplete,
}

/// A backend must not retry writes or send while constructing its write future.
/// Only polling may dispatch. The exporter owns Store and cancellation policy.
pub trait NoteDestination: Sync {
    fn backend(&self) -> &str;
    fn correlation_location(&self) -> CorrelationLocation;
    fn discover(
        &self,
        correlation: Uuid,
        budget: Budget,
    ) -> impl Future<Output = Result<Discovery>> + Send;
    fn read(
        &self,
        id: &str,
        budget: Budget,
    ) -> impl Future<Output = Result<Option<RemoteNote>>> + Send;
    fn create(
        &self,
        id: &str,
        content: &str,
        correlation: Uuid,
        budget: Budget,
    ) -> impl Future<Output = Result<RemoteNote>> + Send;
    fn update(
        &self,
        id: &str,
        content: &str,
        version: u64,
        correlation: Uuid,
        budget: Budget,
    ) -> impl Future<Output = Result<RemoteNote>> + Send;
}

#[derive(Clone, Copy)]
pub struct Budget {
    end: Instant,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            end: Instant::now() + Duration::from_secs(45),
        }
    }
}
impl Budget {
    pub fn remaining(self) -> Result<Duration> {
        self.end
            .checked_duration_since(Instant::now())
            .filter(|v| !v.is_zero())
            .ok_or(Failure::Deadline)
    }
    pub fn request_timeout(self) -> Result<Duration> {
        Ok(self.remaining()?.min(Duration::from_secs(10)))
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ExportStatus {
    VerifiedCurrent,
    HistoricalCompleted,
    CanceledNoEffect,
    Busy,
    Conflict,
    Incomplete,
    ReconcileRequired,
    Refused,
}
#[derive(Clone, Debug, Serialize)]
pub struct ExportReport {
    pub status: ExportStatus,
    pub correlation: Uuid,
    pub effect_operation: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
}
impl ExportReport {
    fn new(status: ExportStatus, correlation: Uuid, effect_operation: Uuid) -> Self {
        Self {
            status,
            correlation,
            effect_operation,
            native_note: None,
            failure: None,
        }
    }
}

pub(crate) fn title_match(title: &str, correlation: Uuid) -> Result<bool> {
    let marker = format!("[RMB:{correlation}]");
    if !title.contains(&marker) {
        return Ok(false);
    }
    let before = title
        .trim_end()
        .strip_suffix(&marker)
        .ok_or(Failure::Conflict)?;
    if title.matches("[RMB:").count() != 1 || !before.ends_with(char::is_whitespace) {
        return Err(Failure::Conflict);
    }
    Ok(true)
}
fn verify_note(
    note: &RemoteNote,
    a: &ExportAssociation,
    location: CorrelationLocation,
) -> Result<()> {
    if note.trashed
        || note.version == 0
        || note.id.is_empty()
        || note.id.len() > 1024
        || note.content.len() > MAX_CONTENT
    {
        return Err(Failure::Conflict);
    }
    let matches = match location {
        CorrelationLocation::Title => title_match(&note.title, a.id)?,
        CorrelationLocation::Metadata => note
            .metadata
            .get(ExportAssociation::METADATA_KEY)
            .is_some_and(|value| value == &a.id.to_string()),
    };
    if !matches {
        return Err(Failure::Conflict);
    }
    Ok(())
}

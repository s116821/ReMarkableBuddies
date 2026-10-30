//! Explicit unverified historical-page transcription. Never confirms or commits.
pub mod command;
use crate::{
    config::Config,
    handwriting::{self, ConfirmedContext, ContextFailure, ExampleSelection, LocalCorrections},
    llm::LLMEngine,
    retrieval::notebook::{self, AdmittedTarget, NormalizeFailure, Normalized, NotebookText},
    storage::{Store, Uuid},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    io::{Read, Write},
    sync::Arc,
};
pub const MAX_INPUT: usize = 12 * 1024 * 1024;
const MAX_REPORT: usize = 32768;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Failure {
    InvalidInput,
    InputLimit,
    InvalidVersion,
    InvalidImage,
    ContextInvalid,
    ContextStale,
    ContextConflict,
    ContextUnavailable,
    OwnershipBusy,
    UnsupportedMode,
    StorageUnavailable,
    PromptLimit,
    CredentialsUnavailable,
    Canceled,
    ProviderFailed,
    InvalidResponse,
    OutputLimit,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Normalized,
    Unreadable,
    Refused,
    Canceled,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Attempt {
    NotCalled,
    Called,
}
fn nullable_context<'de, D: Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<ContextRequest>, D::Error> {
    Option::deserialize(d)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextRequest {
    #[serde(deserialize_with = "handwriting::id")]
    profile: Uuid,
    examples: Vec<ExampleSelection>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    transcription_version: u32,
    #[serde(deserialize_with = "handwriting::id")]
    invocation: Uuid,
    #[serde(deserialize_with = "handwriting::id")]
    document: Uuid,
    page: u32,
    target_png: String,
    #[serde(deserialize_with = "nullable_context")]
    context: Option<ContextRequest>,
}
#[derive(Serialize)]
pub struct CapturedExample {
    correction: Uuid,
    record: Uuid,
    revision: Uuid,
    sample_sha256: String,
}
#[derive(Serialize)]
pub struct CapturedContext {
    profile: Uuid,
    examples: Vec<CapturedExample>,
}
#[derive(Serialize)]
pub struct Report {
    pub transcription_version: u32,
    pub invocation: Option<Uuid>,
    pub status: Status,
    pub failure: Option<Failure>,
    pub provider_attempt: Attempt,
    pub target_sha256: Option<String>,
    pub context: Option<CapturedContext>,
    pub result: Option<NotebookText>,
    pub confirmed: bool,
    #[serde(skip)]
    _owner: Option<ConfirmedContext>,
    #[serde(skip)]
    _refused_owner: Option<LocalCorrections>,
}
impl Report {
    fn new() -> Self {
        Self {
            transcription_version: 1,
            invocation: None,
            status: Status::Refused,
            failure: None,
            provider_attempt: Attempt::NotCalled,
            target_sha256: None,
            context: None,
            result: None,
            confirmed: false,
            _owner: None,
            _refused_owner: None,
        }
    }
    fn fail(&mut self, f: Failure) {
        self.failure = Some(f);
        self.result = None;
        self.status = match f {
            Failure::Canceled => Status::Canceled,
            Failure::StorageUnavailable | Failure::ProviderFailed => Status::Failed,
            _ => Status::Refused,
        };
    }
    pub fn succeeded(&self) -> bool {
        matches!(self.status, Status::Normalized | Status::Unreadable)
    }
    /// Ownership spans serialization, actual write/flush, errors, and caller drop.
    /// The bool describes the actual emitted status, including bounded fallback.
    pub fn write_to(&self, output: &mut impl Write) -> std::io::Result<bool> {
        let mut value =
            serde_json::to_value(self).map_err(|_| std::io::Error::other("output-failure"))?;
        let mut bytes =
            serde_json::to_vec(&value).map_err(|_| std::io::Error::other("output-failure"))?;
        let mut success = self.succeeded();
        if bytes.len() + 1 > MAX_REPORT {
            value["status"] = serde_json::json!("refused");
            value["failure"] = serde_json::json!("output-limit");
            value["result"] = serde_json::Value::Null;
            success = false;
            bytes =
                serde_json::to_vec(&value).map_err(|_| std::io::Error::other("output-failure"))?;
        }
        if bytes.len() + 1 > MAX_REPORT {
            return Err(std::io::Error::other("output-failure"));
        }
        bytes.push(b'\n');
        output.write_all(&bytes)?;
        output.flush()?;
        Ok(success)
    }
}
struct Prepared {
    request: Option<Request>,
    target: Option<AdmittedTarget>,
    report: Report,
}
impl Prepared {
    fn admit(bytes: &[u8], progress: &mut dyn FnMut() -> anyhow::Result<()>) -> Self {
        let mut this = Self {
            request: None,
            target: None,
            report: Report::new(),
        };
        let result = (|| -> std::result::Result<(), Failure> {
            if bytes.len() > MAX_INPUT {
                return Err(Failure::InputLimit);
            }
            let strict =
                crate::config::inspection::strict_json(bytes).map_err(|_| Failure::InvalidInput)?;
            let request: Request =
                serde_json::from_value(strict).map_err(|_| Failure::InvalidInput)?;
            if request.page == 0 {
                return Err(Failure::InvalidInput);
            }
            this.report.invocation = Some(request.invocation);
            if request.transcription_version != 1 {
                return Err(Failure::InvalidVersion);
            }
            if request.target_png.len() > (8 * 1024 * 1024usize).div_ceil(3) * 4 {
                return Err(Failure::InvalidImage);
            }
            let png = STANDARD
                .decode(&request.target_png)
                .map_err(|_| Failure::InvalidImage)?;
            if STANDARD.encode(&png) != request.target_png {
                return Err(Failure::InvalidImage);
            }
            let mut canceled = false;
            let target = notebook::admit_target(request.page, png, &mut |_| {
                if progress().is_err() {
                    canceled = true;
                    anyhow::bail!("canceled");
                }
                Ok(())
            })
            .map_err(|_| {
                if canceled {
                    Failure::Canceled
                } else {
                    Failure::InvalidImage
                }
            })?;
            this.report.target_sha256 = Some(target.sha256().to_owned());
            this.target = Some(target);
            this.request = Some(request);
            Ok(())
        })();
        if let Err(f) = result {
            this.report.fail(f);
        }
        this
    }
    fn capture(&mut self, config: &Config, progress: &mut dyn FnMut() -> anyhow::Result<()>) {
        if self.report.failure.is_some() {
            return;
        }
        let result = (|| -> std::result::Result<(), Failure> {
            progress().map_err(|_| Failure::Canceled)?;
            let request = self.request.as_ref().ok_or(Failure::InvalidInput)?;
            let Some(context) = &request.context else {
                return Ok(());
            };
            if context.examples.is_empty()
                || context.examples.len() > 3
                || context
                    .examples
                    .iter()
                    .map(|e| e.correction)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != context.examples.len()
            {
                return Err(Failure::ContextInvalid);
            }
            if config.sync.enabled {
                return Err(Failure::UnsupportedMode);
            }
            let store =
                Store::open_existing_for_handwriting(config.paths.clone()).map_err(|error| {
                    match error {
                        crate::storage::maintenance::Refusal::Busy => Failure::OwnershipBusy,
                        crate::storage::maintenance::Refusal::Unavailable
                        | crate::storage::maintenance::Refusal::Unsupported => {
                            Failure::StorageUnavailable
                        }
                    }
                })?;
            let owner = LocalCorrections::new(Arc::new(store), config).map_err(|f| match f {
                handwriting::Failure::UnsupportedMode => Failure::UnsupportedMode,
                _ => Failure::StorageUnavailable,
            })?;
            let admitted = owner.capture_context(context.profile, &context.examples, &mut || {
                progress().map_err(|_| ContextFailure::Canceled)
            });
            let captured = match admitted {
                Ok(captured) => captured,
                Err((f, owner)) => {
                    self.report._refused_owner = Some(owner);
                    return Err(match f {
                        ContextFailure::Invalid => Failure::ContextInvalid,
                        ContextFailure::Stale => Failure::ContextStale,
                        ContextFailure::Conflict => Failure::ContextConflict,
                        ContextFailure::Unavailable => Failure::ContextUnavailable,
                        ContextFailure::Storage => Failure::StorageUnavailable,
                        ContextFailure::Canceled => Failure::Canceled,
                    });
                }
            };
            self.report.context = Some(CapturedContext {
                profile: captured.identity().profile,
                examples: captured
                    .identity()
                    .examples
                    .iter()
                    .map(|e| CapturedExample {
                        correction: e.correction,
                        record: e.record,
                        revision: e.revision,
                        sample_sha256: e.sample_sha256.clone(),
                    })
                    .collect(),
            });
            self.report._owner = Some(captured);
            Ok(())
        })();
        if let Err(f) = result {
            self.report.fail(f);
        }
    }
    fn execute(
        mut self,
        model_profile: &str,
        engine: &mut dyn LLMEngine,
        progress: &mut dyn FnMut() -> anyhow::Result<()>,
    ) -> Report {
        if self.report.failure.is_some() {
            return self.report;
        }
        let request = self.request.as_ref().expect("admitted request");
        let (outcome, called) = notebook::normalize_admitted(
            request.document,
            self.target.as_ref().expect("admitted target"),
            model_profile,
            self.report._owner.as_ref(),
            engine,
            &mut |_| progress(),
        );
        self.report.provider_attempt = if called {
            Attempt::Called
        } else {
            Attempt::NotCalled
        };
        match outcome {
            Ok(Normalized::Page(page)) => {
                self.report.status = Status::Normalized;
                self.report.result = Some(*page);
            }
            Ok(Normalized::Unreadable) => self.report.status = Status::Unreadable,
            Err(f) => self.report.fail(match f {
                NormalizeFailure::Canceled => Failure::Canceled,
                NormalizeFailure::InvalidInput => Failure::InvalidInput,
                NormalizeFailure::PromptLimit => Failure::PromptLimit,
                NormalizeFailure::ProviderFailed => Failure::ProviderFailed,
                NormalizeFailure::InvalidResponse => Failure::InvalidResponse,
            }),
        }
        self.report
    }
}
/// Testable production operation. Engine implementations remain cooperative.
/// No Store is opened for null context. Exactly one engine invocation at most.
pub fn run_with_engine(
    config: &Config,
    bytes: &[u8],
    model_profile: &str,
    engine: &mut dyn LLMEngine,
    progress: &mut dyn FnMut() -> anyhow::Result<()>,
) -> Report {
    let mut prepared = Prepared::admit(bytes, progress);
    prepared.capture(config, progress);
    prepared.execute(model_profile, engine, progress)
}
pub(crate) fn bounded_input(input: &mut impl Read) -> std::result::Result<Vec<u8>, Failure> {
    let mut bytes = Vec::new();
    input
        .take(MAX_INPUT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Failure::InvalidInput)?;
    if bytes.len() > MAX_INPUT {
        Err(Failure::InputLimit)
    } else {
        Ok(bytes)
    }
}
#[cfg(test)]
mod tests;

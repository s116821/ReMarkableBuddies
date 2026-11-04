//! One bounded explicit Writer command. Reports are artifacts, not runtime logs.
use super::*;
use crate::config::{self, Config};
use std::{
    io::Read,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
};

struct NoModel;
impl LLMEngine for NoModel {
    fn add_text_content(&mut self, _: &str) {}
    fn add_image_content(&mut self, _: &str) {}
    fn clear_content(&mut self) {}
    fn execute(&mut self) -> Result<String> {
        anyhow::bail!("model unavailable")
    }
}
pub fn run(config_path: Option<PathBuf>, input: &mut impl Read) -> Report {
    let mut identity = None;
    let result = (|| -> std::result::Result<Report, Failure> {
        let mut bytes = Vec::new();
        input
            .take(256 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Failure::InvalidInput)?;
        if bytes.len() > 256 * 1024 {
            return Err(Failure::TooLarge);
        }
        let request: Request = serde_json::from_slice(&bytes).map_err(|_| Failure::InvalidInput)?;
        identity = Some((request.conversation(), request.operation()));
        request.preflight()?;
        let config = Config::load(config_path).map_err(|_| Failure::Storage)?;
        if !config.admits_writer(false) {
            return Err(Failure::Unsupported);
        }
        let preference = config
            .personalization_snapshot()
            .map_err(|_| Failure::Storage)?;
        // Resolve credentials/settings before opening or changing any Store.
        let model = if matches!(request, Request::PrepareExport { .. }) {
            None
        } else {
            let effective = config::resolve(
                &config,
                config::Overrides::default(),
                config::Environment::current(),
            )
            .map_err(|_| Failure::MissingCredentials)?;
            Some(crate::OpenAI::new(
                effective.model,
                effective.key.into_string(),
                effective.base_url,
            ))
        };
        let ledger = Ledger::new(Arc::new(
            Store::open(config.paths).map_err(|_| Failure::Storage)?,
        ));
        if let Some(mut engine) = model {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| Failure::Unsupported)?;
            let flag = Arc::new(AtomicBool::new(false));
            let worker_flag = flag.clone();
            runtime.block_on(async move {
                let mut worker = tokio::task::spawn_blocking(move || {
                    super::run(&ledger, &mut engine, &request, &preference, &mut || {
                        worker_flag.load(Ordering::Acquire)
                    })
                });
                let joined = tokio::select! {
                    result=&mut worker=>result,
                    _=tokio::signal::ctrl_c()=>{flag.store(true,Ordering::Release);worker.await},
                };
                joined.map_err(|_| Failure::Unsupported)
            })
        } else {
            Ok(super::run(
                &ledger,
                &mut NoModel,
                &request,
                &preference,
                &mut || false,
            ))
        }
    })();
    match result {
        Ok(report) => report,
        Err(failure) => {
            let (conversation, operation) = identity.unwrap_or((Uuid::nil(), Uuid::nil()));
            Report {
                status: Status::Refused,
                conversation,
                operation,
                draft: None,
                failure: Some(failure),
                prepared_note: None,
            }
        }
    }
}

/// Existing Writer report bytes remain unchanged for legacy commands.
#[derive(Serialize)]
#[serde(untagged)]
pub enum CommandReport {
    Legacy(Report),
    Flow(crate::writer_flow::Report),
    Notebook(Box<notebook::command::Report>),
}
#[derive(Deserialize)]
#[serde(untagged)]
enum CommandRequest {
    Legacy(Request),
    Flow(crate::writer_flow::Request),
    Notebook(Box<notebook::command::Request>),
}
/// Parse bounded stdin once; each strict variant rejects duplicates and unknown fields.
pub fn dispatch(config_path: Option<PathBuf>, input: &mut impl Read) -> CommandReport {
    let mut bytes = Vec::new();
    let failure = if input.take(256 * 1024 + 1).read_to_end(&mut bytes).is_err() {
        Some(Failure::InvalidInput)
    } else if bytes.len() > 256 * 1024 {
        Some(Failure::TooLarge)
    } else {
        None
    };
    if let Some(failure) = failure {
        return CommandReport::Legacy(Report {
            status: Status::Refused,
            conversation: Uuid::nil(),
            operation: Uuid::nil(),
            draft: None,
            failure: Some(failure),
            prepared_note: None,
        });
    }
    match serde_json::from_slice::<CommandRequest>(&bytes) {
        Ok(CommandRequest::Legacy(request)) => {
            // Retain the public legacy API and its exact validation/report behavior.
            drop(request);
            CommandReport::Legacy(run(config_path, &mut &bytes[..]))
        }
        Ok(CommandRequest::Flow(request)) => CommandReport::Flow(run_flow(config_path, request)),
        Ok(CommandRequest::Notebook(request)) => {
            CommandReport::Notebook(Box::new(notebook::command::run(config_path, *request)))
        }
        Err(_) => CommandReport::Legacy(run(config_path, &mut &bytes[..])),
    }
}
fn run_flow(
    config_path: Option<PathBuf>,
    request: crate::writer_flow::Request,
) -> crate::writer_flow::Report {
    use crate::{
        note_export::mem::MemDestination,
        writer_flow::{self, Failure as FlowFailure},
    };
    let result = (|| -> std::result::Result<_, FlowFailure> {
        request.preflight().map_err(|_| FlowFailure::InvalidInput)?;
        let config = Config::load(config_path).map_err(|_| FlowFailure::Storage)?;
        if !matches!(request, writer_flow::Request::ReconcileExport { .. })
            && !config.admits_writer(true)
        {
            return Err(FlowFailure::Unsupported);
        }
        let preference = config
            .personalization_snapshot()
            .map_err(|_| FlowFailure::Storage)?;
        // All required credentials are resolved before Store creation or intent admission.
        let model = if request.model().is_some() {
            let effective = config::resolve(
                &config,
                config::Overrides::default(),
                config::Environment::current(),
            )
            .map_err(|_| FlowFailure::MissingCredentials)?;
            Some(crate::OpenAI::new(
                effective.model,
                effective.key.into_string(),
                effective.base_url,
            ))
        } else {
            None
        };
        let token = std::env::var("MEM_API_KEY").map_err(|_| FlowFailure::MissingCredentials)?;
        let destination =
            MemDestination::new(&token).map_err(|_| FlowFailure::MissingCredentials)?;
        let ledger = Ledger::new(Arc::new(
            Store::open(config.paths).map_err(|_| FlowFailure::Storage)?,
        ));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| FlowFailure::Unsupported)?;
        let owned_request = request.clone();
        runtime.block_on(async move {
            let (sender, canceled) = tokio::sync::watch::channel(false);
            let mut worker = tokio::task::spawn_blocking(move || {
                if let Some(mut model) = model {
                    writer_flow::run(
                        &ledger,
                        &mut model,
                        &destination,
                        &owned_request,
                        &preference,
                        canceled,
                    )
                } else {
                    writer_flow::run(
                        &ledger,
                        &mut NoModel,
                        &destination,
                        &owned_request,
                        &preference,
                        canceled,
                    )
                }
            });
            let joined = tokio::select! {
                result = &mut worker => result,
                _ = tokio::signal::ctrl_c() => {
                    let _ = sender.send(true);
                    worker.await
                }
            };
            // Keep sender alive until the owned worker has completed; cancellation is latched.
            drop(sender);
            joined.map_err(|_| FlowFailure::Unsupported)
        })
    })();
    result.unwrap_or_else(|failure| writer_flow::Report::refused(&request, failure))
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    struct Broken {
        bytes: std::io::Cursor<Vec<u8>>,
    }
    impl Read for Broken {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            let n = self.bytes.read(output)?;
            if n == 0 {
                Err(std::io::Error::other("injected after complete JSON"))
            } else {
                Ok(n)
            }
        }
    }
    #[test]
    fn read_error_after_valid_json_is_invalid_without_opening_config_or_store() {
        let request = Request::PrepareExport {
            conversation: Uuid::new_v4(),
            draft: Uuid::new_v4(),
            correlation: Uuid::new_v4(),
            effect_operation: Uuid::new_v4(),
        };
        let mut input = Broken {
            bytes: std::io::Cursor::new(serde_json::to_vec(&request).unwrap()),
        };
        let report = dispatch(
            Some(PathBuf::from("missing-config-must-not-be-read")),
            &mut input,
        );
        let CommandReport::Legacy(report) = report else {
            panic!("legacy failure expected")
        };
        assert_eq!(report.failure, Some(Failure::InvalidInput));
        assert!(report.conversation.is_nil());
    }
    #[test]
    fn strict_dispatch_rejects_duplicate_and_unknown_flow_fields() {
        let id = Uuid::new_v4();
        for bytes in [
            format!(
                r#"{{"command":"reconcile-export","conversation":"{id}","correlation":"{}","extra":1}}"#,
                Uuid::new_v4()
            ),
            format!(
                r#"{{"command":"reconcile-export","conversation":"{id}","conversation":"{id}","correlation":"{}"}}"#,
                Uuid::new_v4()
            ),
        ] {
            assert!(serde_json::from_str::<CommandRequest>(&bytes).is_err());
        }
    }
}

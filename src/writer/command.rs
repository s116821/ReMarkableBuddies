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

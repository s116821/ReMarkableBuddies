//! Early explicit CLI path; no device, model retry, or sync worker startup.
use super::*;
use crate::config::{self, Environment, Overrides};
use std::path::PathBuf;
fn configured(
    path: Option<PathBuf>,
    bytes: Vec<u8>,
    progress: &mut dyn FnMut() -> anyhow::Result<()>,
) -> Report {
    let mut prepared = Prepared::admit(&bytes, progress);
    if prepared.report.failure.is_some() {
        return prepared.report;
    }
    let config = match Config::load(path) {
        Ok(c) => c,
        Err(_) => {
            prepared.report.fail(Failure::StorageUnavailable);
            return prepared.report;
        }
    };
    prepared.capture(&config, progress);
    if prepared.report.failure.is_some() {
        return prepared.report;
    }
    let effective = match config::resolve(&config, Overrides::default(), Environment::current()) {
        Ok(c) => c,
        Err(_) => {
            prepared.report.fail(Failure::CredentialsUnavailable);
            return prepared.report;
        }
    };
    let model = effective.model.clone();
    let mut engine = crate::OpenAI::new(
        effective.model,
        effective.key.into_string(),
        effective.base_url,
    );
    prepared.execute(&model, &mut engine, progress)
}
/// Poll the owned signal future on this CLI thread at existing progress points.
/// No wrapper worker or arbitrary-engine preemption is introduced.
pub fn dispatch(path: Option<PathBuf>, input: &mut impl Read) -> Report {
    let bytes = match bounded_input(input) {
        Ok(b) => b,
        Err(f) => {
            let mut report = Report::new();
            report.fail(f);
            return report;
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            let mut report = Report::new();
            report.fail(Failure::ProviderFailed);
            return report;
        }
    };
    let mut signal = Box::pin(tokio::signal::ctrl_c());
    let mut canceled = false;
    let mut progress = || {
        if canceled {
            anyhow::bail!("canceled");
        }
        runtime.block_on(async {
            tokio::select! { biased;
                _ = &mut signal => {canceled=true;Err(anyhow::anyhow!("canceled"))},
                _ = tokio::task::yield_now() => Ok(()),
            }
        })
    };
    configured(path, bytes, &mut progress)
}

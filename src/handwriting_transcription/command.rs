//! Early explicit CLI path; no device, model retry, or sync worker startup.
use super::*;
use crate::config::{self, Environment, Overrides};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};
fn configured(path: Option<PathBuf>, bytes: Vec<u8>, canceled: &AtomicBool) -> Report {
    let mut progress = || {
        if canceled.load(Ordering::Acquire) {
            anyhow::bail!("canceled");
        }
        Ok(())
    };
    let mut prepared = Prepared::admit(&bytes, &mut progress);
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
    prepared.capture(&config, &mut progress);
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
    prepared.execute(&model, &mut engine, &mut progress)
}
/// Signal handling follows the existing one-shot Writer command. It waits for
/// cooperative configured transport completion; this is not engine preemption.
pub fn dispatch(path: Option<PathBuf>, input: &mut impl Read) -> anyhow::Result<Report> {
    let bytes = match bounded_input(input) {
        Ok(b) => b,
        Err(f) => {
            let mut r = Report::new();
            r.fail(f);
            return Ok(r);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(_) => {
            let mut r = Report::new();
            r.fail(Failure::ProviderFailed);
            return Ok(r);
        }
    };
    let flag = Arc::new(AtomicBool::new(false));
    let worker_flag = flag.clone();
    runtime.block_on(async move {
        let mut worker=tokio::task::spawn_blocking(move||configured(path,bytes,&worker_flag));
        let result=tokio::select! {r=&mut worker=>r,_=tokio::signal::ctrl_c()=>{flag.store(true,Ordering::Release);worker.await}};
        result.map_err(|_| anyhow::anyhow!("transcription-unavailable"))
    })
}

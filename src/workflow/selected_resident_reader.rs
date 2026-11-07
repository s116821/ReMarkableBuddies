//! Installed input-only selected Reader. The resident Session retains native custody.
use super::*;
use crate::{
    config::{self, Environment, Overrides},
    conversation::{selected_reader_prepare::ResidentPrepared, Ledger, SelectedAdmission},
    device::{request_input::RequestInput, touch::TriggerCorner},
    storage::{
        sync::{
            selected::{ReaderPhase, ReaderPreferences},
            Worker,
        },
        Store, Uuid,
    },
    OpenAI,
};
use anyhow::{ensure, Context, Result};
use resident_supervisor::{health::ServiceHealth, recovery::activation::InstalledReaderSelection};
use std::{
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) fn finish_selected_request(workflow: &mut Workflow) -> Result<()> {
    let result = workflow.finish_request();
    if workflow.runtime_failed() {
        return result.and_then(|_| {
            Err(anyhow::anyhow!(
                "selected Reader runtime lost during finalization"
            ))
        });
    }
    match result {
        Err(error) if error.is::<resident_source::FinalizedCancellation>() => Ok(()),
        result => result.context("selected Reader request finalization failed"),
    }
}

pub(crate) fn run(
    store: Arc<Store>,
    reader: Arc<SelectedAdmission>,
    config_path: &Path,
    installation: InstalledReaderSelection,
    health: ServiceHealth,
    startup: ReaderPreferences,
    startup_token: crate::storage::selection::SelectionToken,
) -> Result<()> {
    let corner =
        TriggerCorner::from_string(startup.config().trigger_corner.as_deref().unwrap_or("LL"))
            .map_err(|_| anyhow::anyhow!("invalid selected Reader corner"))?;
    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    let preactivate = installation.has_installed_output();
    let mut workflow = Workflow::selected_runtime(reader.clone(), false);
    let verify_startup = || {
        let mut reads = startup.reads(ReaderPhase::Acquisition)?;
        let [catalog, preference] = startup.gates();
        SelectedAdmission::with_gates(&[reader.as_ref(), catalog, preference], |store| {
            startup.verify_locked(store, &reader, &startup_token, &mut reads)?;
            startup.require_acquisition_time()
        })
    };
    verify_startup()?;
    workflow.enable_service_health(health)?;
    workflow.configure_resident_source(installation, store.clone())?;
    let _worker = Worker::google(store.clone(), startup.config().sync.clone())?;
    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    if preactivate {
        workflow
            .resident_source
            .as_mut()
            .unwrap()
            .activate_at_startup(
                corner.clone(),
                workflow.service_health.as_mut().unwrap(),
                Some(startup.acquisition_deadline()),
                verify_startup,
            )?;
    }
    let mode = startup.mode_token().clone();
    drop(startup);

    loop {
        let release = {
            let owner = workflow.resident_source.as_mut().unwrap();
            let health = workflow.service_health.as_mut().unwrap();
            RequestInput::wait_released_reader(corner.clone(), false, &mut || {
                owner.progress(health)
            })?
        };
        let policy = (|| {
            ensure!(
                store.sync_mode_snapshot()?.token == mode,
                "selected Reader service mode changed"
            );
            let (policy, token) = ReaderPreferences::open(
                store.clone(),
                config_path.to_owned(),
                &reader,
                release.released_at,
            )?;
            ensure!(
                *policy.mode_token() == mode,
                "selected Reader service mode changed"
            );
            Ok((policy, token))
        })();
        let (policy, token) = match policy {
            Ok(policy) => policy,
            Err(error) => {
                if store.sync_mode_snapshot()?.token != mode {
                    return Err(error);
                }
                log::info!("Selected Reader release refused before acquisition");
                continue;
            }
        };

        let mut began = false;
        let result = (|| -> Result<_> {
            let check = |expected: &crate::storage::selection::SelectionToken| -> Result<()> {
                let mut reads = policy.reads(ReaderPhase::Acquisition)?;
                let [catalog, preference] = policy.gates();
                SelectedAdmission::with_gates(&[reader.as_ref(), catalog, preference], |store| {
                    policy.verify_locked(store, &reader, expected, &mut reads)?;
                    policy.require_acquisition_time()
                })
            };
            check(&token)?;
            began = true;
            workflow
                .resident_source
                .as_mut()
                .unwrap()
                .begin_with_check(release, workflow.service_health.as_mut().unwrap(), || {
                    check(&token)
                })?;
            workflow.begin_iteration()?;
            let capture = workflow
                .resident_source
                .as_mut()
                .unwrap()
                .acquire_with_check(workflow.service_health.as_mut().unwrap(), |observed| {
                    check(&token)?;
                    if let Some(facts) = observed {
                        let document = Uuid::from_bytes(*facts.page().document.as_bytes());
                        ensure!(
                            crate::memory::catalog::document_key(document)?
                                == reader.scope().key_sha256,
                            "resident observation belongs to another selected document"
                        );
                    }
                    Ok(())
                })?;
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_millis()
                .try_into()?;
            let prepared =
                ResidentPrepared::new(&store, &token, &capture, Uuid::new_v4(), timestamp)?;
            let (token, acknowledgment) = {
                let mut reads = policy.reads(ReaderPhase::Acquisition)?;
                let [catalog, preference] = policy.gates();
                SelectedAdmission::with_gates(&[reader.as_ref(), catalog, preference], |store| {
                    policy.verify_locked(store, &reader, &token, &mut reads)?;
                    policy.require_acquisition_time()?;
                    workflow
                        .resident_source
                        .as_mut()
                        .unwrap()
                        .acquisition_current(workflow.service_health.as_mut().unwrap())?;
                    prepared.publish_locked(store, &token, &mut reads)
                })?
            };
            check(&token)?;
            workflow
                .resident_source
                .as_mut()
                .unwrap()
                .acquisition_current(workflow.service_health.as_mut().unwrap())?;
            let config = policy.config();
            let memory = config
                .memory
                .enabled
                .then(|| crate::memory::configured(store.clone(), &config.memory))
                .transpose()?;
            if let Some(memory) = &memory {
                memory.require_selected_reader(&reader)?;
            }
            let personalization = config.personalization_snapshot()?;
            check(&token)?;
            let effective = config::resolve(config, Overrides::default(), Environment::current())?;
            let engine = OpenAI::new(
                effective.model,
                effective.key.into_string(),
                effective.base_url,
            )
            .with_single_request();
            Ok((token, acknowledgment, engine, memory, personalization))
        })();

        let result = match result {
            Ok((token, acknowledgment, engine, memory, personalization)) => {
                let mut receiver =
                    orchestrator::Orchestrator::new(workflow, engine, Ledger::new(store.clone()));
                if let Some(memory) = memory {
                    receiver.set_selected_subject_memory(memory);
                }
                receiver.set_personalization(personalization);
                let result = receiver.receive_resident_prepared(token, acknowledgment, policy);
                workflow = receiver.into_workflow();
                result
            }
            Err(error) => Err(error),
        };
        if workflow.runtime_failed() {
            return result.and_then(|_| Err(anyhow::anyhow!("selected Reader runtime lost")));
        }
        if began {
            finish_selected_request(&mut workflow)?;
        }
        if result.is_err() {
            log::info!("Selected Reader request ended without further output");
        }
    }
}

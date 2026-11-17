//! One explicit bounded stdin command; no provider, tablet or service startup.
use super::dispatch::{ConfiguredDestination, OwnedDestination};
use super::*;
use crate::{config::Config, conversation::Ledger, storage::Store};
use std::{io::Read, path::PathBuf, sync::Arc};

#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "kebab-case", deny_unknown_fields)]
enum Command {
    Export {
        note: PreparedNote,
    },
    Reconcile {
        conversation: Uuid,
        correlation: Uuid,
    },
}
impl Command {
    fn identities(&self) -> (Uuid, Uuid) {
        match self {
            Self::Export { note } => (note.correlation, note.effect_operation),
            Self::Reconcile { correlation, .. } => (*correlation, Uuid::nil()),
        }
    }
}

pub fn run(config_path: Option<PathBuf>, input: &mut impl Read) -> ExportReport {
    let mut identities = (Uuid::nil(), Uuid::nil());
    let result = (|| -> Result<ExportReport> {
        let mut bytes = Vec::new();
        input
            .take(MAX_INPUT as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Failure::InvalidInput)?;
        if bytes.len() > MAX_INPUT {
            return Err(Failure::InvalidInput);
        }
        let command: Command = serde_json::from_slice(&bytes).map_err(|_| Failure::InvalidInput)?;
        identities = command.identities();
        match &command {
            Command::Export { note } => {
                note.render(CorrelationLocation::Metadata)?;
            }
            Command::Reconcile {
                conversation,
                correlation,
            } => {
                if conversation.is_nil() || correlation.is_nil() || conversation == correlation {
                    return Err(Failure::InvalidInput);
                }
            }
        }
        let config = Config::load(config_path).map_err(|_| Failure::Storage)?;
        let (ledger, destination) = match &command {
            Command::Export { note } => {
                if !config.admits_writer(true) {
                    return Err(Failure::Unsupported);
                }
                if config.export_destination() == crate::config::product::ExportDestination::Mem {
                    note.render(CorrelationLocation::Title)?;
                }
                let destination = ConfiguredDestination::selected(&config)?;
                let store = Store::open(config.paths).map_err(|_| Failure::Storage)?;
                (Ledger::new(Arc::new(store)), destination)
            }
            Command::Reconcile {
                conversation,
                correlation,
            } => ConfiguredDestination::original(&config, *conversation, *correlation)?,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| Failure::Unsupported)?;
        Ok(runtime.block_on(async {
            let Some(owner) = ExportOwner::acquire(&ledger) else {
                return ExportReport::new(ExportStatus::Busy, identities.0, identities.1);
            };
            let (sender, canceled) = tokio::sync::watch::channel(false);
            let signal_sender = sender.clone();
            let signal = tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                let _ = signal_sender.send(true);
            });
            let report = match command {
                Command::Export { note } => {
                    destination
                        .export_owned(&ledger, &note, canceled, &owner, None)
                        .await
                }
                Command::Reconcile {
                    conversation,
                    correlation,
                } => {
                    destination
                        .reconcile_owned(&ledger, conversation, correlation, canceled, &owner, None)
                        .await
                }
            };
            signal.abort();
            drop(sender);
            report
        }))
    })();
    match result {
        Ok(result) => result,
        Err(failure) => {
            let status = if failure == Failure::Conflict {
                ExportStatus::Conflict
            } else {
                ExportStatus::Refused
            };
            let mut report = ExportReport::new(status, identities.0, identities.1);
            report.failure = Some(failure);
            report
        }
    }
}

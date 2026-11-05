//! Fixed, existing-only local journal counts. No retry or publication authority.
use crate::{
    config::Config,
    storage::{
        maintenance::Refusal,
        selection::{InventoryRefusal, SelectedAttemptState},
        Store, StorePaths,
    },
};
use anyhow::{ensure, Result};
use serde::Serialize;
use std::{io::Write, path::PathBuf};

const MAX_OUTPUT: usize = 1024;

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum Status {
    Ok,
    Busy,
    Unavailable,
    Corrupt,
    Unsupported,
}

#[derive(Default, Serialize)]
struct Counts {
    pending: usize,
    withdrawn: usize,
}

#[derive(Serialize)]
struct Report {
    report_version: u32,
    operation: &'static str,
    status: Status,
    counts: Option<Counts>,
}

pub struct Inspection {
    report: Report,
    _store: Option<Store>,
}

impl Inspection {
    fn new(status: Status, counts: Option<Counts>, store: Option<Store>) -> Self {
        Self {
            report: Report {
                report_version: 1,
                operation: "inspect-selected-outbox",
                status,
                counts,
            },
            _store: store,
        }
    }

    /// Keep existing ownership through the single complete or failed output.
    pub fn write_to(self, output: &mut impl Write) -> Result<()> {
        let mut bytes = serde_json::to_vec(&self.report)?;
        ensure!(
            bytes.len() < MAX_OUTPUT,
            "outbox status report exceeds bound"
        );
        bytes.push(b'\n');
        output.write_all(&bytes)?;
        output.flush()?;
        Ok(())
    }
}

pub fn run(config_path: Option<PathBuf>) -> Inspection {
    match Config::load(config_path) {
        Ok(config) => inspect(config.paths),
        Err(_) => Inspection::new(Status::Unavailable, None, None),
    }
}

pub(crate) fn inspect(paths: StorePaths) -> Inspection {
    let store = match Store::open_existing_for_outbox(paths) {
        Ok(store) => store,
        Err(refusal) => {
            let status = match refusal {
                Refusal::Busy => Status::Busy,
                Refusal::Unavailable => Status::Unavailable,
                Refusal::Unsupported => Status::Unsupported,
            };
            return Inspection::new(status, None, None);
        }
    };
    match store.selected_attempt_inventory() {
        Ok(entries) => {
            let mut counts = Counts::default();
            for entry in entries {
                match entry.state {
                    SelectedAttemptState::Pending => counts.pending += 1,
                    SelectedAttemptState::Withdrawn => counts.withdrawn += 1,
                }
            }
            Inspection::new(Status::Ok, Some(counts), Some(store))
        }
        Err(error) => {
            let status = match error.downcast_ref::<InventoryRefusal>() {
                Some(InventoryRefusal::Corrupt) => Status::Corrupt,
                Some(InventoryRefusal::Unsupported) => Status::Unsupported,
                _ => Status::Unavailable,
            };
            Inspection::new(status, None, Some(store))
        }
    }
}

//! Explicit archive custody commands. Never restore or activate archived state.
use crate::{
    config::Config,
    storage::{
        files,
        maintenance::Refusal,
        sync::mode::{OrdinaryModeLease, SyncMode, SyncModeLease},
        BackupInspection, Store,
    },
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

pub const MAX_INPUT: usize = 16_384;
pub const MAX_OUTPUT: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    Inspect,
    ExportSelected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Status {
    Ok,
    InvalidInput,
    Unavailable,
    Unsupported,
    Busy,
    Failed,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    path: PathBuf,
}

#[derive(Serialize)]
struct Data {
    inspection: BackupInspection,
}

struct Custody {
    _store: Arc<Store>,
    _ordinary: Option<OrdinaryModeLease>,
    _explicit: Option<SyncModeLease>,
}
impl Custody {
    fn acquire(store: Arc<Store>) -> Result<Self> {
        let current = store.sync_mode_snapshot()?;
        let (ordinary, explicit) = if matches!(current.mode, None | Some(SyncMode::LocalOnly)) {
            (Some(store.ordinary_mode_lease(&current.token)?), None)
        } else {
            (None, Some(store.sync_mode_lease(&current.token)?))
        };
        Ok(Self {
            _store: store,
            _ordinary: ordinary,
            _explicit: explicit,
        })
    }
}

#[derive(Serialize)]
pub struct Report {
    report_version: u32,
    operation: Operation,
    status: Status,
    incomplete: bool,
    activation_performed: bool,
    data: Option<Data>,
    #[serde(skip)]
    _custody: Option<Custody>,
}
impl Report {
    fn new(operation: Operation, status: Status) -> Self {
        Self {
            report_version: 1,
            operation,
            status,
            incomplete: matches!(status, Status::Failed | Status::Unavailable),
            activation_performed: false,
            data: None,
            _custody: None,
        }
    }
    fn completed(operation: Operation, inspection: BackupInspection) -> Self {
        let mut report = Self::new(operation, Status::Ok);
        report.incomplete = !inspection.complete_media;
        report.data = Some(Data { inspection });
        report
    }
    /// Owns source custody until this one consuming write completes or fails.
    pub fn write_to(self, writer: &mut impl Write) -> Result<()> {
        let mut bytes = serde_json::to_vec(&self)?;
        bytes.push(b'\n');
        ensure!(bytes.len() <= MAX_OUTPUT, "archive report exceeds bound");
        writer.write_all(&bytes)?;
        Ok(())
    }
}

fn request(input: &Path, stdin: &mut impl Read) -> Result<Request> {
    let request: Request = if input.as_os_str() == std::ffi::OsStr::new("-") {
        let mut bytes = Vec::new();
        stdin.take((MAX_INPUT + 1) as u64).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= MAX_INPUT, "archive input exceeds bound");
        serde_json::from_slice(&bytes)?
    } else {
        files::json(input, MAX_INPUT as u64)?
    };
    files::safe_path(&request.path)?;
    Ok(request)
}

fn inspect(path: &Path) -> Report {
    // Check marker type before opening: absent/nonregular paths must not block.
    let marker = path.join("backup.json");
    if files::safe_path(&marker).is_err() {
        return Report::new(Operation::Inspect, Status::InvalidInput);
    }
    match std::fs::symlink_metadata(&marker) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Report::new(Operation::Inspect, Status::Unavailable)
        }
        Ok(metadata) if metadata.is_file() => (),
        _ => return Report::new(Operation::Inspect, Status::Unsupported),
    }
    match Store::inspect_backup(path) {
        Ok(inspection) => Report::completed(Operation::Inspect, inspection),
        Err(_) => Report::new(Operation::Inspect, Status::Unsupported),
    }
}

fn export(store: Arc<Store>, destination: &Path) -> Report {
    let custody = match Custody::acquire(store.clone()) {
        Ok(custody) => custody,
        Err(_) => return Report::new(Operation::ExportSelected, Status::Unsupported),
    };
    let mut report = match store.export_selected(destination) {
        Ok(inspection) => Report::completed(Operation::ExportSelected, inspection),
        // The marker might already be published. Never retry or remove output.
        Err(_) => Report::new(Operation::ExportSelected, Status::Failed),
    };
    report._custody = Some(custody);
    report
}

pub fn run(
    operation: Operation,
    input: &Path,
    config_path: Option<PathBuf>,
    stdin: &mut impl Read,
) -> Report {
    let request = match request(input, stdin) {
        Ok(value) => value,
        Err(_) => return Report::new(operation, Status::InvalidInput),
    };
    if operation == Operation::Inspect {
        return inspect(&request.path);
    }
    let config = match Config::load(config_path) {
        Ok(config) if !config.sync.enabled => config,
        _ => return Report::new(operation, Status::Unsupported),
    };
    // Destination refusal precedes Store admission and all export writes.
    if request.path.exists()
        || [
            &config.paths.data,
            &config.paths.cache,
            &config.paths.credentials,
        ]
        .iter()
        .any(|owned| request.path.starts_with(owned) || owned.starts_with(&request.path))
    {
        return Report::new(operation, Status::InvalidInput);
    }
    match Store::open_existing_for_archive(config.paths) {
        Ok(store) => export(Arc::new(store), &request.path),
        Err(refusal) => Report::new(
            operation,
            match refusal {
                Refusal::Busy => Status::Busy,
                Refusal::Unavailable => Status::Unavailable,
                Refusal::Unsupported => Status::Unsupported,
            },
        ),
    }
}

#[cfg(test)]
mod tests;

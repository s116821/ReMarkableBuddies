//! Explicit archive custody commands. Never restore or activate archived state.
use crate::{
    config::Config,
    storage::{
        files,
        maintenance::Refusal,
        notebook_archive::{check_destination, Detached, ExportSource, Witness},
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
    InspectIsolated,
    ExportSelected,
    InspectNotebook,
    ExportNotebook,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IsolatedRequest {
    path: PathBuf,
    marker_sha256: String,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Data {
    Archive { inspection: BackupInspection },
    Isolated { isolated_package: serde_json::Value },
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

enum NotebookCustody {
    Detached(Detached),
    Export(Box<ExportSource>, Option<Witness>),
}
impl NotebookCustody {
    fn verify(&mut self) -> Result<()> {
        match self {
            Self::Detached(detached) => detached.verify(),
            Self::Export(source, Some(witness)) => source.verify(witness),
            Self::Export(_, None) => Ok(()),
        }
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
    #[serde(skip)]
    notebook: Option<NotebookCustody>,
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
            notebook: None,
        }
    }
    fn completed(operation: Operation, inspection: BackupInspection) -> Self {
        let mut report = Self::new(operation, Status::Ok);
        report.incomplete = !inspection.complete_media;
        report.data = Some(Data::Archive { inspection });
        report
    }
    /// Owns source custody until this one consuming write completes or fails.
    pub fn write_to(mut self, writer: &mut impl Write) -> Result<()> {
        if let Some(custody) = &mut self.notebook {
            custody.verify()?;
        }
        let mut bytes = serde_json::to_vec(&self)?;
        bytes.push(b'\n');
        ensure!(bytes.len() <= MAX_OUTPUT, "archive report exceeds bound");
        writer.write_all(&bytes)?;
        if matches!(
            self.operation,
            Operation::InspectNotebook | Operation::ExportNotebook
        ) {
            writer.flush()?;
        }
        if let Some(custody) = &mut self.notebook {
            custody.verify()?;
        }
        Ok(())
    }
}

fn decode_request<T: serde::de::DeserializeOwned>(
    input: &Path,
    stdin: &mut impl Read,
) -> Result<T> {
    if input.as_os_str() == std::ffi::OsStr::new("-") {
        let mut bytes = Vec::new();
        stdin.take((MAX_INPUT + 1) as u64).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= MAX_INPUT, "archive input exceeds bound");
        Ok(serde_json::from_slice(&bytes)?)
    } else {
        files::json(input, MAX_INPUT as u64)
    }
}

fn request(input: &Path, stdin: &mut impl Read) -> Result<Request> {
    let request: Request = decode_request(input, stdin)?;
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

fn inspect_isolated(input: &Path, stdin: &mut impl Read) -> Report {
    let operation = Operation::InspectIsolated;
    let request = (|| -> Result<IsolatedRequest> {
        let request: IsolatedRequest = decode_request(input, stdin)?;
        files::safe_path(&request.path)?;
        ensure!(
            crate::storage::valid_digest(&request.marker_sha256),
            "invalid marker digest"
        );
        Ok(request)
    })();
    let Ok(request) = request else {
        return Report::new(operation, Status::InvalidInput);
    };
    // Refuse absent/nonregular markers before opening, just like Inspect.
    match std::fs::symlink_metadata(request.path.join("isolated-backup.json")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Report::new(operation, Status::Unavailable)
        }
        Ok(metadata) if metadata.is_file() => (),
        _ => return Report::new(operation, Status::Unsupported),
    }
    match Store::inspect_isolated_package(&request.path, Some(&request.marker_sha256)) {
        Ok(inspection) => {
            let incomplete = !inspection.data.complete_media;
            // Serialize the existing redacted oracle type without widening its
            // module visibility or introducing another package interpretation.
            match serde_json::to_value(inspection) {
                Ok(isolated_package) => {
                    let mut report = Report::new(operation, Status::Ok);
                    report.incomplete = incomplete;
                    report.data = Some(Data::Isolated { isolated_package });
                    report
                }
                Err(_) => Report::new(operation, Status::Failed),
            }
        }
        Err(_) => Report::new(operation, Status::Unsupported),
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

fn inspect_notebook(path: &Path) -> Report {
    let operation = Operation::InspectNotebook;
    match std::fs::symlink_metadata(path.join("backup.json")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Report::new(operation, Status::Unavailable)
        }
        Ok(metadata) if metadata.is_file() => (),
        _ => return Report::new(operation, Status::Unsupported),
    }
    match Detached::inspect(path) {
        Ok((inspection, custody)) => {
            let mut report = Report::completed(operation, inspection);
            report.notebook = Some(NotebookCustody::Detached(custody));
            report
        }
        Err(_) => Report::new(operation, Status::Unsupported),
    }
}

fn status_for_refusal(refusal: Refusal) -> Status {
    match refusal {
        Refusal::Busy => Status::Busy,
        Refusal::Unavailable => Status::Unavailable,
        Refusal::Unsupported => Status::Unsupported,
    }
}

fn export_notebook(config: Config, destination: &Path) -> Report {
    let operation = Operation::ExportNotebook;
    if config.reader.selection.is_some() || config.memory.selection.is_some() {
        return Report::new(operation, Status::Unsupported);
    }
    let mut source = match ExportSource::open(config.paths) {
        Ok(source) => source,
        Err(refusal) => return Report::new(operation, status_for_refusal(refusal)),
    };
    let (mut report, witness) = match source.prepare() {
        Ok(prepared) => match source.export(prepared, destination) {
            Ok((inspection, witness)) => (Report::completed(operation, inspection), Some(witness)),
            Err(_) => (Report::new(operation, Status::Failed), None),
        },
        Err(_) => (Report::new(operation, Status::Unsupported), None),
    };
    report.notebook = Some(NotebookCustody::Export(Box::new(source), witness));
    report
}

pub fn run(
    operation: Operation,
    input: &Path,
    config_path: Option<PathBuf>,
    stdin: &mut impl Read,
) -> Report {
    if operation == Operation::InspectIsolated {
        return inspect_isolated(input, stdin);
    }
    let request = match request(input, stdin) {
        Ok(value) => value,
        Err(_) => return Report::new(operation, Status::InvalidInput),
    };
    match operation {
        Operation::Inspect => return inspect(&request.path),
        Operation::InspectNotebook => return inspect_notebook(&request.path),
        Operation::ExportSelected | Operation::ExportNotebook => (),
        Operation::InspectIsolated => unreachable!("handled before ordinary archive input"),
    }
    let config = match Config::load(config_path) {
        Ok(config) if !config.sync.enabled => config,
        _ => return Report::new(operation, Status::Unsupported),
    };
    // Destination refusal precedes Store admission and all export writes.
    let destination_exists = if operation == Operation::ExportNotebook {
        match std::fs::symlink_metadata(&request.path) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => return Report::new(operation, Status::Unavailable),
        }
    } else {
        request.path.exists()
    };
    if destination_exists
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
    if operation == Operation::ExportNotebook {
        if check_destination(&config.paths, &request.path).is_err() {
            return Report::new(operation, Status::InvalidInput);
        }
        return export_notebook(config, &request.path);
    }
    match Store::open_existing_for_archive(config.paths) {
        Ok(store) => export(Arc::new(store), &request.path),
        Err(refusal) => Report::new(operation, status_for_refusal(refusal)),
    }
}

#[cfg(test)]
mod tests;

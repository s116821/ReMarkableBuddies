//! Explicit local mode intent. No worker, provider or native activation.
use crate::{
    config::Config,
    storage::{
        self, files,
        maintenance::Refusal,
        sync::mode::{
            self, OrdinaryModeLease, SyncModeLease, SyncModeReceipt, SyncModeSnapshot,
            SyncModeToken,
        },
        Store, Uuid,
    },
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

pub const MAX_INPUT: usize = 16 * 1024;
pub const MAX_OUTPUT: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    Initialize,
    Inspect,
    Choose,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Status {
    Ok,
    InvalidInput,
    Unsupported,
    Unavailable,
    Busy,
    Conflict,
    Failed,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expected {
    actor: Uuid,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    operation: Uuid,
    expected: Expected,
    choice: ChoiceInput,
}
// A struct-shaped empty variant rejects extra fields; serde's internally
// tagged unit variant otherwise ignores them even with deny_unknown_fields.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum ChoiceInput {
    LocalOnly {},
    IsolatedBackup {
        binding_sha256: String,
    },
    SharedGroup {
        group: Uuid,
        binding_sha256: String,
        root_id: String,
    },
}
impl From<ChoiceInput> for mode::SyncModeChoice {
    fn from(value: ChoiceInput) -> Self {
        match value {
            ChoiceInput::LocalOnly {} => Self::LocalOnly,
            ChoiceInput::IsolatedBackup { binding_sha256 } => {
                Self::IsolatedBackup { binding_sha256 }
            }
            ChoiceInput::SharedGroup {
                group,
                binding_sha256,
                root_id,
            } => Self::SharedGroup {
                group,
                binding_sha256,
                root_id,
            },
        }
    }
}
struct Validated {
    operation: Uuid,
    expected: SyncModeToken,
    choice: mode::SyncModeChoice,
}
impl Request {
    fn validate(self) -> Result<Validated> {
        ensure!(!self.operation.is_nil(), "invalid mode operation");
        let choice = self.choice.into();
        mode::validate_choice(&choice)?;
        Ok(Validated {
            operation: self.operation,
            expected: SyncModeToken::from_maintenance_request(
                self.expected.actor,
                self.expected.sha256,
            )?,
            choice,
        })
    }
}

#[derive(Serialize)]
struct Current {
    device: Option<Uuid>,
    epoch: Option<Uuid>,
    mode: Option<mode::SyncMode>,
    token: SyncModeToken,
}
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum ReceiptStatus {
    Current,
    Historical,
}
#[derive(Serialize)]
struct Data {
    current_snapshot: Current,
    receipt: Option<SyncModeReceipt>,
    receipt_status: Option<ReceiptStatus>,
}

struct OutputLease {
    _ordinary: Option<OrdinaryModeLease>,
    _explicit: Option<SyncModeLease>,
}
impl OutputLease {
    fn acquire(store: &Arc<Store>, current: &SyncModeSnapshot) -> Result<Self> {
        if matches!(current.mode, None | Some(mode::SyncMode::LocalOnly)) {
            Ok(Self {
                _ordinary: Some(store.ordinary_mode_lease(&current.token)?),
                _explicit: None,
            })
        } else {
            Ok(Self {
                _ordinary: None,
                _explicit: Some(store.sync_mode_lease(&current.token)?),
            })
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
    _store: Option<Arc<Store>>,
    #[serde(skip)]
    _mode: Option<OutputLease>,
}
impl Report {
    fn new(operation: Operation, status: Status) -> Self {
        Self {
            report_version: 1,
            operation,
            status,
            incomplete: matches!(status, Status::Unavailable | Status::Failed),
            activation_performed: false,
            data: None,
            _store: None,
            _mode: None,
        }
    }
    fn encoded(&self) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        if bytes.len() <= MAX_OUTPUT {
            return Ok(bytes);
        }
        let mut bytes = serde_json::to_vec(&Self::new(self.operation, Status::Unavailable))?;
        bytes.push(b'\n');
        Ok(bytes)
    }
    /// Consuming output: no second report or automatic mutation retry on I/O failure.
    pub fn write_to(self, writer: &mut impl Write) -> Result<()> {
        let bytes = self.encoded()?;
        writer.write_all(&bytes)?;
        Ok(())
    }
}

fn read_request(path: &Path, stdin: &mut impl Read) -> Result<Validated> {
    let request: Request = if path.as_os_str() == std::ffi::OsStr::new("-") {
        let mut bytes = Vec::new();
        stdin.take((MAX_INPUT + 1) as u64).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= MAX_INPUT, "mode input exceeds bound");
        serde_json::from_slice(&bytes)?
    } else {
        files::json(path, MAX_INPUT as u64)?
    };
    request.validate()
}

fn mutation_status(error: &anyhow::Error) -> Status {
    // Exact trusted setter errors only. Never infer a category from a substring.
    match error.to_string().as_str() {
        "stale mode token" | "mode operation collision" => Status::Conflict,
        "invalid mode request actor/operation" | "mode namespace identity collision" => {
            Status::InvalidInput
        }
        "stop existing publisher leases before mode change" => Status::Busy,
        _ => Status::Failed,
    }
}

fn finish(
    operation: Operation,
    status: Status,
    store: Arc<Store>,
    receipt: Option<SyncModeReceipt>,
) -> Report {
    let mut report = Report::new(operation, status);
    report._store = Some(store.clone());
    let current = match store.sync_mode_snapshot() {
        Ok(current) => current,
        Err(_) => {
            report.status = Status::Unsupported;
            report.incomplete = true;
            return report;
        }
    };
    let lease = match OutputLease::acquire(&store, &current) {
        Ok(lease) => lease,
        Err(_) => {
            report.status = Status::Unavailable;
            report.incomplete = true;
            return report;
        }
    };
    let receipt_status = receipt.as_ref().map(|receipt| {
        if Some(receipt.epoch) == current.epoch && current.mode.as_ref() == Some(&receipt.mode) {
            ReceiptStatus::Current
        } else {
            ReceiptStatus::Historical
        }
    });
    report._mode = Some(lease);
    report.data = Some(Data {
        current_snapshot: Current {
            device: current.device,
            epoch: current.epoch,
            mode: current.mode,
            token: current.token,
        },
        receipt,
        receipt_status,
    });
    report
}

fn choose(store: Arc<Store>, request: Validated) -> Report {
    match store.set_sync_mode(&request.expected, request.operation, request.choice) {
        Ok(receipt) => finish(Operation::Choose, Status::Ok, store, Some(receipt)),
        Err(error) => finish(Operation::Choose, mutation_status(&error), store, None),
    }
}

pub fn run(
    operation: Operation,
    input: Option<&Path>,
    config_path: Option<PathBuf>,
    stdin: &mut impl Read,
) -> Report {
    let request = match (operation, input) {
        (Operation::Choose, Some(path)) => match read_request(path, stdin) {
            Ok(request) => Some(request),
            Err(_) => return Report::new(operation, Status::InvalidInput),
        },
        (Operation::Choose, None) => return Report::new(operation, Status::InvalidInput),
        (_, Some(_)) => return Report::new(operation, Status::InvalidInput),
        (_, None) => None,
    };
    let Some(path) = config_path else {
        return Report::new(operation, Status::Unavailable);
    };
    let config = match Config::load(Some(path)) {
        Ok(config) => config,
        Err(_) => return Report::new(operation, Status::Unavailable),
    };
    if operation != Operation::Inspect && config.sync.enabled {
        return Report::new(operation, Status::Unsupported);
    }
    let store = if operation == Operation::Initialize {
        match Store::open(config.paths) {
            Ok(store) => store,
            Err(error) => {
                return Report::new(
                    operation,
                    if error.to_string() == "store is owned by another process" {
                        Status::Busy
                    } else {
                        Status::Unavailable
                    },
                )
            }
        }
    } else {
        match Store::open_existing_for_mode(config.paths) {
            Ok(store) => store,
            Err(refusal) => {
                return Report::new(
                    operation,
                    match refusal {
                        Refusal::Busy => Status::Busy,
                        Refusal::Unsupported => Status::Unsupported,
                        Refusal::Unavailable => Status::Unavailable,
                    },
                )
            }
        }
    };
    let store = Arc::new(store);
    if store.unavailable_commits().ok() != Some(0) {
        let mut report = Report::new(operation, Status::Unavailable);
        report._store = Some(store);
        return report;
    }
    if let Some(request) = request {
        choose(store, request)
    } else {
        finish(operation, Status::Ok, store, None)
    }
}

/// Real ordinary-runtime lifetime, not a snapshot-only startup decision.
pub struct OrdinaryLifetime {
    _mode: OrdinaryModeLease,
    _data: storage::LegacySyncLease,
}
pub fn admit_ordinary(store: &Arc<Store>, legacy_sync_enabled: bool) -> Result<OrdinaryLifetime> {
    let current = store.sync_mode_snapshot()?;
    let lease = store.ordinary_mode_lease(&current.token)?;
    ensure!(
        !(current.mode.is_some() && legacy_sync_enabled),
        "explicit local mode requires legacy sync disabled"
    );
    let data = store.legacy_sync_lease()?;
    Ok(OrdinaryLifetime {
        _mode: lease,
        _data: data,
    })
}

#[cfg(test)]
mod tests;

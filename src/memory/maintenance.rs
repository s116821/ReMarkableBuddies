//! One-shot, bounded local maintenance. No provider, sync worker or device initialization.
use super::{Forget, Key, LocalMemory, Mutation, Provider, Query, Status};
use crate::{
    config::Config,
    storage::{self, files, Store, Uuid, MAX_ITEMS, MAX_METADATA, MAX_RECORD},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    sync::Arc,
};

pub const MAX_RESPONSE: usize = MAX_METADATA + 64 * 1024;

#[derive(Clone, Copy, Debug, clap::ValueEnum, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    Inspect,
    Query,
    Put,
    Forget,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ReportStatus {
    Ok,
    Partial,
    Busy,
    Unavailable,
    Unsupported,
    Conflict,
    InvalidInput,
    Failed,
}
#[derive(Serialize)]
pub struct Report {
    pub report_version: u32,
    pub operation: Operation,
    pub status: ReportStatus,
    pub incomplete: bool,
    pub data: Option<serde_json::Value>,
}
impl Report {
    fn new(operation: Operation, status: ReportStatus) -> Self {
        Self {
            report_version: 1,
            operation,
            status,
            incomplete: false,
            data: None,
        }
    }
    /// Bound the entire encoded report, including data/escaping/wrapper. Never retry a write.
    pub fn encoded(&self) -> anyhow::Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        if bytes.len() <= MAX_RESPONSE {
            return Ok(bytes);
        }
        let mut limited = Self::new(self.operation, ReportStatus::Unavailable);
        limited.incomplete = true;
        let mut bytes = serde_json::to_vec(&limited)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}
enum Input {
    Inspect(Key),
    Query(Query),
    Put(Mutation),
    Forget(Forget),
}
impl Input {
    fn read(operation: Operation, path: &Path, profile: Uuid) -> anyhow::Result<Self> {
        let input = match operation {
            Operation::Inspect => Self::Inspect(files::json(path, MAX_RECORD as u64)?),
            Operation::Query => Self::Query(files::json(path, MAX_RECORD as u64)?),
            Operation::Put => Self::Put(files::json(path, MAX_RECORD as u64)?),
            Operation::Forget => Self::Forget(files::json(path, MAX_RECORD as u64)?),
        };
        match &input {
            Self::Inspect(key) => {
                key.validate()?;
                anyhow::ensure!(key.profile == profile, "foreign profile");
            }
            Self::Query(query) => query.validate()?,
            Self::Put(mutation) => {
                mutation.fact.validate()?;
                anyhow::ensure!(mutation.fact.key.profile == profile, "foreign profile");
                valid_operation(mutation.operation, &mutation.expected)?;
            }
            Self::Forget(mutation) => {
                mutation.key.validate()?;
                anyhow::ensure!(mutation.key.profile == profile, "foreign profile");
                valid_operation(mutation.operation, &mutation.expected)?;
                anyhow::ensure!(
                    !mutation.expected.is_empty(),
                    "forget requires explicit heads"
                );
            }
        }
        Ok(input)
    }
}
fn valid_operation(
    operation: Uuid,
    expected: &std::collections::BTreeSet<Uuid>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !operation.is_nil()
            && expected.len() <= MAX_ITEMS
            && expected.iter().all(|id| !id.is_nil()),
        "invalid memory operation"
    );
    let revision = Uuid::new_v5(&operation, b"remarkable-buddies:subject-memory:revision:v1");
    anyhow::ensure!(
        !expected.contains(&revision),
        "invalid self-parent memory operation"
    );
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    format: u32,
    actor: Uuid,
}
fn regular(path: &Path) -> bool {
    files::safe_path(path).is_ok() && fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
}
/// Pure state/path and nonblocking lock admission before normal Store startup.
fn preflight(config: &Config, allow_initialization: bool) -> Result<(), ReportStatus> {
    config
        .paths
        .validate()
        .map_err(|_| ReportStatus::Unsupported)?;
    let root = &config.paths.data;
    let fresh = match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => return Err(ReportStatus::Unavailable),
        Ok(metadata) => {
            if !metadata.is_dir() {
                return Err(ReportStatus::Unsupported);
            }
            fs::read_dir(root)
                .map_err(|_| ReportStatus::Unavailable)?
                .next()
                .is_none()
        }
    };
    if fresh {
        return if allow_initialization {
            Ok(())
        } else {
            Err(ReportStatus::Unavailable)
        };
    }
    let identity = root.join("identity.json");
    let current = root.join("CURRENT");
    let lock = root.join("store.lock");
    if ![&identity, &current, &lock]
        .iter()
        .all(|path| regular(path))
    {
        return Err(ReportStatus::Unavailable);
    }
    let marker: Marker = files::json(&identity, 1024).map_err(|_| ReportStatus::Unsupported)?;
    if marker.format != 1 || marker.actor.is_nil() {
        return Err(ReportStatus::Unsupported);
    }
    let generation: Uuid = files::json(&current, 128).map_err(|_| ReportStatus::Unsupported)?;
    if generation.is_nil() {
        return Err(ReportStatus::Unsupported);
    }
    let generation_root = root.join("generations").join(generation.to_string());
    for directory in [
        &generation_root,
        &generation_root.join("objects"),
        &generation_root.join("commits"),
    ] {
        if files::safe_path(directory).is_err()
            || !fs::symlink_metadata(directory).is_ok_and(|m| m.is_dir())
        {
            return Err(ReportStatus::Unavailable);
        }
    }
    if !regular(&generation_root.join("format.json")) {
        return Err(ReportStatus::Unavailable);
    }
    storage::selection::validate_generation(&generation_root, marker.actor, generation)
        .map_err(|_| ReportStatus::Unsupported)?;
    let handle = File::open(lock).map_err(|_| ReportStatus::Unavailable)?;
    match handle.try_lock() {
        Ok(()) => Ok(()),
        Err(std::fs::TryLockError::WouldBlock) => Err(ReportStatus::Busy),
        Err(std::fs::TryLockError::Error(_)) => Err(ReportStatus::Unsupported),
    }
    // The preflight handle drops before normal Store reacquisition; no unleased read follows.
}

pub fn run(operation: Operation, input_path: &Path, config_path: Option<PathBuf>) -> Report {
    let Some(config_path) = config_path else {
        return Report::new(operation, ReportStatus::Unavailable);
    };
    let config = match Config::load(Some(config_path)) {
        Ok(config) => config,
        Err(_) => return Report::new(operation, ReportStatus::Unavailable),
    };
    let Some(profile) = config.memory.profile else {
        return Report::new(operation, ReportStatus::Unavailable);
    };
    let input = match Input::read(operation, input_path, profile) {
        Ok(input) => input,
        Err(_) => return Report::new(operation, ReportStatus::InvalidInput),
    };
    let allow_initialization =
        matches!(&input, Input::Put(mutation) if mutation.expected.is_empty());
    if let Err(status) = preflight(&config, allow_initialization) {
        return Report::new(operation, status);
    }
    let store = match Store::open(config.paths) {
        Ok(store) => Arc::new(store),
        Err(error) => return Report::new(operation, store_open_status(&error)),
    };
    let memory = match LocalMemory::new_local(store, profile) {
        Ok(memory) => memory,
        Err(_) => return Report::new(operation, ReportStatus::Unsupported),
    };
    let mut report = Report::new(operation, ReportStatus::Ok);
    let result = match input {
        Input::Inspect(key) => memory.inspect(&key).and_then(|envelopes| Ok(serde_json::to_value(envelopes)?)),
        Input::Query(query) => memory.query(&query).map(|result| {
            report.status = match result.status { Status::Partial => ReportStatus::Partial, Status::Unavailable => ReportStatus::Unavailable, _ => ReportStatus::Ok };
            report.incomplete = matches!(result.status, Status::Partial | Status::Unavailable);
            serde_json::json!({"query_status":result.status,"diagnostics":result.diagnostics,"context":result.context})
        }),
        Input::Put(mutation) => memory.put(mutation).map(|receipt| serde_json::json!({"transaction":receipt})),
        Input::Forget(mutation) => memory.forget(mutation).map(|receipt| serde_json::json!({"transaction":receipt})),
    };
    match result {
        Ok(data) => {
            report.data = Some(data);
            report
        }
        Err(error) => {
            if let Some(conflict) = error.downcast_ref::<storage::Conflict>() {
                if conflict.heads.len() > MAX_ITEMS {
                    report.status = ReportStatus::Unavailable;
                    report.incomplete = true;
                } else {
                    report.status = ReportStatus::Conflict;
                    report.data = Some(serde_json::json!({"current_heads":conflict.heads}));
                }
            } else {
                report.status = ReportStatus::Unavailable;
                report.incomplete = true;
            }
            report
        }
    }
}

// Called only for the trusted Store::open stage, never parser/user errors.
fn store_open_status(error: &anyhow::Error) -> ReportStatus {
    if error.to_string() == "store is owned by another process" {
        ReportStatus::Busy
    } else {
        ReportStatus::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_reacquire_refusal_and_exact_error_classifier_never_match_substrings() {
        let root = std::env::temp_dir().join(format!("memory-lock-classifier-{}", Uuid::new_v4()));
        let paths = storage::StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        };
        let held = Store::open(paths.clone()).unwrap();
        let error = match Store::open(paths) {
            Err(error) => error,
            Ok(_) => panic!("exclusive held Store must refuse"),
        };
        assert_eq!(store_open_status(&error), ReportStatus::Busy);
        for text in [
            "private parser value: store is owned by another process",
            "store is owned by another process: private suffix",
            "unsupported locking",
        ] {
            assert_eq!(
                store_open_status(&anyhow::anyhow!(text.to_owned())),
                ReportStatus::Unavailable
            );
        }
        drop(held);
        assert!(root.starts_with(std::env::temp_dir()));
        let _ = fs::remove_dir_all(root);
    }
}

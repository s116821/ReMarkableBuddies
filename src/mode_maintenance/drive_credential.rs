//! Fixed local Drive grant possession, without OAuth or publisher activation.
use super::{Operation, Report as ModeReport, Status as ModeStatus, Uuid, MAX_OUTPUT};
use anyhow::{ensure, Result};
use serde::Serialize;
use std::{
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Serialize)]
pub(super) struct Report {
    report_version: u32,
    action: Option<&'static str>,
    operation: Option<Uuid>,
    outcome: &'static str,
    current: Option<serde_json::Value>,
    disk_readback_verified: bool,
}

impl Report {
    fn new(action: Option<&'static str>, operation: Option<Uuid>, outcome: &'static str) -> Self {
        Self {
            report_version: 1,
            action,
            operation,
            outcome,
            current: None,
            disk_readback_verified: false,
        }
    }

    pub(super) fn encoded(&self) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        ensure!(
            bytes.len() <= MAX_OUTPUT,
            "Drive credential report exceeds bound"
        );
        Ok(bytes)
    }
}

pub(super) fn run(
    input: Option<&Path>,
    config_path: Option<PathBuf>,
    stdin: &mut impl Read,
) -> ModeReport {
    let mut report = ModeReport::new(Operation::DriveCredential, ModeStatus::Unavailable);
    if input != Some(Path::new("-")) {
        report.drive_credential = Some(Report::new(None, None, "invalid-input"));
        return report;
    }
    #[cfg(target_os = "linux")]
    {
        linux::run(report, config_path, stdin)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (config_path, stdin);
        report.drive_credential = Some(Report::new(None, None, "unsupported"));
        report
    }
}

#[cfg(target_os = "linux")]
mod linux;

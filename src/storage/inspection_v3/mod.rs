//! Explicit report3 observer. Never constructs Store or invokes recovery.
#[cfg(target_os = "linux")]
mod linux;
mod report;
#[cfg(target_os = "linux")]
mod scan;

use super::inspection::InspectionStatus;
use super::StorePaths;
use report::Report;
use std::io::{self, Write};

/// Owns existing-only custody until the consuming output completes.
pub struct Inspection {
    report: Report,
    #[cfg(target_os = "linux")]
    custody: Option<(linux::Access, std::fs::File)>,
}
impl Inspection {
    pub fn configuration_unavailable() -> Self {
        Self::failure(InspectionStatus::Unavailable, "configuration-unavailable")
    }
    fn failure(status: InspectionStatus, code: &'static str) -> Self {
        Self {
            report: Report::failure(status, code),
            #[cfg(target_os = "linux")]
            custody: None,
        }
    }
    /// One output attempt. Any partial write or changed final witness is an error;
    /// callers must not append a replacement frame or repeat an operation.
    #[allow(unused_mut)] // Non-Linux builds have only the immutable refusal frame.
    pub fn write_to(mut self, output: &mut impl Write) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        if let Some((access, _lease)) = &mut self.custody {
            if access.verify().is_err() {
                self.report.status = InspectionStatus::Partial;
                self.report.complete_apparent_bytes = None;
                self.report.domain_accounting_complete = false;
                self.report.issue("state-changed");
            }
        }
        self.report.write_to(output)?;
        #[cfg(target_os = "linux")]
        if let Some((access, _lease)) = &mut self.custody {
            access
                .verify()
                .map_err(|_| io::Error::other("inspection witness changed during output"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

pub fn inspect_owned_data(paths: &StorePaths) -> Inspection {
    #[cfg(target_os = "linux")]
    {
        scan::inspect(paths)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = paths;
        Inspection::failure(InspectionStatus::Unsupported, "unsupported-platform")
    }
}

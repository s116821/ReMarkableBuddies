//! Confidential one-shot maintenance of the already declared credential file.
use super::basic::{Custody, Status};
use anyhow::{ensure, Result};
use serde::Serialize;
use std::{
    io::{Read, Write},
    path::PathBuf,
};

#[cfg(target_os = "linux")]
pub(crate) mod linux;
pub const MAX_IO: usize = 16_384;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    Inspect,
    Prepare,
    Apply,
    Status,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    ReadOnly,
    Prepared,
    NoChange,
    Verified,
    DesiredFileObserved,
    Unresolved,
    NotDispatched,
    Unknown,
}
/// This entire report is confidential transport, including secret-derived hashes.
#[derive(Serialize)]
pub struct Report {
    report_version: u32,
    operation: Operation,
    pub status: Status,
    outcome: Outcome,
    disk_readback_verified: bool,
    runtime_activation: &'static str,
    data: Option<serde_json::Value>,
    #[serde(skip)]
    _custody: Option<Custody>,
}
impl Report {
    pub(crate) fn refusal(operation: Operation, status: Status) -> Self {
        Self {
            report_version: 1,
            operation,
            status,
            outcome: if matches!(operation, Operation::Inspect | Operation::Status) {
                Outcome::ReadOnly
            } else {
                Outcome::NotDispatched
            },
            disk_readback_verified: false,
            runtime_activation: "unverified",
            data: None,
            _custody: None,
        }
    }
    #[cfg(target_os = "linux")]
    fn success(operation: Operation, outcome: Outcome, data: serde_json::Value) -> Self {
        let mut report = Self::refusal(operation, Status::Ok);
        report.outcome = outcome;
        report.data = Some(data);
        report
    }
    pub fn write_to(self, writer: &mut impl Write) -> Result<()> {
        let mut bytes = serde_json::to_vec(&self)?;
        bytes.push(b'\n');
        ensure!(bytes.len() <= MAX_IO, "credential report exceeds bound");
        writer.write_all(&bytes)?;
        Ok(())
    }
}
pub fn invalid_input(operation: Operation) -> Report {
    Report::refusal(operation, Status::InvalidInput)
}
pub fn unsupported_input(operation: Operation) -> Report {
    Report::refusal(operation, Status::Unsupported)
}
pub fn inspect(path: Option<PathBuf>) -> Report {
    #[cfg(target_os = "linux")]
    {
        linux::inspect(path)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        unsupported_input(Operation::Inspect)
    }
}
pub fn run_stdin(operation: Operation, path: Option<PathBuf>, input: &mut impl Read) -> Report {
    #[cfg(target_os = "linux")]
    {
        linux::run(operation, path, input)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (path, input);
        unsupported_input(operation)
    }
}

#[cfg(all(test, not(target_os = "linux")))]
mod unsupported_tests {
    use super::*;
    struct Never;
    impl Read for Never {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("unsupported target read input")
        }
    }
    #[test]
    fn unsupported_target_never_reads_input_or_paths() {
        for op in [Operation::Prepare, Operation::Apply, Operation::Status] {
            let r = run_stdin(op, Some("/never/probe".into()), &mut Never);
            assert_eq!(r.status, Status::Unsupported);
        }
        assert_eq!(
            inspect(Some("/never/probe".into())).status,
            Status::Unsupported
        );
    }
}

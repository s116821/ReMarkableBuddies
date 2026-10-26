//! Bounded PID1 observations and the recovery module's fixed stock-start command.
use super::{validate_invocation, validate_unit, GroupObservation, Lease, LeaseRecord};
use anyhow::{ensure, Context, Result};
use nix::libc;
use std::{
    collections::BTreeMap,
    io::Read,
    os::fd::AsRawFd,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const LIMIT: usize = 16 * 1024;
const POLL: Duration = Duration::from_millis(5);
pub(super) const STATE: &[&str] = &[
    "Id",
    "LoadState",
    "ActiveState",
    "SubState",
    "MainPID",
    "ControlPID",
    "ExecMainPID",
    "ExecMainStartTimestampMonotonic",
    "InvocationID",
    "Job",
    "ControlGroup",
];
pub(super) const POLICY: &[&str] = &[
    "Type",
    "NeedDaemonReload",
    "KillMode",
    "Delegate",
    "Restart",
    "RestartMode",
    "RestartForceExitStatus",
    "FragmentPath",
    "DropInPaths",
    "OnSuccess",
    "OnFailure",
    "Conflicts",
    "After",
    "DefaultDependencies",
    "SendSIGKILL",
    "WatchdogUSec",
    "TimeoutStopUSec",
    "TimeoutStartUSec",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    fields: BTreeMap<String, String>,
}

/// Exact selected policy values. The installed profile must independently verify
/// them and the unit/executable bytes; copying a live snapshot is not approval.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicySnapshot(BTreeMap<String, String>);
impl PolicySnapshot {
    pub fn property(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }
}

impl Snapshot {
    pub(super) fn property(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }
    pub(super) fn parse(bytes: &[u8], unit: &str) -> Result<Self> {
        validate_unit(unit)?;
        ensure!(bytes.len() <= LIMIT, "service output exceeds bound");
        let input = std::str::from_utf8(bytes)?;
        ensure!(
            !input.bytes().any(|c| c.is_ascii_control() && c != b'\n'),
            "control byte in service output"
        );
        let mut fields = BTreeMap::new();
        for line in input.lines() {
            let (key, value) = line.split_once('=').context("invalid service property")?;
            ensure!(
                STATE.contains(&key) || POLICY.contains(&key),
                "unknown service property"
            );
            ensure!(
                fields.insert(key.to_owned(), value.to_owned()).is_none(),
                "duplicate service property"
            );
        }
        ensure!(
            fields.len() == STATE.len() + POLICY.len(),
            "missing service property"
        );
        ensure!(fields["Id"] == unit, "service identity mismatch");
        for key in ["MainPID", "ControlPID", "ExecMainPID"] {
            let value = fields[key].parse::<u32>()?;
            ensure!(value.to_string() == fields[key], "noncanonical service PID");
        }
        let start = fields["ExecMainStartTimestampMonotonic"].parse::<u64>()?;
        ensure!(
            start.to_string() == fields["ExecMainStartTimestampMonotonic"],
            "noncanonical service timestamp"
        );
        if !fields["InvocationID"].is_empty() {
            validate_invocation(&fields["InvocationID"])?;
        }
        Ok(Self { fields })
    }
    pub fn policy(&self) -> PolicySnapshot {
        PolicySnapshot(
            POLICY
                .iter()
                .map(|key| ((*key).to_owned(), self.fields[*key].clone()))
                .collect(),
        )
    }
    pub fn main_pid(&self) -> u32 {
        self.fields["MainPID"].parse().expect("parsed PID")
    }
    pub fn invocation(&self) -> &str {
        &self.fields["InvocationID"]
    }
    pub fn start_micros(&self) -> u64 {
        self.fields["ExecMainStartTimestampMonotonic"]
            .parse()
            .expect("parsed timestamp")
    }
    pub fn control_group(&self) -> &str {
        &self.fields["ControlGroup"]
    }

    fn terminal(&self, record: &LeaseRecord, group: GroupObservation) -> Result<TerminalIdentity> {
        record.validate()?;
        let f = &self.fields;
        ensure!(
            f["Id"] == record.group.unit && f["LoadState"] == "loaded",
            "original loaded Buddy unit required"
        );
        ensure!(
            f["MainPID"] == "0" && f["ControlPID"] == "0" && f["Job"].is_empty(),
            "Buddy still running or has pending job"
        );
        ensure!(
            f["KillMode"] == "control-group"
                && f["Delegate"] == "no"
                && f["Restart"] == "no"
                && f["RestartForceExitStatus"].is_empty()
                && f["SendSIGKILL"] == "yes"
                && f["DefaultDependencies"] == "yes",
            "unsafe recovery service policy"
        );
        let original = format!("/system.slice/{}", record.group.unit);
        ensure!(
            f["ControlGroup"] == original
                || (f["ControlGroup"].is_empty() && group == GroupObservation::OriginalAbsent),
            "service control group changed"
        );
        ensure!(
            group != GroupObservation::Populated,
            "original group still populated"
        );
        if self.invocation() == record.buddy_invocation
            && f["ExecMainPID"] == record.group.parent.pid.to_string()
            && self.start_micros() == record.buddy_start_micros
        {
            ensure!(
                (f["ActiveState"] == "inactive" && f["SubState"] == "dead")
                    || (f["ActiveState"] == "failed" && f["SubState"] == "failed"),
                "Buddy not terminal"
            );
            return Ok(TerminalIdentity::Retained);
        }
        ensure!(
            self.invocation().is_empty()
                && f["ExecMainPID"] == "0"
                && self.start_micros() == 0
                && f["ActiveState"] == "inactive"
                && f["SubState"] == "dead"
                && group == GroupObservation::OriginalAbsent,
            "service incarnation changed or incompletely cleared"
        );
        Ok(TerminalIdentity::Cleared)
    }
}

/// Still requires independently qualified deletion semantics, an enforced common
/// activation latch, verified installed policy and the recovery helper's context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalIdentity {
    Retained,
    Cleared,
}

pub struct RecoveryObservation {
    pub identity: TerminalIdentity,
    pub group: GroupObservation,
    /// Compare these exact bracketed values with the verified installed profile.
    pub policy: PolicySnapshot,
}

/// Read only. Service sampling and the group lookup share the caller deadline.
/// No result from this function grants permission to start stock by itself.
pub fn observe_recovery(lease: &Lease, deadline: Instant) -> Result<RecoveryObservation> {
    lease.current()?;
    let before = observe(lease.record().group().unit(), deadline)?;
    let group = lease.record().group().observe()?;
    let after = observe(lease.record().group().unit(), deadline)?;
    ensure!(
        Instant::now() < deadline,
        "recovery observation deadline expired"
    );
    lease.current()?;
    let identity = bracket(lease.record(), &before, group, &after)?;
    ensure!(
        Instant::now() < deadline,
        "recovery observation deadline expired"
    );
    Ok(RecoveryObservation {
        identity,
        group,
        policy: before.policy(),
    })
}

fn bracket(
    record: &LeaseRecord,
    before: &Snapshot,
    group: GroupObservation,
    after: &Snapshot,
) -> Result<TerminalIdentity> {
    let first = before.terminal(record, group)?;
    let last = after.terminal(record, group)?;
    ensure!(
        before == after && first == last,
        "service changed across group observation"
    );
    Ok(first)
}

pub fn observe(unit: &str, deadline: Instant) -> Result<Snapshot> {
    validate_unit(unit)?;
    let bytes = run(command(unit), deadline)?;
    Snapshot::parse(&bytes, unit)
}
fn command(unit: &str) -> Command {
    let mut command = Command::new("/bin/systemctl");
    command.args([
        "--system",
        "--no-pager",
        "--no-ask-password",
        "--all",
        "show",
    ]);
    for property in STATE.iter().chain(POLICY) {
        command.arg(format!("--property={property}"));
    }
    command.args(["--", unit]);
    command
}

/// Only the installed recovery admission path may request this mutation. A
/// command failure does not imply that PID1 rejected or canceled the start job.
pub(super) fn start_stock(deadline: Instant) -> Result<()> {
    let output = run(start_command(), deadline)?;
    ensure!(output.is_empty(), "unexpected stock-start command output");
    Ok(())
}
/// Only the installed activation admission path may stop stock. Failure may
/// leave a queued stop; its caller must exit so independent recovery can observe it.
pub(super) fn stop_stock(deadline: Instant) -> Result<()> {
    let output = run(stop_command(), deadline)?;
    ensure!(output.is_empty(), "unexpected stock-stop command output");
    Ok(())
}
fn stop_command() -> Command {
    let mut command = Command::new("/bin/systemctl");
    command.args([
        "--system",
        "--no-pager",
        "--no-ask-password",
        "--no-block",
        "--job-mode=fail",
        "stop",
        "--",
        "xochitl.service",
    ]);
    command
}
fn start_command() -> Command {
    let mut command = Command::new("/bin/systemctl");
    command.args([
        "--system",
        "--no-pager",
        "--no-ask-password",
        "--no-block",
        "--job-mode=fail",
        "start",
        "--",
        "xochitl.service",
    ]);
    command
}

#[derive(Debug)]
pub struct CommandFailure {
    pub pid: u32,
    pub reaped: bool,
    pub error: anyhow::Error,
}
impl std::fmt::Display for CommandFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "service observation failed: {}; child={}, reaped={}",
            self.error, self.pid, self.reaped
        )
    }
}
impl std::error::Error for CommandFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.error.as_ref())
    }
}

fn nonblocking(stream: &impl AsRawFd) -> Result<()> {
    let flags = unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_GETFL) };
    ensure!(flags >= 0, "pipe flags unavailable");
    ensure!(
        unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } == 0,
        "nonblocking pipe setup failed"
    );
    Ok(())
}
fn drain(stream: &mut impl Read, bytes: &mut Vec<u8>) -> Result<bool> {
    let mut chunk = [0; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(true),
            Ok(count) => {
                ensure!(bytes.len() + count <= LIMIT, "command output exceeds bound");
                bytes.extend_from_slice(&chunk[..count]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) => return Err(error.into()),
        }
    }
}
fn reap(child: &mut Child) -> bool {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return true;
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    let _ = child.kill();
    loop {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
}
fn run(mut command: Command, deadline: Instant) -> Result<Vec<u8>> {
    let caller_deadline = deadline;
    let deadline = deadline.min(Instant::now() + Duration::from_secs(3));
    let check = || -> Result<()> {
        if deadline == caller_deadline {
            crate::workflow::resident_request::before(
                deadline,
                "service observation deadline expired",
            )
        } else {
            ensure!(
                Instant::now() < deadline,
                "service observation internal timeout"
            );
            Ok(())
        }
    };
    check()?;
    let mut child = command
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("SYSTEMD_COLORS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let result = (|| -> Result<Vec<u8>> {
        let mut out = child.stdout.take().context("missing stdout pipe")?;
        let mut err = child.stderr.take().context("missing stderr pipe")?;
        nonblocking(&out)?;
        nonblocking(&err)?;
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        loop {
            check()?;
            let out_eof = drain(&mut out, &mut stdout)?;
            let err_eof = drain(&mut err, &mut stderr)?;
            ensure!(stderr.is_empty(), "service command wrote stderr");
            if let Some(status) = child.try_wait()? {
                ensure!(status.success(), "service command exited unsuccessfully");
                if out_eof && err_eof {
                    check()?;
                    return Ok(stdout);
                }
            }
            thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
        }
    })();
    result.map_err(|error| {
        CommandFailure {
            pid: child.id(),
            reaped: reap(&mut child),
            error,
        }
        .into()
    })
}

#[cfg(test)]
mod tests;

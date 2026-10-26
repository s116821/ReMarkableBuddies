//! Read-only original-service evidence, not a service lease or start authority.
use super::*;
use crate::workflow::resident_supervisor::GroupBaseline;
use serde::{Deserialize, Serialize};
use std::path::Path;

const RECORD_LIMIT: usize = 64 * 1024;

/// The caller must protect/bind serialized records, qualify deletion semantics,
/// and exclude migration/concurrent starts before using terminal observations.
pub struct ServiceBaseline {
    original: Snapshot,
    group: GroupBaseline,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    snapshot: String,
    group: GroupBaseline,
}

impl ServiceBaseline {
    pub fn capture(mount: &Path, unit: &str, deadline: Instant) -> Result<Self> {
        before(deadline)?;
        let original = observe(unit, deadline)?;
        let group = GroupBaseline::capture_process(mount, unit, original.main_pid())?;
        let result = Self { original, group };
        result.validate()?;
        result.current(deadline)?;
        Ok(result)
    }

    pub fn original(&self) -> &Snapshot {
        &self.original
    }

    pub fn group(&self) -> &GroupBaseline {
        &self.group
    }

    pub fn current(&self, deadline: Instant) -> Result<()> {
        self.validate()?;
        before(deadline)?;
        ensure!(
            observe(self.group.unit(), deadline)? == self.original,
            "original service changed"
        );
        self.group.current_process()?;
        ensure!(
            observe(self.group.unit(), deadline)? == self.original,
            "service changed across process observation"
        );
        before(deadline)
    }

    /// None means the original incarnation is still active/stopping. A returned
    /// observation requires the caller's independent policy/ownership admission.
    pub fn stopped(&self, deadline: Instant) -> Result<Option<RecoveryObservation>> {
        self.validate()?;
        before(deadline)?;
        let first = observe(self.group.unit(), deadline)?;
        let group = self.group.observe()?;
        let last = observe(self.group.unit(), deadline)?;
        let result = stopped_bracket(&self.original, &first, group, &last, deadline)?;
        before(deadline)?;
        Ok(result)
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let record = Record {
            version: 1,
            snapshot: self
                .original
                .fields
                .iter()
                .map(|(k, v)| format!("{k}={v}\n"))
                .collect(),
            group: self.group.clone(),
        };
        let bytes = serde_json::to_vec(&record)?;
        ensure!(bytes.len() <= RECORD_LIMIT, "service record exceeds bound");
        Ok(bytes)
    }

    /// Structural validation only. This does not authenticate supplied bytes or
    /// turn a historical observation into current process/restore authority.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() <= RECORD_LIMIT, "service record exceeds bound");
        let record: Record = serde_json::from_slice(bytes)?;
        ensure!(record.version == 1, "unsupported service record version");
        let result = Self {
            original: Snapshot::parse(record.snapshot.as_bytes(), record.group.unit())?,
            group: record.group,
        };
        result.validate()?;
        Ok(result)
    }

    fn validate(&self) -> Result<()> {
        self.group.validate()?;
        let s = &self.original;
        for (key, value) in [
            ("Id", self.group.unit()),
            ("LoadState", "loaded"),
            ("ActiveState", "active"),
            ("SubState", "running"),
            ("ControlPID", "0"),
            ("Job", ""),
        ] {
            ensure!(
                s.property(key) == Some(value),
                "original service state mismatch: {key}"
            );
        }
        validate_invocation(s.invocation())?;
        ensure!(
            s.main_pid() == self.group.parent().pid
                && s.property("ExecMainPID") == Some(s.main_pid().to_string().as_str())
                && s.start_micros() != 0
                && s.control_group() == format!("/system.slice/{}", self.group.unit()),
            "original service/process/group mismatch"
        );
        Ok(())
    }
}

fn before(deadline: Instant) -> Result<()> {
    ensure!(
        Instant::now() < deadline,
        "service baseline deadline expired"
    );
    Ok(())
}

fn stopped_bracket(
    original: &Snapshot,
    first: &Snapshot,
    group: GroupObservation,
    last: &Snapshot,
    deadline: Instant,
) -> Result<Option<RecoveryObservation>> {
    let a = stopped_snapshot(first, original, group)?;
    let b = stopped_snapshot(last, original, group)?;
    before(deadline)?;
    Ok(match (a, b) {
        (Some(a), Some(b)) if a == b && first == last => Some(RecoveryObservation {
            identity: a,
            group,
            policy: first.policy(),
        }),
        _ => None,
    })
}

pub(in crate::workflow::resident_supervisor) fn stopped_snapshot(
    s: &Snapshot,
    original: &Snapshot,
    group: GroupObservation,
) -> Result<Option<TerminalIdentity>> {
    let unit = original
        .property("Id")
        .context("missing original service Id")?;
    validate_unit(unit)?;
    validate_invocation(original.invocation())?;
    ensure!(
        original.main_pid() > 1
            && original.start_micros() != 0
            && original.property("ExecMainPID") == Some(original.main_pid().to_string().as_str()),
        "invalid original service incarnation"
    );
    ensure!(
        s.property("Id") == Some(unit) && s.property("LoadState") == Some("loaded"),
        "unit changed during stop"
    );
    ensure!(
        s.policy() == original.policy(),
        "service policy changed during stop"
    );
    let retained = s.invocation() == original.invocation()
        && s.property("ExecMainPID") == Some(original.main_pid().to_string().as_str())
        && s.start_micros() == original.start_micros();
    let cleared = s.invocation().is_empty()
        && s.property("ExecMainPID") == Some("0")
        && s.start_micros() == 0;
    ensure!(
        retained || cleared,
        "service incarnation changed during stop"
    );
    let active = s.property("ActiveState");
    let sub = s.property("SubState");
    if retained && (active == Some("active") || active == Some("deactivating")) {
        ensure!(
            s.main_pid() == 0 || s.main_pid() == original.main_pid(),
            "service main PID changed"
        );
        ensure!(
            s.control_group() == original.control_group(),
            "service control group changed"
        );
        return Ok(None);
    }
    ensure!(
        s.main_pid() == 0 && s.property("ControlPID") == Some("0") && s.property("Job") == Some(""),
        "service is not terminal"
    );
    ensure!(
        (active == Some("inactive") && sub == Some("dead"))
            || (retained && active == Some("failed") && sub == Some("failed")),
        "unexpected service terminal state"
    );
    ensure!(
        group != GroupObservation::Populated,
        "original service group remains populated"
    );
    ensure!(
        s.control_group() == original.control_group()
            || (s.control_group().is_empty() && group == GroupObservation::OriginalAbsent),
        "service group changed"
    );
    ensure!(
        retained || group == GroupObservation::OriginalAbsent,
        "cleared service needs original group deletion"
    );
    Ok(Some(if retained {
        TerminalIdentity::Retained
    } else {
        TerminalIdentity::Cleared
    }))
}

#[cfg(test)]
mod tests;

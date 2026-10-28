//! Fixed RM2 vendor-pair admission. Read-only: never starts or stops a service.
//! Serialized baselines are historical evidence, not independent launch authority.
use super::{baseline::ServiceBaseline, *};
use crate::workflow::resident_supervisor::{
    recovery::{firmware_version, verify_process, Pinned},
    text,
};
use std::path::Path;

const UNIT: &str = "rm-sync.service";
const EXE: &str = "/usr/bin/rm-sync";
const MOUNT: &str = "/sys/fs/cgroup/unified";
const EXE_SHA: &str = "1caf6493cce2132af89a9377679c19005bef1c5b0e658f704f95b4fdbd07138c";
const UNIT_PATH: &str = "/usr/lib/systemd/system/rm-sync.service";
const DROPIN: &str = "/usr/lib/systemd/system/rm-sync.service.d/rm-sync-service-override.conf";

pub struct OriginalCompanion {
    original: ServiceBaseline,
    pins: Vec<Pinned>,
}

impl OriginalCompanion {
    /// Explicit callers must bind this record to their candidate/lease before
    /// any stop. Loading validates fixed artifacts/policy, not original liveness.
    pub fn load(record: &str, deadline: Instant) -> Result<Self> {
        let original = ServiceBaseline::decode(record.as_bytes())?;
        ensure!(
            original.group().unit() == UNIT
                && original.group().mount_path == Path::new(MOUNT)
                && original.original().property("WatchdogUSec") == Some("0"),
            "unexpected original companion baseline"
        );
        let pins = [
            (EXE, EXE_SHA),
            (
                UNIT_PATH,
                "076675399604b9fdbee7f267900bd4490de88d32ab1cc49a32d8d45a75bcac8f",
            ),
            (
                DROPIN,
                "4d4345a6299448ade99878ddeeb894dd9a1a90de230bc3371a5e44237da7a795",
            ),
        ]
        .into_iter()
        .map(|(path, sha)| Pinned::verified(path, digest(sha), deadline))
        .collect::<Result<Vec<_>>>()?;
        let result = Self { original, pins };
        result.policy(deadline)?;
        Ok(result)
    }

    /// Developer inspection only; the protected candidate must select these
    /// bytes separately. No live sample becomes a release-approved profile.
    pub fn inspect(deadline: Instant) -> Result<String> {
        let original = ServiceBaseline::capture(Path::new(MOUNT), UNIT, deadline)?;
        let record = String::from_utf8(original.encode()?)?;
        let admitted = Self::load(&record, deadline)?;
        admitted.current(deadline)?;
        Ok(record)
    }

    pub fn policy(&self, deadline: Instant) -> Result<()> {
        before(deadline)?;
        ensure!(
            firmware_version(&text(Path::new("/etc/os-release"), 16 * 1024)?)? == "3.28.0.172"
                && text(Path::new("/proc/sys/kernel/osrelease"), 1024)?.trim()
                    == "5.4.70-v1.6.3-rm11x",
            "unqualified vendor firmware/kernel"
        );
        for pin in &self.pins {
            pin.current()?;
        }
        for unit in ["xochitl.service", UNIT] {
            let expected = policy(unit);
            let properties = expected.keys().copied().collect::<Vec<_>>().join(",");
            let mut command = Command::new("/bin/systemctl");
            command.args([
                "--system",
                "--no-pager",
                "--no-ask-password",
                "--all",
                "show",
                "--property",
                &properties,
                "--",
                unit,
            ]);
            check_policy(
                &run_with_progress(command, deadline, &mut || Ok(()))?,
                &expected,
            )?;
        }
        for pin in &self.pins {
            pin.current()?;
        }
        before(deadline)
    }

    pub fn current(&self, deadline: Instant) -> Result<()> {
        self.original.current(deadline)?;
        self.verify_running(&self.original, deadline)?;
        self.original.current(deadline)
    }

    pub fn stopped(&self, deadline: Instant) -> Result<bool> {
        self.policy(deadline)?;
        Ok(self.original.stopped(deadline)?.is_some())
    }

    pub fn verify_running(&self, baseline: &ServiceBaseline, deadline: Instant) -> Result<()> {
        ensure!(
            self.running_ready(baseline, deadline)?,
            "original companion broker name missing"
        );
        Ok(())
    }

    fn running_ready(&self, baseline: &ServiceBaseline, deadline: Instant) -> Result<bool> {
        self.policy(deadline)?;
        ensure!(
            baseline.group().unit() == UNIT
                && baseline.original().policy().matches_observed(
                    &self.original.original().policy(),
                    WatchdogObservation::Runtime
                ),
            "restored companion policy changed"
        );
        baseline.current(deadline)?;
        verify_process(
            baseline.group().parent(),
            EXE,
            digest(EXE_SHA),
            &[EXE],
            deadline,
        )?;
        let owner = companion::observe_owner(deadline, &mut || baseline.current(deadline))?;
        if let Some(owner) = &owner {
            ensure!(
                owner.pid == baseline.group().parent().pid,
                "original companion broker owner changed"
            );
        }
        baseline.current(deadline)?;
        before(deadline)?;
        Ok(owner.is_some())
    }

    /// Observe dependency-driven restoration only. Missing/failed sync never
    /// causes a separate start (BindsTo could start stock outside its owner).
    pub fn restored(
        &self,
        deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<ServiceBaseline> {
        await_ready(
            deadline,
            progress,
            || {
                self.policy(deadline)?;
                let sample = observe(UNIT, deadline)?;
                if sample.property("ActiveState") == Some("active")
                    && sample.property("SubState") == Some("running")
                    && sample.property("Job") == Some("")
                    && sample.property("ControlPID") == Some("0")
                {
                    let running = ServiceBaseline::capture(Path::new(MOUNT), UNIT, deadline)?;
                    ensure!(
                        running.original().same_observation(&sample),
                        "companion changed at recovery"
                    );
                    return Ok(Some(running));
                }
                Ok(None)
            },
            |running| self.running_ready(running, deadline),
        )
    }
}

// Once an active incarnation has been captured, a missing name is merely
// pending. Never recapture a replacement process while waiting for its broker.
fn await_ready<T>(
    deadline: Instant,
    progress: &mut dyn FnMut() -> Result<()>,
    mut acquire: impl FnMut() -> Result<Option<T>>,
    mut ready: impl FnMut(&T) -> Result<bool>,
) -> Result<T> {
    let mut retained = None;
    loop {
        before(deadline)?;
        progress()?;
        before(deadline)?;
        if retained.is_none() {
            retained = acquire()?;
        }
        if let Some(value) = &retained {
            if ready(value)? {
                progress()?;
                before(deadline)?;
                // Verify the retained incarnation again after caller admission.
                ensure!(ready(value)?, "companion readiness lost during delivery");
                before(deadline)?;
                return Ok(retained.take().expect("retained companion"));
            }
        }
        thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

fn before(deadline: Instant) -> Result<()> {
    crate::workflow::resident_request::before(deadline, "vendor admission deadline expired")
}

fn digest(hex: &str) -> [u8; 32] {
    let mut value = [0; 32];
    for (index, byte) in value.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).expect("fixed SHA256");
    }
    value
}

// Independent fixed schema: do not change serialized ServiceBaseline v1 when
// adding loaded-policy checks. Watchdog runtime/reset semantics stay there.
fn policy(unit: &str) -> BTreeMap<&'static str, &'static str> {
    let mut fields = BTreeMap::from([
        ("NeedDaemonReload", "no"),
        ("Type", "simple"),
        ("Restart", "on-failure"),
        ("TimeoutStartUSec", "1min 30s"),
        ("TimeoutStopUSec", "1min 30s"),
        ("UMask", "0022"),
        ("WorkingDirectory", ""),
        ("User", ""),
        ("Group", ""),
        ("KillMode", "control-group"),
        ("KillSignal", "15"),
        ("FinalKillSignal", "9"),
        ("SendSIGKILL", "yes"),
    ]);
    fields.extend(if unit == UNIT {
        [
            ("Id", UNIT), ("RestartMode", "normal"), ("NotifyAccess", "none"),
            ("RestartForceExitStatus", "HUP INT PIPE TERM"), ("BusName", "no.remarkable.sync"),
            ("Nice", "19"), ("Requires", "system.slice sysinit.target"), ("Wants", ""),
            ("BindsTo", "xochitl.service"), ("PartOf", "xochitl.service"),
            ("Before", "shutdown.target multi-user.target"),
            ("After", "home.mount xochitl.service sysinit.target systemd-journald.socket basic.target dbus.socket system.slice"),
            ("FragmentPath", UNIT_PATH), ("DropInPaths", DROPIN),
        ]
    } else {
        [
            ("Id", "xochitl.service"), ("RestartMode", "direct"), ("NotifyAccess", "all"),
            ("RestartForceExitStatus", ""), ("BusName", ""), ("Nice", "0"),
            ("Requires", "dbus.socket system.slice"), ("Wants", "wacom_flash.service rm-sync.service"),
            ("BindsTo", ""), ("PartOf", ""),
            ("Before", "rm-sync.service reader-buddy.service remarkable-reboot.service home.mount shutdown.target remarkable-shutdown.service"),
            ("After", "systemd-journald.socket var-lib-uboot.mount dbus.socket factory-reset.service system.slice"),
            ("FragmentPath", "/usr/lib/systemd/system/xochitl.service"),
            ("DropInPaths", "/usr/lib/systemd/system/xochitl.service.d/xochitl-service-override.conf"),
        ]
    });
    fields
}

fn check_policy(bytes: &[u8], expected: &BTreeMap<&str, &str>) -> Result<()> {
    ensure!(bytes.len() <= LIMIT, "vendor policy exceeds bound");
    let text = std::str::from_utf8(bytes)?;
    ensure!(
        !text.bytes().any(|c| c.is_ascii_control() && c != b'\n'),
        "control byte in vendor policy"
    );
    let mut seen = std::collections::BTreeSet::new();
    for line in text.lines() {
        let (key, value) = line.split_once('=').context("invalid vendor policy")?;
        let wanted = expected.get(key).context("unknown vendor policy")?;
        ensure!(seen.insert(key), "duplicate vendor policy");
        let equal = if matches!(
            key,
            "Requires"
                | "Wants"
                | "BindsTo"
                | "PartOf"
                | "Before"
                | "After"
                | "RestartForceExitStatus"
        ) {
            policy_unit_tokens(value)? == policy_unit_tokens(wanted)?
        } else {
            value == *wanted
        };
        ensure!(equal, "vendor loaded policy mismatch: {key}");
    }
    ensure!(seen.len() == expected.len(), "missing vendor policy");
    Ok(())
}

#[cfg(test)]
mod tests;

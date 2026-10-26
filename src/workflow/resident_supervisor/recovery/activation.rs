//! Installed service handoff. The product caller still owns qualified source
//! admission/reacquisition. No native page or mutation capability is fabricated.
use super::*;
use crate::{
    device::request_input::RequestInput,
    storage::Store,
    workflow::{
        resident_request::{self, FailureKind, InputProgress, Request, RequestFailure},
        resident_runtime::{Artifact, Launch, ResidentRuntime},
        resident_supervisor::{
            health::{ExitOnDrop, ServiceHealth},
            GroupObservation, StockBaseline,
        },
    },
};
use remarkable_open_sdk::{
    native_adapter::{Reply, RuntimeIdentity},
    OperationId,
};
use std::sync::Arc;

const NATIVE_PROFILE: &str = "/home/root/.local/lib/reader-buddy/resident-profile.json";
const ADAPTER: &str = "/home/root/.local/lib/reader-buddy/libremarkable-resident.so";
const ACTIVATION: Duration = Duration::from_secs(60);

/// Retaining the session retains the ordinary runtime's right to remain alive.
/// Its drop poisons the actual health owner and stops only its native child.
pub struct Session {
    pending: Pending,
    runtime: ResidentRuntime,
}

struct Pending {
    // Dropped first, including unwinding/partial publication/launch failure.
    exit: ExitOnDrop,
    installation: Installation,
    lease: Lease,
    buddy: service::Snapshot,
    helper: service::Snapshot,
    stock: service::Snapshot,
    stock_group: GroupBaseline,
}

impl Session {
    /// Only the installed first-gesture caller may compose this with live source
    /// admission. The input guard supplies no page identity or output authority.
    pub fn activate(
        store: Arc<Store>,
        health: &mut ServiceHealth,
        input: &mut RequestInput,
    ) -> Result<Self> {
        let deadline = Instant::now() + ACTIVATION;
        let mut session =
            Self::activate_with_progress(store, health, &mut || input.require_quiet(), deadline)?;
        ensure!(
            input.reacquire()? == InputProgress::Quiet,
            "Input canceled resident activation handoff"
        );
        session.current(health, &mut || input.require_quiet(), deadline)?;
        Ok(session)
    }

    fn activate_with_progress(
        store: Arc<Store>,
        health: &mut ServiceHealth,
        progress: &mut dyn FnMut() -> Result<()>,
        deadline: Instant,
    ) -> Result<Self> {
        ensure!(
            unsafe { libc::geteuid() } == 0,
            "installed activation requires root"
        );
        progress()?;
        health.progress()?; // Requires the original READY main-thread owner.
        let identity =
            installed_directory_identity()?.context("installed lease directory absent")?;
        require_unoccupied(Path::new(LEASE_DIRECTORY))?;
        let mut installation = Installation::load(deadline)?;
        installation.artifacts.push(Pinned::verified(
            NATIVE_PROFILE,
            installation.profile.native_profile_sha256,
            deadline,
        )?);
        installation.artifacts.push(Pinned::verified(
            ADAPTER,
            installation.profile.adapter_sha256,
            deadline,
        )?);
        let buddy = service::observe(BUDDY, deadline)?;
        let group = GroupBaseline::capture(Path::new(MOUNT), BUDDY)?;
        buddy_state(&buddy, &group, &installation.profile)?;
        verify_process(
            group.parent(),
            APP,
            installation.profile.app_sha256,
            &[APP],
            deadline,
        )?;
        ensure!(
            buddy == service::observe(BUDDY, deadline)?,
            "Buddy changed during activation admission"
        );
        let helper = service::observe(HELPER, deadline)?;
        idle_helper(&helper, &installation.profile)?;
        let stock = service::observe(STOCK, deadline)?;
        match_policy(&stock.policy(), &installation.profile.stock_policy)?;
        ensure!(
            stock_state(&stock)? == StockState::Ready,
            "stock must already be running"
        );
        let stock_group =
            GroupBaseline::capture_process(Path::new(MOUNT), STOCK, stock.main_pid())?;
        verify_process(
            stock_group.parent(),
            STOCK_EXE,
            installation.profile.stock_executable_sha256,
            &[STOCK_EXE, "--system"],
            deadline,
        )?;
        ensure!(
            stock == service::observe(STOCK, deadline)?,
            "stock changed during admission"
        );
        let record = LeaseRecord::new(
            group,
            buddy.invocation().to_owned(),
            buddy.start_micros(),
            StockBaseline {
                process: stock_group.parent().clone(),
                invocation: stock.invocation().to_owned(),
                executable_sha256: installation.profile.stock_executable_sha256,
                unit_sha256: installation.profile.stock_unit_sha256,
                dropin_sha256: installation.profile.stock_dropin_sha256,
            },
            installation.profile.native_profile_sha256,
            installation.profile.adapter_sha256,
            installation.digest,
        )?;
        installation.profile.validate(&record)?;
        installation.current(deadline)?;
        progress()?;
        ensure!(
            installed_directory_identity()? == Some(identity),
            "activation directory changed"
        );
        // Publication may fail after creating partial occupied state. Arm BEFORE it.
        let exit = health.exit_on_drop()?;
        let lease = publish_installed(record)?;
        let pending = Pending {
            exit,
            installation,
            lease,
            buddy,
            helper,
            stock,
            stock_group,
        };
        pending.current(health, progress, deadline)?;
        pending.original_stock(deadline)?;
        checked_handoff(
            deadline,
            || {
                progress()?;
                pending.exit.check(health)
            },
            || service::stop_stock(deadline),
        )?; // Exactly once, no retry after uncertainty.
        loop {
            pending.current(health, progress, deadline)?;
            if pending.stock_stopped(deadline)? {
                break;
            }
            thread::sleep(
                Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
        pending.current(health, progress, deadline)?;
        ensure!(pending.stock_stopped(deadline)?, "stock no longer stopped");
        let directory = Path::new(LEASE_DIRECTORY)
            .join(format!("session-{}", pending.lease.record().activation()));
        let profile = &pending.installation.profile;
        let launch = Launch {
            profile: Artifact {
                path: NATIVE_PROFILE.into(),
                sha256: profile.native_profile_sha256,
            },
            executable: Artifact {
                path: STOCK_EXE.into(),
                sha256: profile.stock_executable_sha256,
            },
            adapter: Artifact {
                path: ADAPTER.into(),
                sha256: profile.adapter_sha256,
            },
            directory,
            // Fixed minimal bootstrap environment; actual relocated resources and
            // this environment still require native qualification before approval.
            environment: BTreeMap::from([
                ("HOME".into(), "/home/root".into()),
                ("USER".into(), "root".into()),
                ("LOGNAME".into(), "root".into()),
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("LANG".into(), "C".into()),
            ]),
            arguments: vec!["--system".into()],
            operation_budget_ms: 30_000,
        };
        let runtime = ResidentRuntime::launch_admitted(store, launch, deadline, |deadline| {
            pending.current(health, progress, deadline)?;
            ensure!(
                pending.stock_stopped(deadline)?,
                "stock returned during launch preparation"
            );
            progress()?;
            pending.exit.check(health)?;
            check_deadline(deadline)
        })?;
        let mut session = Self { pending, runtime };
        session.current(health, progress, deadline)?;
        Ok(session)
    }

    /// Execute one request in the installed session. The caller supplies fresh
    /// post-activation input/source admission; historical replies grant no lease.
    pub fn request(
        &mut self,
        health: &mut ServiceHealth,
        input: &mut RequestInput,
        operation: OperationId,
        request: Request,
        deadline: Instant,
    ) -> std::result::Result<Reply, RequestFailure> {
        self.with_request(health, deadline, |runtime, authority| {
            resident_request::execute(
                runtime,
                operation,
                request,
                deadline,
                &mut || input.poll(),
                authority,
            )
        })
    }

    pub fn reconnect(
        &mut self,
        health: &mut ServiceHealth,
        input: &mut RequestInput,
        deadline: Instant,
    ) -> std::result::Result<(), RequestFailure> {
        self.with_request(health, deadline, |runtime, authority| {
            resident_request::reconnect(runtime, deadline, &mut || input.poll(), authority)
        })
    }

    fn with_request<T>(
        &mut self,
        health: &mut ServiceHealth,
        deadline: Instant,
        action: impl FnOnce(
            &mut ResidentRuntime,
            &mut dyn FnMut() -> Result<()>,
        ) -> std::result::Result<T, RequestFailure>,
    ) -> std::result::Result<T, RequestFailure> {
        let Self { pending, runtime } = self;
        let result = match pending.exit.check(health) {
            Ok(()) => {
                let identity = runtime.identity().clone();
                action(runtime, &mut || {
                    pending.session_current(&identity, health, &mut || Ok(()), deadline)
                })
            }
            Err(error) => Err(RequestFailure {
                kind: FailureKind::Integrity,
                lost: runtime.retire_connection(),
                error,
            }),
        };
        pending.exit.finish_request(result)
    }

    fn current(
        &mut self,
        health: &mut ServiceHealth,
        progress: &mut dyn FnMut() -> Result<()>,
        deadline: Instant,
    ) -> Result<()> {
        self.pending
            .session_current(self.runtime.identity(), health, progress, deadline)
    }
}

// Keep the action behind the final admission callback and original deadline.
// The private effect seam lets regression tests prove refusal sends no command.
fn checked_handoff<T>(
    deadline: Instant,
    admit: impl FnOnce() -> Result<()>,
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    check_deadline(deadline)?;
    admit()?;
    check_deadline(deadline)?;
    action()
}

impl Pending {
    fn session_current(
        &self,
        identity: &RuntimeIdentity,
        health: &mut ServiceHealth,
        progress: &mut dyn FnMut() -> Result<()>,
        deadline: Instant,
    ) -> Result<()> {
        self.current(health, progress, deadline)?;
        ensure!(
            self.stock_stopped(deadline)?,
            "stock is competing with resident runtime"
        );
        let child = GroupBaseline::capture_process(Path::new(MOUNT), BUDDY, identity.pid)?;
        let parent = self.lease.record().group();
        ensure!(
            child.context == parent.context
                && child.group == parent.group
                && child.parent.start_ticks == identity.start_ticks,
            "resident child left admitted Buddy group"
        );
        self.current(health, progress, deadline)
    }
    fn current(
        &self,
        health: &mut ServiceHealth,
        progress: &mut dyn FnMut() -> Result<()>,
        deadline: Instant,
    ) -> Result<()> {
        check_deadline(deadline)?;
        progress()?;
        self.exit.check(health)?;
        self.installation.current(deadline)?;
        self.lease.current()?;
        let buddy = service::observe(BUDDY, deadline)?;
        buddy_state(
            &buddy,
            self.lease.record().group(),
            &self.installation.profile,
        )?;
        ensure!(buddy == self.buddy, "original Buddy invocation changed");
        self.lease.record().group().current_caller()?;
        let helper = service::observe(HELPER, deadline)?;
        idle_helper(&helper, &self.installation.profile)?;
        ensure!(
            helper == self.helper,
            "recovery helper changed during activation"
        );
        ensure!(
            buddy == service::observe(BUDDY, deadline)?,
            "Buddy changed across admission"
        );
        self.installation.current(deadline)?;
        self.lease.current()?;
        check_deadline(deadline)
    }

    fn original_stock(&self, deadline: Instant) -> Result<()> {
        let before = service::observe(STOCK, deadline)?;
        ensure!(before == self.stock, "original stock changed before stop");
        self.stock_group.current_process()?;
        ensure!(
            service::observe(STOCK, deadline)? == self.stock,
            "stock changed across stop admission"
        );
        check_deadline(deadline)
    }

    fn stock_stopped(&self, deadline: Instant) -> Result<bool> {
        let before = service::observe(STOCK, deadline)?;
        let group = self.stock_group.observe()?;
        let after = service::observe(STOCK, deadline)?;
        match_policy(&before.policy(), &self.installation.profile.stock_policy)?;
        match_policy(&after.policy(), &self.installation.profile.stock_policy)?;
        let before_stopped = stopped_stock(&before, &self.stock, group)?;
        let after_stopped = stopped_stock(&after, &self.stock, group)?;
        check_deadline(deadline)?;
        // A legitimate shutdown can advance across a read. Wait for a stable
        // terminal bracket; both samples must still satisfy original identity.
        Ok(before == after && before_stopped && after_stopped)
    }
}

fn buddy_state(s: &service::Snapshot, group: &GroupBaseline, profile: &Profile) -> Result<()> {
    ensure!(
        group.unit == BUDDY && group.mount_path == Path::new(MOUNT),
        "Buddy group context mismatch"
    );
    match_policy(&s.policy(), &profile.buddy_policy)?;
    for (key, value) in [
        ("Id", BUDDY),
        ("LoadState", "loaded"),
        ("ActiveState", "active"),
        ("SubState", "running"),
        ("ControlPID", "0"),
        ("Job", ""),
    ] {
        ensure!(
            s.property(key) == Some(value),
            "Buddy activation state mismatch: {key}"
        );
    }
    ensure!(
        s.main_pid() == std::process::id()
            && s.main_pid() == group.parent().pid
            && s.property("ExecMainPID") == Some(s.main_pid().to_string().as_str())
            && s.start_micros() != 0
            && s.control_group() == format!("/system.slice/{BUDDY}"),
        "Buddy activation process mismatch"
    );
    validate_invocation(s.invocation())
}

fn idle_helper(s: &service::Snapshot, profile: &Profile) -> Result<()> {
    match_policy(&s.policy(), &profile.helper_policy)?;
    for (key, value) in [
        ("Id", HELPER),
        ("LoadState", "loaded"),
        ("ActiveState", "inactive"),
        ("SubState", "dead"),
        ("MainPID", "0"),
        ("ControlPID", "0"),
        ("Job", ""),
    ] {
        ensure!(
            s.property(key) == Some(value),
            "recovery helper is not idle: {key}"
        );
    }
    Ok(())
}

fn stopped_stock(
    s: &service::Snapshot,
    original: &service::Snapshot,
    group: GroupObservation,
) -> Result<bool> {
    ensure!(
        original.property("Id") == Some(STOCK),
        "original stock unit mismatch"
    );
    Ok(service::baseline::stopped_snapshot(s, original, group)?.is_some())
}

#[cfg(test)]
mod tests;

//! Installed service handoff. The product caller still owns qualified source
//! admission/reacquisition. No native page or mutation capability is fabricated.
use super::*;
use crate::{
    device::request_input::RequestInput,
    storage::Store,
    workflow::{
        resident_request::{self, FailureKind, InputProgress, Request, RequestFailure},
        resident_runtime::{Artifact, CompanionProfile, Launch, ResidentRuntime},
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

const ACTIVATION: Duration = Duration::from_secs(60);

/// Explicit mode in a release-approved, strictly pinned installed profile.
/// This value does not advertise native capability; no approved profile is shipped.
pub struct InstalledReaderSelection {
    digest: [u8; 32],
    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    installed_output: bool,
}

#[cfg(all(
    feature = "development-resident-bootstrap",
    feature = "development-input-diagnostics"
))]
pub struct InstalledOutput {
    profile: Pinned,
    digest: [u8; 32],
    native_profile: [u8; 32],
    adapter: [u8; 32],
    executable: [u8; 32],
}

#[cfg(all(
    feature = "development-resident-bootstrap",
    feature = "development-input-diagnostics"
))]
impl InstalledOutput {
    pub(crate) fn current(&self) -> Result<()> {
        self.profile.current()
    }

    pub(crate) fn admits(&self, runtime: &RuntimeIdentity) -> bool {
        runtime.profile_sha256 == self.native_profile
            && runtime.adapter_sha256 == self.adapter
            && runtime.executable_sha256 == self.executable
    }

    pub(crate) fn digest(&self) -> [u8; 32] {
        self.digest
    }
}
impl InstalledReaderSelection {
    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    pub(crate) fn has_installed_output(&self) -> bool {
        self.installed_output
    }

    pub fn load(deadline: Instant) -> Result<Option<Self>> {
        let Some(path) = profile_path()? else {
            return Ok(None);
        };
        let (pin, profile, digest) = read_profile(path, deadline)?;
        let Some(selection) = Self::from_profile(&profile, digest) else {
            pin.current()?;
            check_deadline(deadline)?;
            return Ok(None);
        };
        let installation = Installation::load(deadline)?;
        selection.require_digest(installation.digest)?;
        Self::validate_native(&installation, deadline)?;
        pin.current()?;
        check_deadline(deadline)?;
        Ok(Some(selection))
    }
    #[cfg(test)]
    pub(super) fn declared(bytes: &[u8]) -> Result<Option<Self>> {
        Ok(Self::from_profile(&parse_profile(bytes)?, sha256(bytes)))
    }
    fn from_profile(profile: &Profile, digest: [u8; 32]) -> Option<Self> {
        profile.source_mode.as_ref().map(|_| Self {
            digest,
            #[cfg(all(
                feature = "development-resident-bootstrap",
                feature = "development-input-diagnostics"
            ))]
            installed_output: profile.installed_output_definition(),
        })
    }
    pub(super) fn require_digest(&self, digest: [u8; 32]) -> Result<()> {
        ensure!(
            digest == self.digest,
            "Installed resident selection changed"
        );
        Ok(())
    }
    fn validate_native(installation: &Installation, deadline: Instant) -> Result<()> {
        ensure!(
            ((installation.profile.version == 3
                && installation.profile.source_mode.as_deref() == Some("resident-source-v1"))
                || installation.profile.development_definition()
                || installation.profile.installed_output_definition()),
            "Installed resident source mode unsupported"
        );
        Pinned::verified(
            installation.profile.layout().native_profile,
            installation.profile.native_profile_sha256,
            deadline,
        )?;
        Pinned::verified(
            installation.profile.layout().adapter,
            installation.profile.adapter_sha256,
            deadline,
        )?;
        check_deadline(deadline)
    }
    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    pub(crate) fn output(&self, deadline: Instant) -> Result<Option<InstalledOutput>> {
        if !self.installed_output {
            return Ok(None);
        }
        let installation = Installation::load(deadline)?;
        self.require_digest(installation.digest)?;
        ensure!(
            installation.profile.installed_output_definition(),
            "installed output selection changed"
        );
        Self::validate_native(&installation, deadline)?;
        let profile = Pinned::verified(
            installation.profile.layout().recovery,
            self.digest,
            deadline,
        )?;
        Ok(Some(InstalledOutput {
            profile,
            digest: self.digest,
            native_profile: installation.profile.native_profile_sha256,
            adapter: installation.profile.adapter_sha256,
            executable: installation.profile.stock_executable_sha256,
        }))
    }
    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    pub(crate) fn require_development_output(&self, path: &Path, deadline: Instant) -> Result<()> {
        let installation = Installation::load(deadline)?;
        self.require_digest(installation.digest)?;
        ensure!(
            installation.profile.development_definition()
                && path == Path::new(DEVELOPMENT_OUTPUT_PLAN),
            "explicit development output profile/argv required"
        );
        let plan = installation.profile.development_output.as_ref().unwrap();
        let mut pinned = Pinned::verified(&plan.path, plan.sha256, deadline)?;
        let raw = pinned.bytes(16 * 1024, deadline)?;
        let value: serde_json::Value = serde_json::from_slice(&raw)?;
        ensure!(
            value["nonce"].as_str() == Some(plan.nonce.as_str()),
            "development plan/profile nonce mismatch"
        );
        Self::validate_native(&installation, deadline)
    }
    pub(crate) fn current(&self, deadline: Instant) -> Result<()> {
        let installation = Installation::load(deadline)?;
        self.require_digest(installation.digest)?;
        Self::validate_native(&installation, deadline)
    }
    #[cfg(test)]
    pub(crate) fn fixture() -> Self {
        Self {
            digest: [17; 32],
            #[cfg(all(
                feature = "development-resident-bootstrap",
                feature = "development-input-diagnostics"
            ))]
            installed_output: false,
        }
    }
}

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
    vendor: Option<service::vendor::OriginalCompanion>,
}

impl Session {
    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    pub(crate) fn take_writer_request(
        &self,
        document: crate::storage::Uuid,
        page: crate::storage::Uuid,
    ) -> Result<Option<crate::writer::Request>> {
        self.pending.lease.current()?;
        super::super::writer_request::take(self.pending.lease.record().activation(), document, page)
    }

    /// Only the installed first-gesture caller may compose this with live source
    /// admission. The input guard supplies no page identity or output authority.
    pub fn activate(
        store: Arc<Store>,
        health: &mut ServiceHealth,
        input: &mut RequestInput,
    ) -> Result<Self> {
        Self::activate_until(store, health, input, Instant::now() + ACTIVATION)
    }
    pub fn activate_until(
        store: Arc<Store>,
        health: &mut ServiceHealth,
        input: &mut RequestInput,
        original_deadline: Instant,
    ) -> Result<Self> {
        Self::activate_selected_until(store, health, input, original_deadline, None)
    }
    pub(crate) fn activate_selected_until(
        store: Arc<Store>,
        health: &mut ServiceHealth,
        input: &mut RequestInput,
        original_deadline: Instant,
        selection: Option<&InstalledReaderSelection>,
    ) -> Result<Self> {
        let deadline = original_deadline.min(Instant::now() + ACTIVATION);
        let mut session = Self::activate_with_progress(
            store,
            health,
            &mut || input.require_quiet(),
            deadline,
            selection,
        )?;
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
        selection: Option<&InstalledReaderSelection>,
    ) -> Result<Self> {
        ensure!(
            unsafe { libc::geteuid() } == 0,
            "installed activation requires root"
        );
        progress()?;
        health.progress()?; // Requires the original READY main-thread owner.
        let mut installation = Installation::load(deadline)?;
        let runtime_directory = installation.profile.runtime_directory()?;
        let identity = installed_directory_identity_at(&runtime_directory)?
            .context("installed lease directory absent")?;
        require_unoccupied(&runtime_directory)?;
        if let Some(selected) = selection {
            selected.require_digest(installation.digest)?;
            InstalledReaderSelection::validate_native(&installation, deadline)?;
        }
        installation.artifacts.push(Pinned::verified(
            installation.profile.layout().native_profile,
            installation.profile.native_profile_sha256,
            deadline,
        )?);
        installation.artifacts.push(Pinned::verified(
            installation.profile.layout().adapter,
            installation.profile.adapter_sha256,
            deadline,
        )?);
        let buddy = service::observe(BUDDY, deadline)?;
        let group = GroupBaseline::capture(Path::new(MOUNT), BUDDY)?;
        buddy_state(&buddy, &group, &installation.profile)?;
        verify_process(
            group.parent(),
            installation.profile.layout().app,
            installation.profile.app_sha256,
            &installation.profile.app_argv(),
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
        let companion_record = installation
            .profile
            .companion
            .as_ref()
            .map(|_| service::vendor::OriginalCompanion::inspect(deadline))
            .transpose()?;
        let vendor = companion_record
            .as_deref()
            .map(|record| service::vendor::OriginalCompanion::load(record, deadline))
            .transpose()?;
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
        let record = if let Some(companion) = companion_record {
            record.with_companion(companion)?
        } else {
            record
        };
        installation.profile.validate(&record)?;
        installation.current(deadline)?;
        progress()?;
        ensure!(
            installed_directory_identity_at(&runtime_directory)? == Some(identity),
            "activation directory changed"
        );
        // Publication may fail after creating partial occupied state. Arm BEFORE it.
        let exit = health.exit_on_drop()?;
        let lease = publish_installed_at(record, &runtime_directory)?;
        let pending = Pending {
            exit,
            installation,
            lease,
            buddy,
            helper,
            stock,
            stock_group,
            vendor,
        };
        pending.current(health, progress, deadline)?;
        checked_handoff(
            deadline,
            || {
                progress()?;
                pending.exit.check(health)?;
                pending.original_stock(deadline)?;
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
        let directory = pending
            .installation
            .profile
            .runtime_directory()?
            .join(format!("session-{}", pending.lease.record().activation()));
        let profile = &pending.installation.profile;
        let launch = Launch {
            companion: pending
                .vendor
                .as_ref()
                .map(|_| CompanionProfile::rm2_3_28_0_172()),
            profile: Artifact {
                path: profile.layout().native_profile.into(),
                sha256: profile.native_profile_sha256,
            },
            executable: Artifact {
                path: STOCK_EXE.into(),
                sha256: profile.stock_executable_sha256,
            },
            adapter: Artifact {
                path: profile.layout().adapter.into(),
                sha256: profile.adapter_sha256,
            },
            directory,
            // Match the observed stock service tool search path and UTF-8 locale;
            // relocated resources still require native qualification before approval.
            environment: BTreeMap::from([
                ("HOME".into(), "/home/root".into()),
                ("USER".into(), "root".into()),
                ("LOGNAME".into(), "root".into()),
                (
                    "PATH".into(),
                    "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin".into(),
                ),
                ("LANG".into(), "en_US.UTF-8".into()),
            ]),
            arguments: vec!["--system".into()],
            operation_budget_ms: 30_000,
        };
        let runtime = if selection.is_some() {
            ResidentRuntime::launch_read_only_admitted(
                store,
                launch,
                deadline,
                &mut ActivationAdmission {
                    pending: &pending,
                    health,
                    progress,
                },
            )?
        } else {
            ResidentRuntime::launch_admitted(store, launch, deadline, |deadline| {
                pending.current(health, progress, deadline)?;
                ensure!(
                    pending.stock_stopped(deadline)?,
                    "stock returned during launch preparation"
                );
                progress()?;
                pending.exit.check(health)?;
                check_deadline(deadline)
            })?
        };
        let mut session = Self { pending, runtime };
        session.current(health, progress, deadline)?;
        Ok(session)
    }

    /// Execute one request in the installed session. The caller supplies fresh
    /// post-activation input/source admission; historical replies grant no lease.
    pub(crate) fn source_pulse(&mut self, health: &mut ServiceHealth) -> Result<()> {
        let result = self
            .pending
            .pulse(self.runtime.identity(), health)
            .and_then(|()| self.runtime.pulse());
        if result.is_err() {
            self.pending.exit.fail();
        }
        result
    }
    pub(crate) fn source_request(
        &mut self,
        health: &mut ServiceHealth,
        input: &mut RequestInput,
        operation: OperationId,
        request: crate::workflow::resident_runtime::ReadOnlyRequest,
        deadline: Instant,
    ) -> std::result::Result<Reply, RequestFailure> {
        let identity = self.runtime.identity().clone();
        let mut guard = SourceAdmission {
            pending: &self.pending,
            identity,
            health,
            input,
        };
        let result = self
            .runtime
            .request_read_only(operation, request, deadline, &mut guard);
        self.pending
            .exit
            .finish_request(resident_request::finish(&mut self.runtime, result))
    }
    pub(crate) fn source_reconnect(
        &mut self,
        health: &mut ServiceHealth,
        input: &mut RequestInput,
        deadline: Instant,
    ) -> std::result::Result<(), RequestFailure> {
        let identity = self.runtime.identity().clone();
        let mut guard = SourceAdmission {
            pending: &self.pending,
            identity,
            health,
            input,
        };
        let result = self.runtime.reconnect_read_only(deadline, &mut guard);
        self.pending
            .exit
            .finish_request(resident_request::finish(&mut self.runtime, result))
    }
    pub(crate) fn source_delivery(
        &mut self,
        health: &mut ServiceHealth,
        input: &mut RequestInput,
        deadline: Instant,
    ) -> std::result::Result<(), RequestFailure> {
        let identity = self.runtime.identity().clone();
        let mut guard = SourceAdmission {
            pending: &self.pending,
            identity,
            health,
            input,
        };
        let result = self.runtime.read_only_boundary(deadline, &mut guard);
        self.pending
            .exit
            .finish_request(resident_request::finish(&mut self.runtime, result))
    }

    pub(crate) fn source_insert(
        &mut self,
        health: &mut ServiceHealth,
        input: &mut RequestInput,
        conversation: crate::storage::Uuid,
        operation: OperationId,
        insertion: remarkable_open_sdk::native_adapter::InsertAfter,
        deadline: Instant,
    ) -> std::result::Result<Reply, RequestFailure> {
        let identity = self.runtime.identity().clone();
        let mut guard = SourceAdmission {
            pending: &self.pending,
            identity,
            health,
            input,
        };
        let result =
            self.runtime
                .insert_admitted(conversation, operation, insertion, deadline, &mut guard);
        self.pending
            .exit
            .finish_request(resident_request::finish(&mut self.runtime, result))
    }

    pub(crate) fn source_identity(&self) -> RuntimeIdentity {
        self.runtime.identity().clone()
    }
    pub(crate) fn finish_source_request(&mut self) {
        self.runtime.retire_connection();
    }

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

struct ActivationAdmission<'a> {
    pending: &'a Pending,
    health: &'a mut ServiceHealth,
    progress: &'a mut dyn FnMut() -> Result<()>,
}
impl crate::workflow::resident_runtime::ReadOnlyAdmission for ActivationAdmission<'_> {
    fn pulse(&mut self, deadline: Instant) -> Result<()> {
        check_deadline(deadline)?;
        self.pending.activation_pulse(self.health)?;
        (self.progress)()?;
        check_deadline(deadline)
    }
    fn boundary(&mut self, deadline: Instant) -> Result<()> {
        self.pending
            .source_boundary(None, self.health, deadline, self.progress)
    }
}

struct SourceAdmission<'a> {
    pending: &'a Pending,
    identity: RuntimeIdentity,
    health: &'a mut ServiceHealth,
    input: &'a mut RequestInput,
}
impl crate::workflow::resident_runtime::ReadOnlyAdmission for SourceAdmission<'_> {
    fn pulse(&mut self, deadline: Instant) -> Result<()> {
        resident_request::admission(deadline, &mut || self.input.poll(), &mut || {
            self.pending.pulse(&self.identity, self.health)
        })
    }
    fn boundary(&mut self, deadline: Instant) -> Result<()> {
        self.boundary_with_pulse(deadline, &mut || Ok(()))
    }
    fn boundary_with_pulse(
        &mut self,
        deadline: Instant,
        runtime_pulse: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        self.pulse(deadline)?;
        self.pending
            .source_boundary(Some(&self.identity), self.health, deadline, &mut || {
                runtime_pulse()?;
                resident_request::admission(deadline, &mut || self.input.poll(), &mut || Ok(()))
            })
            .map_err(resident_request::authority_error)?;
        self.pulse(deadline)
    }
}

impl Pending {
    fn pulse(&self, identity: &RuntimeIdentity, health: &mut ServiceHealth) -> Result<()> {
        self.activation_pulse(health)?;
        let child = GroupBaseline::capture_process(Path::new(MOUNT), BUDDY, identity.pid)?;
        let parent = self.lease.record().group();
        ensure!(
            child.context == parent.context
                && child.group == parent.group
                && child.parent.start_ticks == identity.start_ticks,
            "resident child left admitted Buddy group"
        );
        Ok(())
    }
    fn activation_pulse(&self, health: &mut ServiceHealth) -> Result<()> {
        self.exit.check(health)?;
        for artifact in &self.installation.artifacts {
            artifact.current()?;
        }
        self.lease.current()?;
        self.lease.record().group().current_caller()?;
        ensure!(
            self.stock_group.observe()? != GroupObservation::Populated,
            "original stock group returned"
        );
        if let Some(vendor) = &self.vendor {
            vendor.pulse_stopped()?;
        }
        Ok(())
    }
    fn source_boundary(
        &self,
        identity: Option<&RuntimeIdentity>,
        health: &mut ServiceHealth,
        deadline: Instant,
        pulse: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        if let Some(identity) = identity {
            self.pulse(identity, health)?;
        } else {
            self.activation_pulse(health)?;
        }
        let mut units = vec![BUDDY, HELPER, STOCK];
        if self.vendor.is_some() {
            units.push("rm-sync.service");
        }
        let mut checked = || {
            if let Some(identity) = identity {
                self.pulse(identity, health)?;
            } else {
                self.activation_pulse(health)?;
            }
            pulse()?;
            check_deadline(deadline)
        };
        let first = service::observe_many(&units, deadline, &mut checked)?;
        checked()?;
        let stock_group = self.stock_group.observe()?;
        let vendor_group = self
            .vendor
            .as_ref()
            .map(|v| v.stopped_group())
            .transpose()?;
        let last = service::observe_many(&units, deadline, &mut checked)?;
        for sample in [&first, &last] {
            let buddy = sample.get(BUDDY)?;
            buddy_state(
                buddy,
                self.lease.record().group(),
                &self.installation.profile,
            )?;
            ensure!(*buddy == self.buddy, "original Buddy invocation changed");
            let helper = sample.get(HELPER)?;
            idle_helper(helper, &self.installation.profile)?;
            ensure!(
                helper.same_observation(&self.helper),
                "recovery helper changed during request"
            );
            let stock = sample.get(STOCK)?;
            match_observed_policy(
                &stock.policy(),
                &self.installation.profile.stock_policy,
                stock.watchdog_observation(stock_group),
            )?;
            ensure!(
                stopped_stock(stock, &self.stock, stock_group)?,
                "stock is competing with resident runtime"
            );
        }
        ensure!(
            first.get(STOCK)?.same_observation(last.get(STOCK)?),
            "stock changed across request admission"
        );
        if let Some(vendor) = &self.vendor {
            ensure!(
                vendor.stopped_batches(&first, vendor_group.unwrap(), &last, deadline)?,
                "original companion returned"
            );
        }
        if let Some(identity) = identity {
            self.pulse(identity, health)?;
        } else {
            self.activation_pulse(health)?;
        }
        check_deadline(deadline)
    }
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
        if let Some(vendor) = &self.vendor {
            vendor.policy(deadline)?;
        }
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
            helper.same_observation(&self.helper),
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
        if let Some(vendor) = &self.vendor {
            vendor.current(deadline)?;
        }
        ensure!(
            service::observe(STOCK, deadline)? == self.stock,
            "stock changed across stop admission"
        );
        check_deadline(deadline)
    }

    fn stock_stopped(&self, deadline: Instant) -> Result<bool> {
        let before = service::observe(STOCK, deadline)?;
        let group = self.stock_group.observe()?;
        let companion_stopped = self
            .vendor
            .as_ref()
            .map_or(Ok(true), |vendor| vendor.stopped(deadline))?;
        let after = service::observe(STOCK, deadline)?;
        match_observed_policy(
            &before.policy(),
            &self.installation.profile.stock_policy,
            before.watchdog_observation(group),
        )?;
        match_observed_policy(
            &after.policy(),
            &self.installation.profile.stock_policy,
            after.watchdog_observation(group),
        )?;
        let before_stopped = stopped_stock(&before, &self.stock, group)?;
        let after_stopped = stopped_stock(&after, &self.stock, group)?;
        check_deadline(deadline)?;
        // A legitimate shutdown can advance across a read. Wait for a stable
        // terminal bracket; both samples must still satisfy original identity.
        Ok(before.same_observation(&after) && before_stopped && after_stopped && companion_stopped)
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
    require(&profile.helper_policy, "WatchdogUSec", "0")?;
    let mut expected = profile.helper_policy.clone();
    if s.unstarted_watchdog() {
        // Only this idle observation can contain systemd's pre-start sentinel.
        // Installation pins the approved zero-watchdog unit; active helper
        // admission still matches the original profile (zero) without changes.
        expected.insert("WatchdogUSec".into(), "infinity".into());
    }
    match_policy(&s.policy(), &expected)?;
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

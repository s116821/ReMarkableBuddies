//! Closed read-only dispatch. No mutable connection or effect closure escapes.
use super::*;
use remarkable_open_sdk::native_adapter::{ReaderFacts, Reply};

pub enum ReadOnlyRequest {
    Observe,
    #[cfg(buddy_development_native_pre_enter)]
    CapturePreEnter {
        offset: u32,
        count: u16,
    },
    #[cfg(feature = "development-native-close")]
    ObserveClosed,
    CaptureSource(ReaderFacts),
    #[cfg(feature = "development-resident-bootstrap")]
    CaptureVisual,
}
pub trait ReadOnlyAdmission {
    fn pulse(&mut self, deadline: Instant) -> Result<()>;
    fn boundary(&mut self, deadline: Instant) -> Result<()>;
    fn boundary_with_pulse(
        &mut self,
        deadline: Instant,
        runtime_pulse: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        runtime_pulse()?;
        self.boundary(deadline)?;
        runtime_pulse()
    }
    fn before_api(&mut self, _deadline: Instant) -> Result<()> {
        Ok(())
    }
    #[cfg(test)]
    fn reply_observed(&mut self) {} // Injection seam after a real successful SDK result.
    fn after_api(
        &mut self,
        _deadline: Instant,
        _invoked: bool,
        _result: &Result<Reply>,
    ) -> Result<()> {
        Ok(())
    }
}
impl ResidentRuntime {
    /// Cheap retained ownership checks, including while no socket/request exists.
    pub fn pulse(&mut self) -> Result<()> {
        self.prepared.current(true).map_err(integrity)?;
        check_children(&mut self.child, &mut self.prepared).map_err(integrity)
    }
    fn full_read_only_boundary(
        &mut self,
        deadline: Instant,
        admission: &mut dyn ReadOnlyAdmission,
    ) -> Result<()> {
        let Self {
            child, prepared, ..
        } = self;
        let mut pulse = || {
            before(deadline)?;
            prepared.current(true).map_err(integrity)?;
            check_children(child, prepared).map_err(integrity)?;
            before(deadline)
        };
        admission.boundary_with_pulse(deadline, &mut pulse)
    }
    pub fn read_only_boundary(
        &mut self,
        deadline: Instant,
        admission: &mut dyn ReadOnlyAdmission,
    ) -> Result<()> {
        before(deadline)?;
        self.full_read_only_boundary(deadline, admission)?;
        self.observe_companion(deadline, &mut || admission.pulse(deadline))?;
        self.pulse()?;
        self.full_read_only_boundary(deadline, admission)?;
        before(deadline)
    }
    pub fn request_read_only(
        &mut self,
        operation: OperationId,
        request: ReadOnlyRequest,
        deadline: Instant,
        admission: &mut dyn ReadOnlyAdmission,
    ) -> Result<Reply> {
        self.request_read_only_inner(operation, request, deadline, admission, false)
    }
    /// Development capture has a separate bounded reconnect. Charge the actual
    /// request its own ten seconds only after fresh pre-dispatch admission, still
    /// capped by the caller's original setup/overall deadline.
    #[cfg(feature = "development-resident-bootstrap")]
    pub fn request_read_only_capture(
        &mut self,
        operation: OperationId,
        request: ReadOnlyRequest,
        deadline: Instant,
        admission: &mut dyn ReadOnlyAdmission,
    ) -> Result<Reply> {
        self.request_read_only_inner(operation, request, deadline, admission, true)
    }
    fn request_read_only_inner(
        &mut self,
        operation: OperationId,
        request: ReadOnlyRequest,
        deadline: Instant,
        admission: &mut dyn ReadOnlyAdmission,
        fresh_capture_budget: bool,
    ) -> Result<Reply> {
        self.read_only_boundary(deadline, admission)?;
        self.pulse()?;
        admission.pulse(deadline)?;
        ensure!(
            self.connection.is_some(),
            "resident connection absent; reconnect explicitly"
        );
        let deadline = if fresh_capture_budget {
            deadline.min(Instant::now() + Duration::from_secs(10))
        } else {
            deadline
        };
        let mut invoked = false;
        let result = (|| -> Result<Reply> {
            admission.before_api(deadline)?;
            before(deadline)?;
            let Self {
                child,
                connection,
                prepared,
                ..
            } = self;
            let result = {
                let mut pulse = || {
                    before(deadline)?;
                    admission.pulse(deadline)?;
                    prepared.current(true).map_err(integrity)?;
                    check_children(child, prepared).map_err(integrity)?;
                    before(deadline)
                };
                before(deadline)?;
                let connection = connection
                    .as_mut()
                    .context("resident connection absent; reconnect explicitly")?;
                invoked = true;
                match request {
                    #[cfg(buddy_development_native_pre_enter)]
                    ReadOnlyRequest::CapturePreEnter { offset, count } => connection
                        .capture_pre_enter_with_progress(
                            operation, offset, count, deadline, &mut pulse,
                        ),
                    ReadOnlyRequest::Observe => {
                        connection.observe_with_progress(operation, deadline, &mut pulse)
                    }
                    #[cfg(feature = "development-native-close")]
                    ReadOnlyRequest::ObserveClosed => {
                        connection.observe_closed_with_progress(operation, deadline, &mut pulse)
                    }
                    ReadOnlyRequest::CaptureSource(expected) => connection
                        .capture_source_with_progress(operation, expected, deadline, &mut pulse),
                    #[cfg(feature = "development-resident-bootstrap")]
                    ReadOnlyRequest::CaptureVisual => {
                        connection.capture_visual_with_progress(operation, deadline, &mut pulse)
                    }
                }
            };
            #[cfg(test)]
            if result.is_ok() {
                admission.reply_observed();
            }
            if result.is_ok() {
                before(deadline)?;
                admission.pulse(deadline)?;
                prepared.current(true).map_err(integrity)?;
                check_children(child, prepared).map_err(integrity)?;
                before(deadline)?;
            }
            result
        })();
        let retention = admission.after_api(deadline, invoked, &result);
        let result = match (result, retention) {
            (Err(error), Err(retention)) => {
                return Err(error.context(format!("API diagnostic retention failed: {retention:#}")))
            }
            (Err(error), Ok(())) => return Err(error),
            (Ok(_), Err(error)) => return Err(error),
            (Ok(value), Ok(())) => value,
        };
        self.observe_companion(deadline, &mut || admission.pulse(deadline))?;
        self.full_read_only_boundary(deadline, admission)?;
        self.pulse()?;
        before(deadline)?;
        Ok(result)
    }
    pub fn reconnect_read_only(
        &mut self,
        deadline: Instant,
        admission: &mut dyn ReadOnlyAdmission,
    ) -> Result<()> {
        // Existing reconnect retains loss and same-child semantics. Its recursive
        // callbacks now receive ONLY cheap pulses; full admission brackets it.
        before(deadline)?;
        self.full_read_only_boundary(deadline, admission)?;
        self.reconnect(deadline, &mut || admission.pulse(deadline))?;
        self.full_read_only_boundary(deadline, admission)?;
        self.pulse()?;
        before(deadline)
    }
}

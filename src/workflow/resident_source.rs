//! Real selected Workflow source route. Historical pixels confer no output lease.
// The researched route is deliberately disabled pending intent admission.
#[allow(dead_code)]
pub(super) mod original_pdf;

#[cfg(test)]
use super::resident_runtime::ReadOnlyAdmission;
use super::{
    resident_request::{FailureKind, RequestFailure},
    resident_runtime::{ReadOnlyRequest as Request, ResidentRuntime},
    resident_supervisor::{
        health::ServiceHealth,
        recovery::activation::{InstalledReaderSelection, Session},
    },
};
use crate::{
    conversation::ResidentSourceCapture,
    device::request_input::{InputProgress, ReleasedReaderInput, RequestInput, ResidentGesture},
    storage::Store,
};
use anyhow::{ensure, Context, Result};
use remarkable_open_sdk::{
    native_adapter::{ReaderFacts, Reply, RuntimeIdentity},
    OperationId, Uuid,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) const ACQUISITION: Duration = Duration::from_secs(90);
#[cfg(feature = "installed-resident-output")]
type OrdinaryPdfCapture = (
    ResidentSourceCapture,
    Result<Option<original_pdf::OrdinaryPdfSource>>,
);

#[derive(Debug)]
pub(super) struct FinalizedCancellation;

impl std::fmt::Display for FinalizedCancellation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Resident source request canceled")
    }
}

impl std::error::Error for FinalizedCancellation {}
fn operation() -> OperationId {
    OperationId(Uuid::parse(&uuid::Uuid::new_v4().to_string()).expect("OS UUID"))
}
fn before(deadline: Instant) -> Result<()> {
    super::resident_request::before(deadline, "resident source acquisition expired")
}
fn admitted_progress(deadline: Instant, progress: impl FnOnce() -> Result<()>) -> Result<()> {
    before(deadline)?;
    progress()?;
    before(deadline)
}

trait SourceSession {
    fn pulse(&mut self, health: &mut ServiceHealth) -> Result<()>;
    fn delivery(
        &mut self,
        health: &mut ServiceHealth,
        input: &mut RequestInput,
        deadline: Instant,
    ) -> std::result::Result<(), RequestFailure>;
    fn runtime(&self) -> &ResidentRuntime;
    fn identity(&self) -> RuntimeIdentity {
        self.runtime().identity().clone()
    }
    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    fn take_writer_request(
        &self,
        _document: crate::storage::Uuid,
        _page: crate::storage::Uuid,
    ) -> Result<Option<super::resident_supervisor::writer_request::InstalledRequest>> {
        Ok(None)
    }
    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    fn peek_regional_writer(
        &self,
    ) -> Result<Option<super::resident_supervisor::writer_request::RegionalPin>> {
        Ok(None)
    }
    fn reconnect(
        &mut self,
        health: &mut ServiceHealth,
        input: &mut RequestInput,
        deadline: Instant,
    ) -> std::result::Result<(), RequestFailure>;
    fn request(
        &mut self,
        health: &mut ServiceHealth,
        input: &mut RequestInput,
        operation: OperationId,
        request: Request,
        deadline: Instant,
    ) -> std::result::Result<Reply, RequestFailure>;
    fn insert(
        &mut self,
        _health: &mut ServiceHealth,
        _input: &mut RequestInput,
        _conversation: crate::storage::Uuid,
        _operation: OperationId,
        _insertion: remarkable_open_sdk::native_adapter::InsertAfter,
        _deadline: Instant,
    ) -> std::result::Result<Reply, RequestFailure> {
        Err(RequestFailure {
            kind: FailureKind::Refused,
            lost: remarkable_open_sdk::native_adapter::LostRequest::NoPendingMutation,
            error: anyhow::anyhow!("resident insertion backend unavailable"),
        })
    }
    fn finish(&mut self);
    #[cfg(test)]
    fn companion_pid(&self) -> Option<u32> {
        None
    }
}
impl SourceSession for Session {
    fn pulse(&mut self, health: &mut ServiceHealth) -> Result<()> {
        self.source_pulse(health)
    }
    fn delivery(
        &mut self,
        health: &mut ServiceHealth,
        input: &mut RequestInput,
        deadline: Instant,
    ) -> std::result::Result<(), RequestFailure> {
        self.source_delivery(health, input, deadline)
    }
    fn runtime(&self) -> &ResidentRuntime {
        self.source_runtime()
    }
    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    fn take_writer_request(
        &self,
        document: crate::storage::Uuid,
        page: crate::storage::Uuid,
    ) -> Result<Option<super::resident_supervisor::writer_request::InstalledRequest>> {
        Session::take_writer_request(self, document, page)
    }
    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    fn peek_regional_writer(
        &self,
    ) -> Result<Option<super::resident_supervisor::writer_request::RegionalPin>> {
        Session::peek_regional_writer(self)
    }
    fn reconnect(
        &mut self,
        health: &mut ServiceHealth,
        input: &mut RequestInput,
        deadline: Instant,
    ) -> std::result::Result<(), RequestFailure> {
        Session::source_reconnect(self, health, input, deadline)
    }
    fn request(
        &mut self,
        health: &mut ServiceHealth,
        input: &mut RequestInput,
        operation: OperationId,
        request: Request,
        deadline: Instant,
    ) -> std::result::Result<Reply, RequestFailure> {
        Session::source_request(self, health, input, operation, request, deadline)
    }
    fn insert(
        &mut self,
        health: &mut ServiceHealth,
        input: &mut RequestInput,
        conversation: crate::storage::Uuid,
        operation: OperationId,
        insertion: remarkable_open_sdk::native_adapter::InsertAfter,
        deadline: Instant,
    ) -> std::result::Result<Reply, RequestFailure> {
        self.source_insert(health, input, conversation, operation, insertion, deadline)
    }
    fn finish(&mut self) {
        self.finish_source_request();
    }
}
trait Factory {
    fn activate(
        &mut self,
        selection: &InstalledReaderSelection,
        store: Arc<Store>,
        health: &mut ServiceHealth,
        input: &mut RequestInput,
        deadline: Instant,
    ) -> Result<Box<dyn SourceSession>>;
}
struct InstalledFactory;
impl Factory for InstalledFactory {
    fn activate(
        &mut self,
        selection: &InstalledReaderSelection,
        store: Arc<Store>,
        health: &mut ServiceHealth,
        input: &mut RequestInput,
        deadline: Instant,
    ) -> Result<Box<dyn SourceSession>> {
        selection.current(deadline)?;
        Ok(Box::new(Session::activate_selected_until(
            store,
            health,
            input,
            deadline,
            Some(selection),
        )?))
    }
}

pub(super) struct ResidentSourceOwner {
    selection: InstalledReaderSelection,
    store: Arc<Store>,
    factory: Box<dyn Factory>,
    session: Option<Box<dyn SourceSession>>,
    input: Option<RequestInput>,
    gesture: Option<ResidentGesture>,
    deadline: Option<Instant>,
    reconnect: bool,
    canceled: bool,
    fatal: bool,
    captured_source: Option<remarkable_open_sdk::native_adapter::ReaderFacts>,
    #[cfg(all(test, feature = "installed-resident-output"))]
    ordinary_pdf_fixture: Option<(std::path::PathBuf, std::path::PathBuf)>,
    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    development_diagnostics: Option<std::path::PathBuf>,
}
impl ResidentSourceOwner {
    pub(super) fn new(selection: InstalledReaderSelection, store: Arc<Store>) -> Self {
        Self {
            selection,
            store,
            factory: Box::new(InstalledFactory),
            session: None,
            input: None,
            gesture: None,
            deadline: None,
            reconnect: false,
            canceled: false,
            fatal: false,
            captured_source: None,
            #[cfg(all(test, feature = "installed-resident-output"))]
            ordinary_pdf_fixture: None,
            #[cfg(all(
                feature = "development-resident-bootstrap",
                feature = "development-input-diagnostics"
            ))]
            development_diagnostics: None,
        }
    }
    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    pub(super) fn require_development_output(&mut self, path: &std::path::Path) -> Result<()> {
        self.selection
            .require_development_output(path, Instant::now() + Duration::from_secs(10))?;
        self.development_diagnostics = Some(
            path.parent()
                .context("development plan parent")?
                .to_path_buf(),
        );
        Ok(())
    }

    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    pub(super) fn installed_output(
        &self,
    ) -> Result<Option<super::resident_supervisor::recovery::activation::InstalledOutput>> {
        self.selection
            .output(Instant::now() + Duration::from_secs(10))
    }

    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    pub(super) fn activate_at_startup(
        &mut self,
        corner: crate::device::touch::TriggerCorner,
        health: &mut ServiceHealth,
        deadline: Option<Instant>,
        check: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        self.activate_at_startup_with_input(
            health,
            deadline,
            || RequestInput::new(corner, None),
            check,
        )
    }

    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    fn activate_at_startup_with_input(
        &mut self,
        health: &mut ServiceHealth,
        deadline: Option<Instant>,
        input: impl FnOnce() -> Result<RequestInput>,
        check: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        ensure!(
            !self.fatal && self.session.is_none() && self.input.is_none(),
            "resident startup activation requires an unused owner"
        );
        if let Some(deadline) = deadline {
            before(deadline)?;
        }
        // Startup observes quiet input without creating a released Reader request.
        let mut input = input()?;
        input.require_quiet()?;
        check()?;
        let mut session = self.factory.activate(
            &self.selection,
            self.store.clone(),
            health,
            &mut input,
            deadline.unwrap_or_else(|| Instant::now() + ACQUISITION),
        )?;
        session.finish();
        self.session = Some(session);
        self.reconnect = true;
        log::info!("Resident Reader activated before the first request");
        Ok(())
    }
    pub(super) fn fail_idle(&mut self, health: &ServiceHealth) {
        self.fatal = true;
        health.poison();
    }
    pub(super) fn fatal(&self) -> bool {
        self.fatal
    }
    pub(super) fn canceled(&self) -> bool {
        self.canceled
    }
    pub(super) fn blank_start_requested(&self) -> bool {
        self.gesture == Some(ResidentGesture::BlankStart)
    }

    #[cfg(feature = "installed-resident-output")]
    pub(super) fn installed_output_selected(&self) -> bool {
        self.selection.has_installed_output()
    }

    /// An unbound final page has no successor to reuse or adopt. The original
    /// operation can block insertion even when it belongs to another notebook.
    #[cfg(feature = "installed-resident-output")]
    pub(super) fn blocked_unbound_insertion(&self) -> Option<OperationId> {
        let source = self.captured_source.as_ref()?;
        if self.installed_output_selected() && source.order().last() == Some(&source.page().page) {
            return self.session.as_ref()?.runtime().unresolved_operation();
        }
        None
    }

    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    pub(super) fn take_installed_writer(
        &self,
        capture: &ResidentSourceCapture,
    ) -> Result<Option<super::resident_output::SelectedConversationRequest>> {
        if self.gesture != Some(ResidentGesture::Reader) || !self.selection.has_installed_output() {
            return Ok(None);
        }
        use super::{
            resident_output::SelectedConversationRequest,
            resident_supervisor::writer_request::InstalledRequest,
        };

        let session = self
            .session
            .as_ref()
            .context("resident Writer session absent")?;
        Ok(session
            .take_writer_request(capture.document(), capture.page())?
            .map(|request| match request {
                InstalledRequest::Refine(request) => {
                    SelectedConversationRequest::WriterRefinement {
                        request,
                        require_current_draft: true,
                    }
                }
                InstalledRequest::GenerateRegion(_) => {
                    SelectedConversationRequest::UnsupportedRegionalWriter
                }
                InstalledRequest::GenerateCurrentTarget(request) => {
                    SelectedConversationRequest::WriterCurrentTarget(
                        crate::writer::target::CurrentRequest {
                            operation: request.operation,
                            request: request.request,
                            guidance: request.guidance,
                        },
                    )
                }
            }))
    }
    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    pub(super) fn peek_regional_writer(
        &self,
    ) -> Result<Option<super::resident_supervisor::writer_request::RegionalPin>> {
        if !self.selection.has_installed_output() {
            return Ok(None);
        }
        self.session
            .as_ref()
            .map(|session| session.peek_regional_writer())
            .transpose()
            .map(Option::flatten)
    }
    pub(super) fn begin(
        &mut self,
        released: ReleasedReaderInput,
        health: &mut ServiceHealth,
    ) -> Result<()> {
        self.begin_with_check(released, health, || Ok(()))
    }

    pub(super) fn begin_with_check(
        &mut self,
        released: ReleasedReaderInput,
        health: &mut ServiceHealth,
        check: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        self.begin_until(released, health, ACQUISITION, check)
    }

    pub(super) fn begin_until(
        &mut self,
        released: ReleasedReaderInput,
        health: &mut ServiceHealth,
        acquisition: Duration,
        check: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        ensure!(
            acquisition <= ACQUISITION,
            "resident acquisition may only be clamped"
        );
        ensure!(
            !self.fatal && self.input.is_none(),
            "Resident source request cannot restart"
        );
        self.deadline = Some(released.released_at + acquisition);
        self.input = Some(released.input);
        self.gesture = Some(released.gesture);
        self.canceled = false;
        self.captured_source = None;
        self.progress(health)?;
        check()?;
        let deadline = self.deadline.unwrap();
        before(deadline)?;
        let input = self.input.as_mut().unwrap();
        if self.session.is_none() {
            match self.factory.activate(
                &self.selection,
                self.store.clone(),
                health,
                input,
                deadline,
            ) {
                Ok(session) => self.session = Some(session),
                Err(error) => {
                    self.fatal = true;
                    return Err(error);
                }
            }
        } else if self.reconnect {
            let end = deadline.min(Instant::now() + Duration::from_secs(30));
            let result = self.session.as_mut().unwrap().reconnect(health, input, end);
            if let Err(error) = result {
                return self.failure(health, error);
            }
            self.reconnect = false;
        }
        before(deadline)?;
        self.progress(health)
    }
    fn failure<T>(&mut self, health: &ServiceHealth, error: RequestFailure) -> Result<T> {
        self.canceled |= error.kind == FailureKind::Canceled;
        self.fatal |= error.kind == FailureKind::Integrity;
        if self.fatal {
            health.poison();
        }
        self.reconnect = true;
        Err(anyhow::Error::new(error))
    }
    pub(super) fn progress(&mut self, health: &mut ServiceHealth) -> Result<()> {
        ensure!(!self.fatal, "Resident source integrity lost");
        if let Err(error) = health.progress() {
            self.fatal = true;
            return Err(error);
        }
        if let Some(session) = &mut self.session {
            if let Err(error) = session.pulse(health) {
                self.fatal = true;
                health.poison();
                return Err(error);
            }
        }
        if let Some(input) = &mut self.input {
            match input.poll() {
                Ok(InputProgress::Quiet) => {}
                Ok(InputProgress::Canceled) => self.canceled = true,
                Err(error) => {
                    self.fatal = true;
                    health.poison();
                    return Err(error);
                }
            }
        }
        ensure!(!self.canceled, "Resident source request canceled");
        Ok(())
    }
    pub(super) fn acquisition_current(&mut self, health: &mut ServiceHealth) -> Result<()> {
        let deadline = self
            .deadline
            .context("Resident source requires a released gesture")?;
        admitted_progress(deadline, || self.progress(health))
    }
    pub(super) fn acquire(&mut self, health: &mut ServiceHealth) -> Result<ResidentSourceCapture> {
        self.acquire_with_check(health, |_| Ok(()))
    }

    pub(super) fn acquire_with_check(
        &mut self,
        health: &mut ServiceHealth,
        check: impl FnMut(Option<&ReaderFacts>) -> Result<()>,
    ) -> Result<ResidentSourceCapture> {
        self.acquire_with_source(health, None, check, |_, _, _| Ok(()))
            .map(|(capture, ())| capture)
    }

    /// Acquire the original PDF under the existing released-input owner. The
    /// caller supplies short selected boundaries and a separate progress check.
    /// Neither callback may retain a gate across source I/O or worker waits.
    #[allow(dead_code, clippy::too_many_arguments)]
    pub(super) fn acquire_original_pdf_with_check(
        &mut self,
        health: &mut ServiceHealth,
        admitted_document: crate::storage::Uuid,
        acquisition_deadline: Instant,
        request_deadline: Instant,
        check: impl FnMut(Option<&ReaderFacts>) -> Result<()>,
        mut progress: impl FnMut() -> Result<()>,
    ) -> Result<(ResidentSourceCapture, original_pdf::CapturedOriginalPdf)> {
        let deadline = self.original_pdf_deadline(acquisition_deadline, request_deadline)?;
        self.acquire_original_pdf_using(
            health,
            deadline,
            request_deadline,
            check,
            |facts, pulse| {
                original_pdf::ObservedOriginalPdf::observe(
                    facts,
                    admitted_document,
                    deadline,
                    pulse,
                )
            },
            &mut progress,
            original_pdf::ObservedOriginalPdf::captured_by_owner,
        )
    }

    #[cfg(feature = "installed-resident-output")]
    pub(super) fn acquire_ordinary_pdf(
        &mut self,
        health: &mut ServiceHealth,
    ) -> Result<OrdinaryPdfCapture> {
        if self.gesture != Some(ResidentGesture::Reader) {
            return Ok((self.acquire(health)?, Ok(None)));
        }
        let deadline = self.deadline.context("Resident source deadline absent")?;
        self.acquire_with_source(
            health,
            Some(deadline),
            |_| Ok(()),
            |_owner, _, facts| {
                // Only metadata is sampled here. Writer arbitration precedes
                // every byte read from either asset.
                #[cfg(test)]
                if let Some((root, _)) = &_owner.ordinary_pdf_fixture {
                    return Ok(original_pdf::OrdinaryPdfSource::pin_fixture(root, facts).map(Some));
                }
                Ok(original_pdf::OrdinaryPdfSource::pin(facts).map(Some))
            },
        )
    }

    #[cfg(feature = "installed-resident-output")]
    pub(super) fn read_ordinary_pdf(
        &mut self,
        health: &mut ServiceHealth,
        source: original_pdf::OrdinaryPdfSource,
    ) -> Result<Option<original_pdf::ObservedOriginalPdf>> {
        let facts = self
            .captured_source
            .clone()
            .context("Resident PDF capture facts absent")?;
        let deadline = self.deadline.context("Resident source deadline absent")?;
        self.acquisition_current(health)?;
        source.read(&facts, deadline, &mut || {
            self.original_pdf_progress(health, deadline, &mut || Ok(()))
        })
    }

    #[cfg(feature = "installed-resident-output")]
    pub(super) fn complete_ordinary_pdf(
        &mut self,
        health: &mut ServiceHealth,
        observed: original_pdf::ObservedOriginalPdf,
        deadline: Instant,
    ) -> Result<original_pdf::CapturedOriginalPdf> {
        let facts = self
            .captured_source
            .clone()
            .context("Resident PDF capture facts absent")?;
        let acquired = self.deadline.context("Resident source deadline absent")?;
        #[cfg(test)]
        let fixture = self.ordinary_pdf_fixture.clone();
        let mut progress = || self.original_pdf_progress(health, acquired, &mut || Ok(()));
        #[cfg(test)]
        if let Some((_, executable)) = fixture {
            return observed.complete_ordinary_fixture(
                &facts,
                &executable,
                deadline,
                &mut progress,
            );
        }
        observed.captured_by_owner(&facts, deadline, &mut progress)
    }

    #[cfg(feature = "installed-resident-output")]
    pub(super) fn ordinary_pdf_deadline(&self) -> Result<Instant> {
        let acquired = self.deadline.context("Resident source deadline absent")?;
        // Bound both readings, optional research and the final answer using
        // the original release's acquisition deadline.
        Ok(acquired + Duration::from_secs(270))
    }

    fn original_pdf_deadline(
        &self,
        acquisition_deadline: Instant,
        request_deadline: Instant,
    ) -> Result<Instant> {
        ensure!(
            !self.reconnect
                && self.captured_source.is_none()
                && self.gesture == Some(ResidentGesture::Reader)
                && acquisition_deadline <= request_deadline,
            "Original PDF capture is unavailable for this release"
        );
        let owner_deadline = self
            .deadline
            .context("Resident source requires a released gesture")?;
        Ok(owner_deadline.min(acquisition_deadline))
    }

    fn original_pdf_progress(
        &mut self,
        health: &mut ServiceHealth,
        deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        admitted_progress(deadline, || {
            self.progress(health)?;
            progress()
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn acquire_original_pdf_using<T>(
        &mut self,
        health: &mut ServiceHealth,
        deadline: Instant,
        request_deadline: Instant,
        check: impl FnMut(Option<&ReaderFacts>) -> Result<()>,
        observe: impl FnOnce(
            &ReaderFacts,
            &mut dyn FnMut() -> Result<()>,
        ) -> Result<original_pdf::ObservedOriginalPdf>,
        progress: &mut dyn FnMut() -> Result<()>,
        complete: impl FnOnce(
            original_pdf::ObservedOriginalPdf,
            &ReaderFacts,
            Instant,
            &mut dyn FnMut() -> Result<()>,
        ) -> Result<T>,
    ) -> Result<(ResidentSourceCapture, T)> {
        let result =
            self.acquire_with_source(health, Some(deadline), check, |owner, health, facts| {
                let mut pulse = || owner.original_pdf_progress(health, deadline, progress);
                observe(facts, &mut pulse)
            });
        if result.is_err() && !self.reconnect {
            // Refuse any retry against this release, even if local PDF
            // admission failed before the capture request reached the SDK.
            if let Some(session) = self.session.as_mut() {
                session.finish();
            }
            self.reconnect = true;
        }
        let (capture, pdf) = result?;
        let verified = self
            .captured_source
            .clone()
            .context("Resident PDF capture facts absent")?;
        let mut pulse = || self.original_pdf_progress(health, deadline, progress);
        Ok((
            capture,
            complete(pdf, &verified, request_deadline, &mut pulse)?,
        ))
    }

    fn acquire_with_source<T>(
        &mut self,
        health: &mut ServiceHealth,
        deadline: Option<Instant>,
        mut check: impl FnMut(Option<&ReaderFacts>) -> Result<()>,
        observed_source: impl FnOnce(&mut Self, &mut ServiceHealth, &ReaderFacts) -> Result<T>,
    ) -> Result<(ResidentSourceCapture, T)> {
        self.acquisition_current(health)?;
        let prepare_source = deadline.is_some();
        if let Some(deadline) = deadline {
            before(deadline)?;
        }
        check(None)?;
        let deadline = deadline.unwrap_or_else(|| self.deadline.unwrap());
        let observation = operation();
        let capture = operation();
        let session = self.session.as_mut().context("Resident Session absent")?;
        let runtime = session.identity();
        let input = self.input.as_mut().unwrap();
        let observed = session.request(
            health,
            input,
            observation,
            Request::Observe,
            deadline.min(Instant::now() + Duration::from_secs(10)),
        );
        let facts = match observed {
            Ok(Reply::Observed(facts)) => facts,
            Ok(other) => {
                #[cfg(all(
                    feature = "development-resident-bootstrap",
                    feature = "development-input-diagnostics"
                ))]
                if let Some(directory) = self
                    .development_diagnostics
                    .as_ref()
                    .filter(|_| !prepare_source)
                {
                    // Explicit installed development profile only. A visual sample
                    // permits fixture setup inspection, never page authority.
                    let visual = session.request(
                        health,
                        input,
                        operation(),
                        Request::CaptureVisual,
                        deadline.min(Instant::now() + Duration::from_secs(10)),
                    );
                    if let Ok(Reply::VisualCaptured(image)) = visual {
                        use std::{io::Write, os::unix::fs::OpenOptionsExt};
                        let path = directory.join(format!("setup-{}.png", uuid::Uuid::new_v4()));
                        let mut file = std::fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .mode(0o600)
                            .open(&path)?;
                        file.write_all(image.png())?;
                        file.sync_all()?;
                        log::info!(
                            "Development setup image: {} sha256={}",
                            path.display(),
                            crate::storage::digest(image.png())
                        );
                    }
                }
                session.finish();
                self.reconnect = true;
                anyhow::bail!("Resident source observation refused: {other:?}")
            }
            Err(error) => {
                session.finish();
                return self.failure(health, error);
            }
        };
        check(Some(&facts))?;
        let source = observed_source(self, health, &facts)?;
        if prepare_source {
            check(Some(&facts))?;
        }
        before(deadline)?;
        let session = self.session.as_mut().context("Resident Session absent")?;
        let input = self.input.as_mut().unwrap();
        let reply = session.request(
            health,
            input,
            capture,
            Request::CaptureSource(facts.clone()),
            deadline.min(Instant::now() + Duration::from_secs(10)),
        );
        // End only this socket/request. Native Session/child remains owned.
        session.finish();
        self.reconnect = true;
        let historical = match reply {
            Ok(Reply::Captured(value)) => value,
            Ok(other) => anyhow::bail!("Resident source capture refused: {other:?}"),
            Err(error) => return self.failure(health, error),
        };
        self.acquisition_current(health)?;
        self.captured_source = Some(facts.clone());
        // Retain only information actually supplied/validated by this route.
        match ResidentSourceCapture::from_session(&runtime, observation, capture, facts, historical)
        {
            Ok(value) => Ok((value, source)),
            Err(error) => {
                self.fatal = true;
                Err(error)
            }
        }
    }
    pub(super) fn deliver(&mut self, health: &mut ServiceHealth) -> Result<()> {
        self.acquisition_current(health)?;
        let deadline = self.deadline.unwrap();
        let result = self
            .session
            .as_mut()
            .context("Resident Session absent at delivery")?
            .delivery(
                health,
                self.input
                    .as_mut()
                    .context("Resident input absent at delivery")?,
                deadline,
            );
        if let Err(error) = result {
            return self.failure(health, error);
        }
        self.acquisition_current(health)?;
        self.deadline = None;
        Ok(())
    }
    pub(super) fn finish(&mut self, health: &mut ServiceHealth) -> Result<()> {
        let result = self.progress(health);
        let canceled = result.is_err() && self.canceled && !self.fatal && !health.failed();
        if let Some(session) = &mut self.session {
            session.finish();
            self.reconnect = true;
        }
        self.input = None;
        self.gesture = None;
        self.deadline = None;
        self.captured_source = None;
        self.canceled = false;
        // The old guard is gone, never renewed. Cancellation stayed sticky
        // throughout its request; only a distinct release can begin another.
        if canceled {
            Err(FinalizedCancellation.into())
        } else {
            result
        }
    }
}

fn unsettled_output_insertion(
    ledger: &crate::conversation::Ledger,
    facts: &remarkable_open_sdk::native_adapter::ReaderFacts,
) -> Result<Option<Uuid>> {
    use crate::conversation::HistoricalPageAssociationInspection::HistoricalCandidate;
    use crate::storage::Uuid as StoredUuid;

    let page = facts.page();
    let document = StoredUuid::from_bytes(*page.document.as_bytes());
    for attempt in super::resident_client::attempts(ledger.store())? {
        let Some(insertion) = attempt.possible_insertion() else {
            continue;
        };
        let source = insertion.source.page();
        if source.document != page.document
            || (source.page != page.page && insertion.target != page.page)
        {
            continue;
        }
        // A request can select a new conversation after an interrupted binding.
        // The historical target must already be bound and still next to its source.
        let adjacent = facts
            .order()
            .windows(2)
            .any(|pair| pair == [source.page, insertion.target]);
        if !adjacent || !matches!(attempt.reply, Some(Reply::InsertionObserved { .. })) {
            return Ok(Some(insertion.target));
        }
        let target = StoredUuid::from_bytes(*insertion.target.as_bytes());
        let binding = ledger.inspect_page_association(attempt.conversation, document, target)?;
        if !matches!(binding, HistoricalCandidate(association)
            if association.receipt().source.document == document
                && association.receipt().source.page.as_bytes() == source.page.as_bytes())
        {
            return Ok(Some(insertion.target));
        }
    }
    Ok(None)
}

impl ResidentSourceOwner {
    #[cfg(feature = "installed-resident-output")]
    pub(super) fn require_ordinary_pdf_current(
        &mut self,
        health: &mut ServiceHealth,
        deadline: Instant,
    ) -> Result<()> {
        let original = self
            .captured_source
            .clone()
            .context("Resident PDF capture facts absent")?;
        self.reconnect_for_observation(health, deadline)?;
        let current = self.output_observe(health, deadline)?;
        // Provider and indexing work can outlast an idle client connection.
        // Retain the same owner and open a fresh read-only connection when needed.
        self.session
            .as_mut()
            .context("resident Session absent")?
            .finish();
        self.reconnect = true;
        ensure!(
            current == original,
            "Resident PDF source or page order changed"
        );
        Ok(())
    }

    fn reconnect_for_observation(
        &mut self,
        health: &mut ServiceHealth,
        end: Instant,
    ) -> Result<()> {
        admitted_progress(end, || self.progress(health))?;
        if self.reconnect {
            let reconnect_end = end.min(Instant::now() + Duration::from_secs(30));
            let result = self
                .session
                .as_mut()
                .context("resident Session absent")?
                .reconnect(
                    health,
                    self.input.as_mut().context("resident input guard absent")?,
                    reconnect_end,
                );
            if let Err(error) = result {
                return self.failure(health, error);
            }
            self.reconnect = false;
        }
        Ok(())
    }

    fn output_observe(
        &mut self,
        health: &mut ServiceHealth,
        end: Instant,
    ) -> Result<remarkable_open_sdk::native_adapter::ReaderFacts> {
        admitted_progress(end, || self.progress(health))?;
        let result = self
            .session
            .as_mut()
            .context("resident output Session absent")?
            .request(
                health,
                self.input
                    .as_mut()
                    .context("resident output guard absent")?,
                operation(),
                Request::Observe,
                end.min(Instant::now() + Duration::from_secs(10)),
            );
        match result {
            Ok(Reply::Observed(facts)) => {
                admitted_progress(end, || self.progress(health))?;
                Ok(facts)
            }
            Ok(reply) => anyhow::bail!("resident output observation refused: {reply:?}"),
            Err(error) => self.failure(health, error),
        }
    }
    fn output_capture(
        &mut self,
        health: &mut ServiceHealth,
        expected: &remarkable_open_sdk::native_adapter::ReaderFacts,
        end: Instant,
    ) -> Result<remarkable_open_sdk::native_adapter::HistoricalCapture> {
        let result = self
            .session
            .as_mut()
            .context("resident output Session absent")?
            .request(
                health,
                self.input
                    .as_mut()
                    .context("resident output guard absent")?,
                operation(),
                Request::CaptureSource(expected.clone()),
                end.min(Instant::now() + Duration::from_secs(10)),
            );
        let image = match result {
            Ok(Reply::Captured(image)) => image,
            Ok(other) => anyhow::bail!("resident target capture refused: {other:?}"),
            Err(error) => return self.failure(health, error),
        };
        ensure!(
            image.before() == expected && image.after() == expected,
            "target capture changed source/order"
        );
        admitted_progress(end, || self.progress(health))?;
        Ok(image)
    }
    #[allow(clippy::too_many_arguments)]
    fn output_role(
        &mut self,
        health: &mut ServiceHealth,
        backend: &mut dyn super::resident_output::ResidentOutputBackend,
        ledger: &crate::conversation::Ledger,
        conversation: crate::storage::Uuid,
        expected: &remarkable_open_sdk::native_adapter::ReaderFacts,
        blank_start_source: bool,
        end: Instant,
        before_effect: &mut dyn FnMut() -> Result<()>,
    ) -> Result<super::resident_output::PageUse> {
        let runtime = self.session.as_ref().unwrap().identity();
        let image = self.output_capture(health, expected, end)?;
        let mut role = backend.classify(
            ledger,
            conversation,
            &runtime,
            expected,
            &image,
            blank_start_source,
            end,
            &mut || admitted_progress(end, || self.progress(health)),
        )?;
        if role == super::resident_output::PageUse::LegacyBuddy {
            admitted_progress(end, || self.progress(health))?;
            before_effect()?;
            admitted_progress(end, || self.progress(health))?;
            if !backend.adopt(ledger, conversation, &runtime, expected, end)? {
                return Ok(super::resident_output::PageUse::Unresolved);
            }
            admitted_progress(end, || self.progress(health))?;
            let fresh = self.output_observe(health, end)?;
            super::resident_output::exact(&fresh, expected)?;
            let image = self.output_capture(health, expected, end)?;
            role = backend.classify(
                ledger,
                conversation,
                &runtime,
                expected,
                &image,
                blank_start_source,
                end,
                &mut || admitted_progress(end, || self.progress(health)),
            )?;
            ensure!(
                role == super::resident_output::PageUse::RelevantBuddy,
                "adoption did not produce qualified relevance"
            );
        }
        Ok(role)
    }
    fn output_navigate(
        &mut self,
        health: &mut ServiceHealth,
        backend: &mut dyn super::resident_output::ResidentOutputBackend,
        direction: remarkable_open_sdk::navigation::LogicalDirection,
        end: Instant,
        before_effect: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        admitted_progress(end, || self.progress(health))?;
        before_effect()?;
        admitted_progress(end, || self.progress(health))?;
        let runtime = self.session.as_ref().unwrap().identity();
        let old = self
            .input
            .take()
            .context("resident navigation guard absent")?;
        let result = backend.navigate(old, direction, &runtime, end, &mut || {
            admitted_progress(end, || self.progress(health))
        });
        match result {
            Ok(input) => self.input = Some(input),
            Err(error) => {
                self.fatal = true;
                health.poison();
                return Err(error);
            }
        }
        admitted_progress(end, || self.progress(health))
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_answer(
        &mut self,
        health: &mut ServiceHealth,
        backend: &mut dyn super::resident_output::ResidentOutputBackend,
        ledger: &crate::conversation::Ledger,
        conversation: crate::storage::Uuid,
        text: super::resident_output::ResidentOutputText<'_>,
        association: Option<&crate::conversation::HistoricalPageAssociation>,
        before_effect: &mut dyn FnMut() -> Result<()>,
    ) -> Result<super::resident_output::ResidentOutputOutcome> {
        use super::resident_output::{exact, PageUse, ResidentOutputOutcome as Outcome};
        use remarkable_open_sdk::{
            native_adapter::{InsertAfter, ReaderFacts},
            navigation::LogicalDirection,
            PageKey,
        };
        // One original output deadline. Provider work cannot refresh this window;
        // later stages only clamp it, and synchronous work is checked on return.
        let end = Instant::now() + Duration::from_secs(90);
        self.selection.current(end)?;
        admitted_progress(end, || self.progress(health))?;
        let original = self
            .captured_source
            .clone()
            .context("no resident source for output")?;
        let runtime = self
            .session
            .as_ref()
            .context("resident output Session absent")?
            .identity();
        backend.admit(&runtime, association, end)?;
        #[cfg(feature = "development-resident-bootstrap")]
        if let Some(keyboard) = backend.owned_keyboard()? {
            self.input
                .as_mut()
                .context("resident output guard absent")?
                .bind_owned_keyboard(&keyboard)?;
        }

        self.reconnect_for_observation(health, end)?;
        let fresh = self.output_observe(health, end)?;
        exact(&fresh, &original)?;
        if let Some(target) = unsettled_output_insertion(ledger, &fresh)? {
            log::info!("Resident output requires reconciliation for historical insertion {target}");
            return Ok(Outcome::Unresolved);
        }
        let association_current = || -> Result<bool> {
            let Some(expected) = association else {
                return Ok(true);
            };
            let receipt = expected.receipt();
            Ok(matches!(
                ledger.inspect_page_association(conversation, receipt.observed_document, receipt.observed_page)?,
                crate::conversation::HistoricalPageAssociationInspection::HistoricalCandidate(current)
                    if current.binding_revision() == expected.binding_revision()
            ))
        };
        if !association_current()? {
            return Ok(Outcome::Unresolved);
        }
        let blank_start_source = self.blank_start_requested();
        let current = self.output_role(
            health,
            backend,
            ledger,
            conversation,
            &original,
            blank_start_source,
            end,
            before_effect,
        )?;
        let mut newly_created = false;
        let target = if current == PageUse::RelevantBuddy {
            original.clone() // current-first reuse: do not inspect a successor
        } else {
            if current != PageUse::ConfirmedNonBuddy {
                return Ok(Outcome::Unresolved);
            }
            let source_index = original
                .order()
                .iter()
                .position(|id| *id == original.page().page)
                .unwrap();
            let mut reuse = None;
            if let Some(next) = original.order().get(source_index + 1).copied() {
                self.output_navigate(health, backend, LogicalDirection::Next, end, before_effect)?;
                let expected = ReaderFacts::new(
                    PageKey {
                        document: original.page().document,
                        page: next,
                    },
                    original.order().to_vec(),
                )
                .map_err(|error| anyhow::anyhow!("target facts: {error:?}"))?;
                let observed = self.output_observe(health, end)?;
                exact(&observed, &expected)?;
                let role = self.output_role(
                    health,
                    backend,
                    ledger,
                    conversation,
                    &expected,
                    false,
                    end,
                    before_effect,
                )?;
                if role == PageUse::RelevantBuddy {
                    reuse = Some(expected);
                } else {
                    // The inspected neighbor may be unrelated or unresolved.
                    // Restore the original only from the exact observed neighbor.
                    let observed = self.output_observe(health, end)?;
                    exact(&observed, &expected)?;
                    self.output_navigate(
                        health,
                        backend,
                        LogicalDirection::Previous,
                        end,
                        before_effect,
                    )?;
                    let observed = self.output_observe(health, end)?;
                    exact(&observed, &original)?;
                    if role != PageUse::ConfirmedNonBuddy || association.is_some() {
                        log::info!(
                            "Resident output refused after restoring source {}",
                            original.page().page
                        );
                        return Ok(Outcome::Unresolved);
                    }
                }
            }
            if let Some(existing) = reuse {
                existing
            } else {
                if association.is_some() {
                    return Ok(Outcome::Unresolved);
                }
                ensure!(original.order().len() <= 16, "native insertion order bound");
                let observed = self.output_observe(health, end)?;
                exact(&observed, &original)?;
                let target = Uuid::parse(&uuid::Uuid::new_v4().to_string()).expect("OS UUID");
                let insertion = InsertAfter {
                    source: original.clone(),
                    target,
                };
                let after_order = insertion.expected_after_order();
                // Existing resident_client persists canonical intent and uncertainty
                // before dispatch; a refusal/error never dispatches a replacement.
                before_effect()?;
                let result = self.session.as_mut().unwrap().insert(
                    health,
                    self.input.as_mut().unwrap(),
                    conversation,
                    operation(),
                    insertion,
                    end.min(Instant::now() + Duration::from_secs(30)),
                );
                match result {
                    Ok(Reply::InsertionObserved {
                        target: actual,
                        after_order: order,
                    }) => {
                        ensure!(actual==PageKey{document:original.page().document,page:target} && order==after_order,
                            "insertion result does not match original source/allocated target/order");
                        log::info!("Resident insertion observed on {target}");
                    }
                    Ok(Reply::Rejected(reason)) => {
                        log::info!("Resident page insertion refused: {reason:?}");
                        return Ok(Outcome::InsertionRefused);
                    }
                    Ok(other) => {
                        anyhow::bail!("resident insertion unresolved; no retry: {other:?}")
                    }
                    Err(error) => return self.failure(health, error),
                }
                newly_created = true;
                let expected = ReaderFacts::new(
                    PageKey {
                        document: original.page().document,
                        page: target,
                    },
                    after_order.clone(),
                )
                .map_err(|error| anyhow::anyhow!("inserted target facts: {error:?}"))?;
                let observed = self.output_observe(health, end)?;
                if observed != expected {
                    let source_after =
                        ReaderFacts::new(original.page(), after_order).map_err(|error| {
                            anyhow::anyhow!("source facts after insertion: {error:?}")
                        })?;
                    exact(&observed, &source_after)?;
                    self.output_navigate(
                        health,
                        backend,
                        LogicalDirection::Next,
                        end,
                        before_effect,
                    )?;
                    let observed = self.output_observe(health, end)?;
                    exact(&observed, &expected)?;
                }
                expected
            }
        };
        if let Some(expected) = association {
            if expected.receipt().observed_page.as_bytes() != target.page().page.as_bytes()
                || !association_current()?
            {
                return Ok(Outcome::Unresolved);
            }
        }
        if text.is_empty() && !newly_created {
            return Ok(Outcome::ReusedWithoutWrite);
        }
        // Fresh full service/broker admission plus a second exact Observe before
        // requesting a live render lease. Historical Insert/PNG are not that lease.
        let delivery =
            self.session
                .as_mut()
                .unwrap()
                .delivery(health, self.input.as_mut().unwrap(), end);
        if let Err(error) = delivery {
            return self.failure(health, error);
        }
        let observed = self.output_observe(health, end)?;
        exact(&observed, &target)?;
        before_effect()?;
        admitted_progress(end, || self.progress(health))?;
        let mut renderer = backend.renderer(
            ledger,
            conversation,
            &runtime,
            &target,
            newly_created,
            end,
            &mut || admitted_progress(end, || self.progress(health)),
        )?;
        admitted_progress(end, || self.progress(health))?;
        // Non-cloneable scoped lease borrows the REAL retained owner and guard.
        // Every lower effect takes full admission + fresh native Observe before
        // contact and checks owned input/runtime/deadline throughout and after.
        let final_admission = self.session.as_mut().unwrap().delivery(
            health,
            self.input.as_mut().context("target input absent")?,
            end,
        );
        if let Err(error) = final_admission {
            return self.failure(health, error);
        }
        let final_target = self.output_observe(health, end)?;
        exact(&final_target, &target)?;
        admitted_progress(end, || self.progress(health))?;
        let mut lease = DevelopmentTargetLease {
            owner: self,
            health,
            target,
            runtime,
            end,
            steps: 0,
        };
        renderer.render_output(text, &mut lease)?;
        lease.current()?;
        let rendered_target = lease.target.clone();
        let rendered_runtime = lease.runtime.clone();
        drop(lease);
        drop(renderer);
        let rendered = self.output_capture(health, &rendered_target, end)?;
        backend.record_rendered(&rendered_runtime, &rendered_target, &rendered)?;
        log::info!("Resident answer submitted; persistence remains unverified");
        Ok(Outcome::SubmittedUnverified)
    }
}

/// Explicit development authority, NOT SDK PageObservation or durable permission.
/// No public constructor, Clone, serialization or saved-fact conversion exists.
/// The owner can mint it only after its real full boundary and fresh target Observe.
pub struct DevelopmentTargetLease<'a> {
    owner: &'a mut ResidentSourceOwner,
    health: &'a mut ServiceHealth,
    target: remarkable_open_sdk::native_adapter::ReaderFacts,
    runtime: RuntimeIdentity,
    end: Instant,
    steps: usize,
}
impl DevelopmentTargetLease<'_> {
    pub fn current(&mut self) -> Result<()> {
        ensure!(
            self.owner
                .session
                .as_ref()
                .context("target Session absent")?
                .identity()
                == self.runtime,
            "target runtime changed"
        );
        admitted_progress(self.end, || self.owner.progress(self.health))
    }
    /// At most512 semantic lower effects in ONE original90s operation; a failed
    /// step remains consumed and no replacement/retry token can be issued.
    pub fn step(
        &mut self,
        action: impl FnOnce(&mut dyn FnMut() -> Result<()>) -> Result<()>,
    ) -> Result<()> {
        ensure!(self.steps < 512, "development render step bound");
        self.current()?;
        let result = self.owner.session.as_mut().unwrap().delivery(
            self.health,
            self.owner.input.as_mut().context("target input absent")?,
            self.end,
        );
        if let Err(error) = result {
            return self.owner.failure(self.health, error);
        }
        let observed = self.owner.output_observe(self.health, self.end)?;
        super::resident_output::exact(&observed, &self.target)?;
        self.current()?;
        self.steps += 1;
        let result = action(&mut || self.current());
        let closing = self.current();
        match (result, closing) {
            (Err(primary), Err(closing)) => {
                Err(primary.context(format!("target closing check: {closing:#}")))
            }
            (Err(primary), _) => Err(primary),
            (Ok(()), Err(closing)) => Err(closing),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::workflow::resident_runtime::tests::Fixture;
    use crate::workflow::{
        resident_client,
        resident_output::{
            PageUse, ResidentOutputBackend, ResidentOutputOutcome, ResidentRenderer,
        },
    };
    use crate::{
        conversation::{HistoricalPageAssociation, Ledger, Outcome, Record},
        storage::{Namespace, StorePaths, Uuid as ConversationId},
        workflow::{orchestrator::Orchestrator, Workflow},
    };
    use nix::libc;
    use remarkable_open_sdk::{
        native_adapter::{
            Command, ConnectionIdentity, Frame, HistoricalCapture, InsertAfter, Message,
            ReaderFacts,
        },
        navigation::LogicalDirection,
    };
    use std::{
        cell::{Cell, RefCell},
        os::unix::net::UnixDatagram,
        path::PathBuf,
        rc::Rc,
    };
    type Events = Rc<RefCell<Vec<&'static str>>>;
    struct OwnedSession {
        runtime: ResidentRuntime,
        _fixture: Fixture,
        events: Events,
        delivery_fault: bool,
        #[cfg(feature = "installed-resident-output")]
        writer_slot: Option<Rc<super::super::resident_supervisor::writer_request::FixtureSlot>>,
    }
    struct OwnedAdmission<'a> {
        health: &'a mut ServiceHealth,
        input: &'a mut RequestInput,
        policy_fault: bool,
    }
    impl ReadOnlyAdmission for OwnedAdmission<'_> {
        fn pulse(&mut self, deadline: Instant) -> Result<()> {
            super::super::resident_request::admission(
                deadline,
                &mut || self.input.poll(),
                &mut || self.health.progress(),
            )
        }
        fn boundary(&mut self, deadline: Instant) -> Result<()> {
            self.pulse(deadline)?;
            if self.policy_fault {
                return Err(super::super::resident_request::authority_error(
                    anyhow::anyhow!("delivery loaded policy changed"),
                ));
            }
            Ok(())
        }
    }
    impl SourceSession for OwnedSession {
        #[cfg(feature = "installed-resident-output")]
        fn take_writer_request(
            &self,
            document: ConversationId,
            page: ConversationId,
        ) -> Result<Option<super::super::resident_supervisor::writer_request::InstalledRequest>>
        {
            use super::super::resident_supervisor::writer_request::{
                InstalledRequest, RegionalRequest,
            };
            if let Some(slot) = &self.writer_slot {
                return slot.take_on_page(document, page);
            }
            let mut events = self.events.borrow_mut();
            let Some(index) = events
                .iter()
                .position(|event| *event == "current-target-writer-queued")
            else {
                return Ok(None);
            };
            events.remove(index);
            events.push("current-target-writer-received");
            Ok(Some(InstalledRequest::GenerateCurrentTarget(
                RegionalRequest {
                    document,
                    page,
                    operation: ConversationId::new_v4(),
                    request: "Make a note about the current selected source".into(),
                    guidance: None,
                },
            )))
        }

        fn companion_pid(&self) -> Option<u32> {
            self.runtime.fixture_companion_pid()
        }
        fn pulse(&mut self, health: &mut ServiceHealth) -> Result<()> {
            health.progress()?;
            self.runtime.pulse()
        }
        fn delivery(
            &mut self,
            health: &mut ServiceHealth,
            input: &mut RequestInput,
            deadline: Instant,
        ) -> std::result::Result<(), RequestFailure> {
            let result = self.runtime.read_only_boundary(
                deadline,
                &mut OwnedAdmission {
                    health,
                    input,
                    policy_fault: self.delivery_fault,
                },
            );
            super::super::resident_request::finish(&mut self.runtime, result)
        }
        fn runtime(&self) -> &ResidentRuntime {
            &self.runtime
        }
        fn reconnect(
            &mut self,
            health: &mut ServiceHealth,
            input: &mut RequestInput,
            deadline: Instant,
        ) -> std::result::Result<(), RequestFailure> {
            assert!(deadline.saturating_duration_since(Instant::now()) <= Duration::from_secs(30));
            self.events.borrow_mut().push("reconnect");
            let result = self.runtime.reconnect_read_only(
                deadline,
                &mut OwnedAdmission {
                    health,
                    input,
                    policy_fault: false,
                },
            );
            super::super::resident_request::finish(&mut self.runtime, result)
        }
        fn request(
            &mut self,
            health: &mut ServiceHealth,
            input: &mut RequestInput,
            operation: OperationId,
            request: Request,
            deadline: Instant,
        ) -> std::result::Result<Reply, RequestFailure> {
            assert!(deadline.saturating_duration_since(Instant::now()) <= Duration::from_secs(10));
            self.events.borrow_mut().push(match &request {
                Request::Observe => "observe",
                Request::CaptureSource(_) => "capture",
                _ => panic!("effect request"),
            });
            let result = self.runtime.request_read_only(
                operation,
                request,
                deadline,
                &mut OwnedAdmission {
                    health,
                    input,
                    policy_fault: false,
                },
            );
            super::super::resident_request::finish(&mut self.runtime, result)
        }
        fn insert(
            &mut self,
            health: &mut ServiceHealth,
            input: &mut RequestInput,
            conversation: ConversationId,
            operation: OperationId,
            insertion: InsertAfter,
            deadline: Instant,
        ) -> std::result::Result<Reply, RequestFailure> {
            self.events.borrow_mut().push("insert");
            let result = self.runtime.insert_admitted(
                conversation,
                operation,
                insertion,
                deadline,
                &mut OwnedAdmission {
                    health,
                    input,
                    policy_fault: false,
                },
            );
            super::super::resident_request::finish(&mut self.runtime, result)
        }
        fn finish(&mut self) {
            self.runtime.retire_connection();
        }
    }
    struct OwnedFactory {
        source: Option<(ConversationId, ConversationId)>,
        events: Events,
        mode: &'static str,
        delay: Duration,
    }
    impl Factory for OwnedFactory {
        fn activate(
            &mut self,
            _: &InstalledReaderSelection,
            store: Arc<Store>,
            health: &mut ServiceHealth,
            input: &mut RequestInput,
            deadline: Instant,
        ) -> Result<Box<dyn SourceSession>> {
            self.events.borrow_mut().push("activate");
            assert!(deadline.saturating_duration_since(Instant::now()) <= ACQUISITION);
            input.require_quiet()?;
            health.progress()?;
            std::thread::sleep(self.delay);
            before(deadline)?;
            let fixture = Fixture::new();
            if let Some((document, page)) = self.source {
                fixture.declare_source(document, page);
            }
            let runtime = if self.mode == "source-handoff-companion" {
                fixture.start_source_companion_with_store(
                    store,
                    Duration::from_secs(5).min(deadline.saturating_duration_since(Instant::now())),
                )?
            } else {
                fixture.start_with_store(
                    store,
                    self.mode,
                    Duration::from_secs(5).min(deadline.saturating_duration_since(Instant::now())),
                )?
            };
            before(deadline)?;
            self.events.borrow_mut().push("spawn");
            Ok(Box::new(OwnedSession {
                runtime,
                _fixture: fixture,
                events: self.events.clone(),
                delivery_fault: self.mode == "reconnect-source-handoff-delivery-policy",
                #[cfg(feature = "installed-resident-output")]
                writer_slot: None,
            }))
        }
    }
    fn quiet_guard() -> RequestInput {
        // Finite scripted supply. Exhaustion remains an invariant failure in
        // InputObserver; do not globally turn unexpected polls into quiet input.
        RequestInput::scripted((0..4096).map(|_| Ok(vec![])).collect())
    }
    fn released(at: Instant) -> ReleasedReaderInput {
        ReleasedReaderInput {
            released_at: at,
            gesture: ResidentGesture::Reader,
            input: quiet_guard(),
        }
    }
    fn workflow(
        mode: &'static str,
        releases: Vec<ReleasedReaderInput>,
        delay: Duration,
    ) -> (Workflow, Arc<Store>, PathBuf, Events, UnixDatagram) {
        let base = std::env::temp_dir().join(format!("resident-workflow-{}", uuid::Uuid::new_v4()));
        let store = Arc::new(
            Store::open(StorePaths {
                data: base.join("data"),
                cache: base.join("cache"),
                credentials: base.join("credentials"),
            })
            .unwrap(),
        );
        let events = Rc::new(RefCell::new(vec![]));
        let backend =
            super::super::selected_backend::RefusingBackend::with_resident_releases(releases);
        let mut workflow = Workflow::with_device(Box::new(backend), false);
        let (socket, receiver) = UnixDatagram::pair().unwrap();
        workflow
            .enable_service_health(ServiceHealth::fixture(socket))
            .unwrap();
        workflow
            .configure_resident_source(InstalledReaderSelection::fixture(), store.clone())
            .unwrap();
        workflow.resident_source.as_mut().unwrap().factory = Box::new(OwnedFactory {
            source: None,
            events: events.clone(),
            mode,
            delay,
        });
        (workflow, store, base, events, receiver)
    }

    #[cfg(feature = "installed-resident-output")]
    #[test]
    fn ordinary_installed_current_target_reaches_writer_without_reading_the_pdf() {
        use std::{
            ffi::CString,
            fs::{self, File},
            io::Read,
            os::{
                fd::{AsRawFd, FromRawFd},
                unix::{ffi::OsStrExt, fs::DirBuilderExt},
            },
        };

        let mode = "reconnect-source-handoff";
        let (mut workflow, store, base, events, _notify) =
            workflow(mode, vec![released(Instant::now())], Duration::ZERO);
        let document = ConversationId::new_v4();
        let page = ConversationId::new_v4();
        let root = base.join("writer-pdf");
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let table = root.join(format!("{document}.content"));
        let pdf = root.join(format!("{document}.pdf"));
        crate::storage::files::atomic_json(
            &table,
            &serde_json::json!({
                "fileType": "pdf",
                "cPages": {"original": {"value": 1}, "pages": [
                    {"id": page, "idx": {"value": "a"}, "redir": {"value": 0}}
                ]}
            }),
        )
        .unwrap();
        fs::write(&pdf, vec![0; 256 * 1024]).unwrap();

        // Observe content access at the filesystem boundary. The Writer must
        // reach dispatch even when either read would outlast its budget.
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        assert!(fd >= 0);
        let mut watch = unsafe { File::from_raw_fd(fd) };
        for path in [&table, &pdf] {
            let path = CString::new(path.as_os_str().as_bytes()).unwrap();
            assert!(
                unsafe {
                    libc::inotify_add_watch(watch.as_raw_fd(), path.as_ptr(), libc::IN_ACCESS)
                } >= 0
            );
        }

        let owner = workflow.resident_source.as_mut().unwrap();
        owner.selection = InstalledReaderSelection::fixture_with_installed_output();
        owner.ordinary_pdf_fixture = Some((root, PathBuf::new()));
        owner.factory = Box::new(OwnedFactory {
            source: Some((document, page)),
            events: events.clone(),
            mode,
            delay: Duration::ZERO,
        });
        events.borrow_mut().push("current-target-writer-queued");
        let config: crate::config::Config = serde_json::from_value(serde_json::json!({
            "paths": store.paths,
            "reader": { "researched_pdf": {} }
        }))
        .unwrap();
        let ledger = Ledger::new(store.clone());
        let dispatch = super::super::conversation_dispatch::Dispatch::local(&config, &ledger)
            .unwrap()
            .with_regional_writer_model("fixture".into());
        let calls = Rc::new(Cell::new(0));
        let provider = Provider {
            calls: calls.clone(),
            batches: Rc::new(RefCell::new(vec![])),
            images: vec![],
            payload_delay: Duration::ZERO,
            kill_pid: None,
            selection_subject: Some("24 / 6"),
        };
        let mut reader = Orchestrator::new(workflow, provider, ledger);
        let error = reader
            .run_iteration_with_trigger(true, Some(&dispatch))
            .unwrap_err();
        assert!(
            error.to_string().contains("requires a bound conversation"),
            "{error:#}"
        );
        assert!(events.borrow().contains(&"current-target-writer-received"));
        assert_eq!(calls.get(), 0);
        assert_eq!(
            watch.read(&mut [0; 4096]).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        drop(reader);
        drop(dispatch);
        drop(store);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[cfg(feature = "installed-resident-output")]
    #[test]
    fn ordinary_installed_current_target_places_the_masked_regional_draft() {
        use super::super::resident_supervisor::writer_request::{FixtureSlot, RegionalRequest};
        use crate::conversation::{
            resident_capture::tests::capture_for,
            target::{geometry::Geometry, Candidate, Understanding},
            target_source::tests::Fixture as StoreFixture,
            BindingAcquisition, BindingReceipt, ExpectedHeads, Mode, Role, SourceObservation, Turn,
        };
        use base64::{engine::general_purpose::STANDARD, Engine};

        #[derive(Default)]
        struct RegionalModel {
            calls: Rc<Cell<usize>>,
            images: Vec<String>,
            text: Vec<String>,
        }

        impl crate::LLMEngine for RegionalModel {
            fn guarded_timeout(&self) -> Option<Duration> {
                Some(Duration::from_secs(1))
            }
            fn execute_with_handoff(
                &mut self,
                progress: &mut dyn FnMut() -> Result<()>,
                handoff: &mut crate::llm::ModelHandoff<'_>,
            ) -> Result<String> {
                progress()?;
                let mut result = None;
                handoff(&mut || {
                    result = Some(self.execute());
                    Ok(())
                })?;
                result.context("regional fixture provider was not dispatched")?
            }
            fn add_text_content(&mut self, text: &str) {
                self.text.push(text.into());
            }
            fn add_image_content(&mut self, image: &str) {
                self.images.push(image.into());
            }
            fn clear_content(&mut self) {
                self.images.clear();
                self.text.clear();
            }
            fn execute(&mut self) -> Result<String> {
                let call = self.calls.get();
                self.calls.set(call + 1);
                match call {
                    0 => {
                        let [image] = self.images.as_slice() else {
                            panic!("normalization must receive one masked original");
                        };
                        let png = STANDARD.decode(image).unwrap();
                        let pixels =
                            image::load_from_memory_with_format(&png, image::ImageFormat::Png)
                                .unwrap()
                                .to_rgba8();
                        assert_eq!(pixels.dimensions(), (12, 20));
                        assert_eq!(pixels.get_pixel(1, 1).0, [17, 29, 41, 255]);
                        assert_eq!(pixels.get_pixel(10, 1).0, [255, 255, 255, 255]);
                        Ok(
                            r#"{"pages":[{"page":1,"status":"transcribed","text":"24 / 6 = 4"}]}"#
                                .into(),
                        )
                    }
                    1 => {
                        assert!(self.images.is_empty());
                        assert!(self.text.iter().any(|part| part.contains("24 / 6 = 4")));
                        Ok("# Regional note\n\nSelected equation equals four.".into())
                    }
                    _ => panic!("unexpected extra model request"),
                }
            }
        }

        let base = StoreFixture(std::env::temp_dir().join(format!(
            "resident-current-target-{}",
            ConversationId::new_v4(),
        )));
        let config = crate::config::Config {
            paths: StorePaths {
                data: base.0.join("data"),
                cache: base.0.join("cache"),
                credentials: base.0.join("credentials"),
            },
            ..Default::default()
        };
        let store = Arc::new(Store::open_source_target_startup(&config).unwrap());
        let ledger = Ledger::new(store.clone());
        let document = ConversationId::new_v4();
        let original = capture_for(document);
        let page = original.page();
        let conversation = ConversationId::new_v4();
        ledger
            .create(conversation, ConversationId::new_v4(), 1)
            .unwrap();
        let image_id = ConversationId::new_v4();
        let mut user = Turn {
            id: ConversationId::new_v4(),
            conversation,
            exchange: ConversationId::new_v4(),
            sequence: 0,
            role: Role::User,
            mode: Mode::Reader,
            outcome: Outcome::Prepared,
            text: None,
            sources: vec![],
            correction_of: None,
            created_ms: 1,
            updated_ms: 1,
            completion: None,
        };
        ledger
            .prepare_resident_historical(
                ConversationId::new_v4(),
                ledger
                    .expected(Namespace::Conversation, conversation)
                    .unwrap(),
                user.clone(),
                image_id,
                &original,
            )
            .unwrap();
        let mut frame = ledger.target_source_frame(conversation, image_id).unwrap();
        let user_heads = ledger.expected(Namespace::Conversation, user.id).unwrap();
        user.outcome = Outcome::Interpreted;
        user.updated_ms = 2;
        user.text = Some("What is this?".into());
        user.sources = vec![image_id];
        ledger
            .advance_with_source_target(
                ConversationId::new_v4(),
                user_heads,
                user,
                &mut frame,
                &Candidate {
                    subject: "24 / 6".into(),
                    geometry: Some(Geometry {
                        components: vec![vec![
                            [0, 0],
                            [500_000, 0],
                            [500_000, 1_000_000],
                            [0, 1_000_000],
                        ]],
                    }),
                    understanding: Understanding::Interpreted,
                },
                Some(
                    crate::analysis::SelectionCenter::from_pixels(192.0, 512.0, 768, 1024).unwrap(),
                ),
            )
            .unwrap();
        let target = ledger
            .current_resident_writer_target(conversation, document, page)
            .unwrap()
            .unwrap();

        let native = Fixture::new();
        native.declare_source(document, page);
        let runtime = native
            .start_with_store(
                store.clone(),
                "reconnect-source-handoff",
                Duration::from_secs(5),
            )
            .unwrap();
        let profile = InstalledReaderSelection::fixture_for_installed_output(
            &base.0.join("output-profile"),
            runtime.identity(),
        )
        .unwrap();
        let hex = |bytes: [u8; 32]| {
            bytes
                .into_iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let bind = ConversationId::new_v4();
        ledger
            .bind(
                bind,
                ledger
                    .expected(Namespace::Conversation, conversation)
                    .unwrap(),
                ExpectedHeads::default(),
                BindingReceipt {
                    operation: bind,
                    conversation,
                    request_fingerprint: "fixture-request".into(),
                    adapter_fingerprint: hex(runtime.identity().adapter_sha256),
                    source: SourceObservation {
                        document,
                        page,
                        session: "fixture-session".into(),
                        visit: "fixture-visit".into(),
                        content_revision: None,
                        order_revision: None,
                        capability_revision: hex(runtime.identity().profile_sha256),
                        evidence_procedure: "fixture-source".into(),
                    },
                    intended_target: Some(page),
                    observed_document: document,
                    observed_page: page,
                    expected_order: vec![page],
                    observed_order: vec![page],
                    persisted_revision: "fixture-binding".into(),
                    observed_ms: 3,
                    evidence_origin: "installed-native-guarded-v1".into(),
                    acquisition: BindingAcquisition::ExistingCurrent,
                },
            )
            .unwrap();

        let operation = ConversationId::new_v4();
        let slot = Rc::new(FixtureSlot::current_target(RegionalRequest {
            document,
            page,
            operation,
            request: "Make a note about this marked source".into(),
            guidance: None,
        }));
        let preview = Rc::new(RefCell::new(vec![]));
        let backend =
            super::super::selected_backend::RefusingBackend::with_resident_releases(vec![
                released(Instant::now()),
            ])
            .with_test_output(Box::new(RecognitionProbe {
                writer_preview: Some(preview.clone()),
                ..RecognitionProbe::default()
            }));
        let mut workflow = Workflow::with_device(Box::new(backend), false);
        let (socket, _notify) = UnixDatagram::pair().unwrap();
        workflow
            .enable_service_health(ServiceHealth::fixture(socket))
            .unwrap();
        workflow
            .configure_resident_source(profile, store.clone())
            .unwrap();
        let events = Rc::new(RefCell::new(vec![]));
        workflow.resident_source.as_mut().unwrap().session = Some(Box::new(OwnedSession {
            runtime,
            _fixture: native,
            events: events.clone(),
            delivery_fault: false,
            writer_slot: Some(slot.clone()),
        }));
        let dispatch = super::super::conversation_dispatch::Dispatch::local(&config, &ledger)
            .unwrap()
            .with_regional_writer_model("fixture".into());
        let provider = RegionalModel::default();
        let calls = provider.calls.clone();
        let mut reader = Orchestrator::new(workflow, provider, Ledger::new(store.clone()));
        reader
            .run_iteration_with_trigger(true, Some(&dispatch))
            .unwrap();

        assert_eq!(calls.get(), 2);
        assert!(!slot.pending());
        assert!(!events.borrow().contains(&"insert"));
        assert_eq!(
            preview.borrow().as_slice(),
            &["Regional note\n\nAbout: 24 / 6\n\nSelected equation equals four.\n"],
        );
        let records = ledger.inspect(conversation, false).unwrap();
        assert!(records.iter().any(|record| matches!(record,
            Record::RegionSource(region) if region.target == target
                && region.normalized.document.document == document)));
        assert!(records.iter().any(|record| matches!(record,
            Record::Turn(turn) if turn.exchange == operation
                && turn.role == Role::Assistant
                && turn.mode == Mode::Writer
                && turn.outcome == Outcome::Generated)));
    }

    #[cfg(feature = "installed-resident-output")]
    pub(crate) fn ordinary_pdf_receiver_fixture<M: crate::LLMEngine>(
        document: ConversationId,
        page: ConversationId,
        root: PathBuf,
        executable: PathBuf,
        model: impl FnOnce(&Ledger, PathBuf) -> M,
    ) -> Result<(Result<()>, Vec<Record>)> {
        let mode = "reconnect-source-handoff-ordinary-pdf";
        let (mut workflow, store, base, events, _notify) =
            workflow(mode, vec![released(Instant::now())], Duration::ZERO);
        let native = Fixture::new();
        native.declare_source(document, page);
        let runtime = native.start_with_store(store.clone(), mode, Duration::from_secs(5))?;
        let order_change = native.ordinary_pdf_order_change();
        let owner = workflow.resident_source.as_mut().unwrap();
        owner.selection = InstalledReaderSelection::fixture_with_installed_output();
        owner.ordinary_pdf_fixture = Some((root, executable));
        owner.session = Some(Box::new(OwnedSession {
            runtime,
            _fixture: native,
            events: events.clone(),
            delivery_fault: false,
            writer_slot: None,
        }));
        crate::storage::files::atomic(
            &store.paths.credentials.join("fixture_model_key"),
            b"PRIVATE_RESEARCH_FIXTURE",
        )?;
        let config: crate::config::Config = serde_json::from_value(serde_json::json!({
            "paths": store.paths,
            "model": "gpt-6.1-terra",
            "model_credential": "fixture_model_key",
            "reader": { "researched_pdf": { "research_model": "gpt-5.6-terra" } },
            "research_policy": "required"
        }))?;
        config.validate_values()?;
        let ledger = Ledger::new(store.clone());
        let dispatch = crate::workflow::conversation_dispatch::Dispatch::local(&config, &ledger)?;
        let mut reader = Orchestrator::new(
            workflow,
            model(&ledger, order_change),
            Ledger::new(store.clone()),
        );
        reader.set_local_handwriting(&dispatch)?;
        let result = reader.run_iteration_with_trigger(true, Some(&dispatch));
        {
            let events = events.borrow();
            assert_eq!(&events[..2], &["observe", "capture"]);
            assert_eq!(events.len() % 2, 0);
            assert!(events[2..]
                .chunks_exact(2)
                .all(|pair| pair == ["reconnect", "observe"]));
        }
        let records = store
            .snapshot_heads_matching(
                &[Namespace::Conversation],
                crate::storage::MAX_ITEMS,
                |_| true,
            )?
            .iter()
            .map(Ledger::decode)
            .collect::<Result<Vec<_>>>()?;
        drop((reader, dispatch, ledger, store));
        std::fs::remove_dir_all(base)?;
        Ok((result, records))
    }

    #[test]
    fn resident_owner_brackets_original_pdf_source_and_refuses_same_release_retry() {
        use std::{fs, os::unix::fs::DirBuilderExt};

        for refusal in [None, Some("before"), Some("after")] {
            let (mut workflow, store, base, events, _notify) =
                workflow("reconnect-source-handoff", vec![], Duration::ZERO);
            let document = ConversationId::from_u128(81);
            let page = ConversationId::from_u128(82);
            let root = base.join("original-pdf");
            fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
            let pdf_path = root.join(format!("{document}.pdf"));
            if refusal != Some("before") {
                fs::write(&pdf_path, b"synthetic original source").unwrap();
            }
            fs::write(
                root.join(format!("{document}.content")),
                serde_json::to_vec(&serde_json::json!({
                    "fileType": "pdf",
                    "cPages": {"original": {"value": 2}, "pages": [
                        {"id": page, "idx": {"value": "a"}, "redir": {"value": 1}}
                    ]}
                }))
                .unwrap(),
            )
            .unwrap();
            let owner = workflow.resident_source.as_mut().unwrap();
            let health = workflow.service_health.as_mut().unwrap();
            owner.factory = Box::new(OwnedFactory {
                source: Some((document, page)),
                events: events.clone(),
                mode: "reconnect-source-handoff",
                delay: Duration::ZERO,
            });
            let started = Instant::now();
            owner.begin(released(started), health).unwrap();
            let requested = started + Duration::from_secs(20);
            let deadline = owner.original_pdf_deadline(requested, requested).unwrap();
            assert_eq!(deadline, requested);
            let checks = RefCell::new(Vec::new());
            let result = owner.acquire_original_pdf_using(
                health,
                deadline,
                requested,
                |facts| {
                    checks.borrow_mut().push(facts.is_some());
                    Ok(())
                },
                |facts, pulse| {
                    assert_eq!(&*events.borrow(), &["activate", "spawn", "observe"]);
                    original_pdf::ObservedOriginalPdf::observe_at(
                        &root,
                        (unsafe { libc::geteuid() }, None),
                        true,
                        facts,
                        document,
                        deadline,
                        pulse,
                    )
                },
                &mut || Ok(()),
                |pdf, verified, _, pulse| {
                    assert_eq!(
                        &*events.borrow(),
                        &["activate", "spawn", "observe", "capture"]
                    );
                    if refusal == Some("after") {
                        fs::write(&pdf_path, b"changed after capture")?;
                    }
                    pdf.verify_fixture_capture(verified, pulse)
                },
            );
            let expected_checks: &[bool] = if refusal == Some("before") {
                &[false, true]
            } else {
                &[false, true, true]
            };
            assert_eq!(checks.borrow().as_slice(), expected_checks);
            if refusal.is_some() {
                assert!(result.is_err());
            } else {
                let (capture, original_page) = result.unwrap();
                assert_eq!(capture.document(), document);
                assert_eq!(capture.page(), page);
                assert_eq!(original_page, 2);
            }
            assert!(owner.original_pdf_deadline(requested, requested).is_err());
            owner.finish(health).unwrap();
            let expected_events: &[&str] = if refusal == Some("before") {
                &["activate", "spawn", "observe"]
            } else {
                &["activate", "spawn", "observe", "capture"]
            };
            assert_eq!(events.borrow().as_slice(), expected_events);
            drop((workflow, store));
            fs::remove_dir_all(base).unwrap();
        }
    }

    /// Actual installed receiver body with a declared protocol peer, live input
    /// guard/health owner and normal finalization. This is not native authority.
    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    pub(crate) fn fresh_receiver_fixture<E: crate::LLMEngine>(
        store: Arc<Store>,
        writer: &crate::conversation::SelectedAdmission,
        pin: &crate::workflow::resident_supervisor::writer_request::RegionalPin,
        preferences: crate::storage::sync::selected::RequestPreferences,
        engine: &mut E,
    ) -> Result<()> {
        let events = Rc::new(RefCell::new(vec![]));
        let backend =
            super::super::selected_backend::RefusingBackend::with_resident_releases(vec![]);
        let mut workflow = Workflow::with_device(Box::new(backend), false);
        let (socket, _receiver) = UnixDatagram::pair()?;
        workflow.enable_service_health(ServiceHealth::fixture(socket))?;
        workflow.configure_resident_source(InstalledReaderSelection::fixture(), store)?;
        workflow.resident_source.as_mut().unwrap().factory = Box::new(OwnedFactory {
            source: Some((pin.request().document, pin.request().page)),
            events: events.clone(),
            mode: "reconnect-source-handoff",
            delay: Duration::ZERO,
        });
        let started = preferences.regional_cutoff(60)? - Duration::from_secs(60);
        let result = super::super::selected_resident_reader::receive_fresh_request(
            &mut workflow,
            writer,
            released(started),
            pin,
            preferences,
            engine,
            None,
            "fixture-model",
        );
        assert!(!workflow.runtime_failed());
        let owner = workflow.resident_source.as_ref().unwrap();
        assert!(
            owner.input.is_none() && owner.deadline.is_none(),
            "normal Workflow finalization retired source/input guard"
        );
        assert_eq!(
            &*events.borrow(),
            &["activate", "spawn", "observe", "capture"]
        );
        assert!(
            !writer
                .writer_store()
                .writer_active
                .load(std::sync::atomic::Ordering::Acquire),
            "original Writer owner released only after finalization"
        );
        result
    }

    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    pub(crate) fn assert_selected_policy_boundaries(mut verify: impl FnMut(bool) -> Result<()>) {
        let expected: [&[&str]; 4] = [
            &[],
            &[],
            &["activate", "spawn"],
            &["activate", "spawn", "observe"],
        ];
        for (boundary, events_expected) in expected.into_iter().enumerate() {
            verify(false).unwrap();
            let (mut workflow, store, base, events, _notify) =
                workflow("reconnect-source-handoff", vec![], Duration::ZERO);
            let owner = workflow.resident_source.as_mut().unwrap();
            let health = workflow.service_health.as_mut().unwrap();
            let result = match boundary {
                0 => owner.activate_at_startup_with_input(
                    health,
                    Some(Instant::now() + Duration::from_secs(10)),
                    || Ok(quiet_guard()),
                    || verify(true),
                ),
                1 => owner.begin_with_check(released(Instant::now()), health, || verify(true)),
                _ => {
                    owner
                        .begin_with_check(released(Instant::now()), health, || verify(false))
                        .unwrap();
                    owner
                        .acquire_with_check(health, |observed| {
                            verify((boundary == 3) == observed.is_some())
                        })
                        .map(|_| ())
                }
            };
            assert!(result.is_err(), "policy boundary {boundary} accepted");
            assert_eq!(
                &*events.borrow(),
                events_expected,
                "policy boundary {boundary}"
            );
            if boundary > 0 {
                crate::workflow::selected_resident_reader::finish_selected_request(&mut workflow)
                    .unwrap();
            }
            drop((workflow, store));
            std::fs::remove_dir_all(base).unwrap();
        }
        verify(false).unwrap();
    }
    struct Provider {
        calls: Rc<Cell<usize>>,
        batches: Rc<RefCell<Vec<Vec<String>>>>,
        images: Vec<String>,
        payload_delay: Duration,
        kill_pid: Option<u32>,
        selection_subject: Option<&'static str>,
    }
    impl crate::LLMEngine for Provider {
        fn add_text_content(&mut self, _: &str) {}
        fn add_image_content(&mut self, image: &str) {
            self.images.push(image.into());
            std::thread::sleep(self.payload_delay);
        }
        fn clear_content(&mut self) {
            self.images.clear();
        }
        fn execute(&mut self) -> Result<String> {
            if let Some(pid) = self.kill_pid.take() {
                kill_owned(pid);
            }
            let index = self.calls.get();
            self.calls.set(index + 1);
            self.batches.borrow_mut().push(self.images.clone());
            let Some(subject) = self.selection_subject else {
                return Ok("NONE".into());
            };
            Ok(if index.is_multiple_of(2) {
                format!("QUESTION: why?\nQUESTION_BOX: 130,35,200,50\nSELECTION_CENTER: 385,230\nSELECTION_SUBJECT: {subject}\n---\nANSWER: Because the model fixture says so.")
            } else {
                "TRANSCRIPTION: why?".into()
            })
        }
    }
    #[test]
    fn selected_workflow_two_requests_reuse_actual_peer_and_deliver_only_stored_provider_images() {
        let (workflow, store, base, events, _notify) = workflow(
            "reconnect-source-handoff",
            vec![released(Instant::now()), released(Instant::now())],
            Duration::ZERO,
        );
        let calls = Rc::new(Cell::new(0));
        let batches = Rc::new(RefCell::new(vec![]));
        let model = Provider {
            calls: calls.clone(),
            batches: batches.clone(),
            images: vec![],
            payload_delay: Duration::ZERO,
            kill_pid: None,
            selection_subject: Some("24 / 6"),
        };
        let mut reader = Orchestrator::new(workflow, model, Ledger::new(store.clone()));
        reader.run_iteration().unwrap();
        reader.run_iteration().unwrap();
        assert_eq!(calls.get(), 4);
        assert_eq!(
            &*events.borrow(),
            &[
                "activate",
                "spawn",
                "observe",
                "capture",
                "reconnect",
                "observe",
                "capture"
            ]
        );
        let stored = store
            .snapshot_heads_matching(&[Namespace::Source], 10, |_| true)
            .unwrap();
        assert_eq!(stored.len(), 2);
        use base64::Engine;
        for value in stored {
            let Record::ResidentCapture(evidence) =
                serde_json::from_value::<Record>(value.payload).unwrap()
            else {
                panic!("wrong historical origin")
            };
            let ledger = Ledger::new(store.clone());
            let actual = ledger.stored_resident_images(evidence.id).unwrap();
            let images = actual
                .images
                .iter()
                .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
                .collect::<Vec<_>>();
            assert!(
                batches
                    .borrow()
                    .iter()
                    .filter(|batch| **batch == images)
                    .count()
                    >= 2
            );
            let history = ledger.inspect(evidence.conversation, false).unwrap();
            assert!(history.iter().any(|r| matches!(r, Record::Turn(t) if t.role == crate::conversation::Role::Assistant && t.text.is_some())));
            assert!(!history.iter().any(|r| matches!(
                r,
                Record::Binding(_)
                    | Record::Capture(_)
                    | Record::DevelopmentCapture(_)
                    | Record::LegacyCapture(_)
            )));
            assert!(history
                .iter()
                .all(|r| !matches!(r, Record::Turn(t) if t.outcome == Outcome::Completed)));
        }
        drop(reader);
        drop(store);
        std::fs::remove_dir_all(base).unwrap();
    }
    #[derive(Default)]
    struct RecognitionProbe {
        conversation: Option<ConversationId>,
        confirmed_source: bool,
        #[cfg(feature = "installed-resident-output")]
        writer_preview: Option<Rc<RefCell<Vec<String>>>>,
    }

    #[cfg(feature = "installed-resident-output")]
    struct WriterPreview(Rc<RefCell<Vec<String>>>);

    #[cfg(feature = "installed-resident-output")]
    impl ResidentRenderer for WriterPreview {
        fn render(&mut self, text: &str, lease: &mut DevelopmentTargetLease<'_>) -> Result<()> {
            lease.step(|pulse| {
                pulse()?;
                self.0.borrow_mut().push(text.to_owned());
                pulse()
            })
        }
    }

    #[cfg(feature = "installed-resident-output")]
    #[test]
    fn installed_reader_reopens_literal_history_with_the_original_overview_target() {
        assert_installed_reader_subject_history("24 / 6", Some("24 / 6"));
    }

    #[cfg(feature = "installed-resident-output")]
    #[test]
    fn installed_reader_keeps_an_uncertain_subject_out_of_history() {
        assert_installed_reader_subject_history("NONE", None);
    }

    #[cfg(feature = "installed-resident-output")]
    fn assert_installed_reader_subject_history(
        proposed_subject: &'static str,
        retained_subject: Option<&str>,
    ) {
        use crate::conversation::{receiving::answer::follow_up, ContextBudget, Role};

        #[derive(Default)]
        struct Continuation {
            payload: serde_json::Value,
            images: Vec<String>,
            observed: Option<(serde_json::Value, Vec<String>)>,
        }
        impl crate::LLMEngine for Continuation {
            fn add_text_content(&mut self, text: &str) {
                if let Ok(payload) = serde_json::from_str(text) {
                    self.payload = payload;
                }
            }
            fn add_image_content(&mut self, image: &str) {
                self.images.push(image.into());
            }
            fn clear_content(&mut self) {
                self.payload = serde_json::Value::Null;
                self.images.clear();
            }
            fn execute(&mut self) -> Result<String> {
                self.observed = Some((self.payload.clone(), self.images.clone()));
                Ok("follow-up fixture".into())
            }
        }

        let (mut workflow, store, base, _events, _notify) = workflow(
            "reconnect-source-handoff",
            vec![released(Instant::now())],
            Duration::ZERO,
        );
        workflow.resident_source.as_mut().unwrap().selection =
            InstalledReaderSelection::fixture_with_installed_output();
        let batches = Rc::new(RefCell::new(vec![]));
        let calls = Rc::new(Cell::new(0));
        let provider = Provider {
            calls: calls.clone(),
            batches: batches.clone(),
            images: vec![],
            payload_delay: Duration::ZERO,
            kill_pid: None,
            selection_subject: Some(proposed_subject),
        };
        let paths = store.paths.clone();
        let mut reader = Orchestrator::new(workflow, provider, Ledger::new(store.clone()));
        reader.run_iteration().unwrap();
        let entries = store
            .snapshot_heads_matching(&[Namespace::Source], 2, |_| true)
            .unwrap();
        let [entry] = entries.as_slice() else {
            panic!("expected the original capture")
        };
        let Record::ResidentCapture(capture) = Ledger::decode(entry).unwrap() else {
            panic!("resident capture")
        };
        let target = capture.reader_target.unwrap();
        assert!((target[0] - 385.0 / 768.0).abs() < 1e-12);
        assert!((target[1] - 230.0 / 1024.0).abs() < 1e-12);
        assert_eq!(
            capture.schema,
            if retained_subject.is_some() { 3 } else { 2 }
        );
        assert_eq!(calls.get(), 2);
        assert_eq!(
            capture
                .reader_subject
                .as_ref()
                .map(crate::analysis::ReaderSourceSubject::as_str),
            retained_subject
        );
        drop(reader);
        drop(store);
        let ledger = Ledger::new(Arc::new(Store::open(paths).unwrap()));
        let records = ledger.inspect(capture.conversation, false).unwrap();
        let turns = records
            .iter()
            .filter_map(|record| match record {
                Record::Turn(turn) => Some(turn),
                _ => None,
            })
            .collect::<Vec<_>>();
        let [question, answer] = turns.as_slice() else {
            panic!("expected separate role turns")
        };
        assert_eq!(question.role, Role::User);
        assert_eq!(question.text.as_deref(), Some("why?"));
        assert_eq!(answer.role, Role::Assistant);
        assert_eq!(
            answer.text.as_deref(),
            Some("Because the model fixture says so.")
        );
        assert_eq!(question.sources, vec![capture.id]);
        assert_eq!(answer.sources, question.sources);
        assert_eq!(answer.outcome, Outcome::Failed);
        let context = ledger
            .context(
                capture.conversation,
                &ContextBudget {
                    max_text_bytes: 16 * 1024,
                    max_turns: 64,
                    provider_token_limit: Some(16 * 1024),
                },
                |_| Ok(10),
            )
            .unwrap();
        let mut continuation = Continuation::default();
        follow_up(
            &ledger,
            &mut continuation,
            &context,
            "Why again?",
            &crate::config::Personalization::default()
                .snapshot()
                .unwrap(),
            None,
            &mut || Ok(()),
        )
        .unwrap();
        let (payload, images) = continuation.observed.unwrap();
        assert_eq!(payload["history"], serde_json::json!([question]));
        assert_eq!(images, batches.borrow()[0]);
        let images = payload["historical_images"].as_array().unwrap();
        assert_eq!(images.len(), 4);
        assert_eq!(images[0]["dimensions"], serde_json::json!([768, 1024]));
        assert_eq!(images[0]["source"], serde_json::json!(capture.id));
        assert_eq!(images[0]["sha256"], capture.images[0].media.sha256);
        let mut expected_target = serde_json::json!({
            "turn": question.id,
            "document": capture.before.document,
            "page": capture.before.page,
            "normalized": target,
        });
        if let Some(subject) = retained_subject {
            expected_target["subject"] = serde_json::json!(subject);
        }
        assert_eq!(images[0]["reader_target"], expected_target);
        assert!(images[1..]
            .iter()
            .all(|image| image.get("reader_target").is_none()));
        drop(ledger);
        std::fs::remove_dir_all(base).unwrap();
    }

    impl ResidentOutputBackend for RecognitionProbe {
        fn admit(
            &mut self,
            _: &RuntimeIdentity,
            _: Option<&HistoricalPageAssociation>,
            _: Instant,
        ) -> Result<()> {
            Ok(())
        }

        fn classify(
            &mut self,
            _: &Ledger,
            conversation: ConversationId,
            _: &RuntimeIdentity,
            _: &ReaderFacts,
            _: &HistoricalCapture,
            _: bool,
            _: Instant,
            _: &mut dyn FnMut() -> Result<()>,
        ) -> Result<PageUse> {
            assert!(self.conversation.replace(conversation).is_none());
            #[cfg(feature = "installed-resident-output")]
            if self.writer_preview.is_some() {
                return Ok(PageUse::RelevantBuddy);
            }
            Ok(if self.confirmed_source {
                PageUse::ConfirmedNonBuddy
            } else {
                PageUse::Unresolved
            })
        }

        fn adopt(
            &mut self,
            _: &Ledger,
            _: ConversationId,
            _: &RuntimeIdentity,
            _: &ReaderFacts,
            _: Instant,
        ) -> Result<bool> {
            panic!("unexpected adoption");
        }

        fn navigate(
            &mut self,
            _: RequestInput,
            _: LogicalDirection,
            _: &RuntimeIdentity,
            _: Instant,
            _: &mut dyn FnMut() -> Result<()>,
        ) -> Result<RequestInput> {
            panic!("unexpected navigation");
        }

        fn renderer(
            &mut self,
            _: &Ledger,
            _: ConversationId,
            _: &RuntimeIdentity,
            _: &ReaderFacts,
            _: bool,
            _: Instant,
            _: &mut dyn FnMut() -> Result<()>,
        ) -> Result<Box<dyn ResidentRenderer + '_>> {
            #[cfg(feature = "installed-resident-output")]
            if let Some(preview) = &self.writer_preview {
                return Ok(Box::new(WriterPreview(preview.clone())));
            }
            panic!("unexpected renderer request");
        }
    }

    #[test]
    fn reader_records_the_original_insertion_reply_on_the_generated_answer() {
        use crate::conversation::{AttemptReason, Role};
        use crate::workflow::selected_backend::RefusingBackend;
        use remarkable_open_sdk::native_adapter::Rejection;

        for (mode, reply, outcome, reason) in [
            (
                "reconnect-source-handoff-current-rejected",
                Reply::Rejected(Rejection::ExecutorBeforeMutation),
                Outcome::Failed,
                AttemptReason::NoSuccessor,
            ),
            (
                "reconnect-source-handoff-current-indeterminate",
                Reply::Indeterminate,
                Outcome::ReconcileRequired,
                AttemptReason::DeviceUncertain,
            ),
        ] {
            let (mut workflow, store, base, events, _notify) =
                workflow(mode, vec![], Duration::ZERO);
            let fixture = Fixture::new();
            let runtime = fixture
                .start_with_store(store.clone(), mode, Duration::from_secs(5))
                .unwrap();
            let owner = workflow.resident_source.as_mut().unwrap();
            owner.selection = InstalledReaderSelection::fixture_for_output(
                &base.join("output-profile"),
                runtime.identity(),
            )
            .unwrap();
            owner.session = Some(Box::new(OwnedSession {
                runtime,
                _fixture: fixture,
                events: events.clone(),
                delivery_fault: false,
                #[cfg(feature = "installed-resident-output")]
                writer_slot: None,
            }));
            let source = RecognitionProbe {
                confirmed_source: true,
                ..RecognitionProbe::default()
            };
            workflow.device = Box::new(
                RefusingBackend::with_resident_releases(vec![released(Instant::now())])
                    .with_test_output(Box::new(source)),
            );
            let calls = Rc::new(Cell::new(0));
            let model = Provider {
                calls: calls.clone(),
                batches: Rc::new(RefCell::new(vec![])),
                images: vec![],
                payload_delay: Duration::ZERO,
                kill_pid: None,
                selection_subject: Some("24 / 6"),
            };
            let ledger = Ledger::new(store.clone());
            let mut reader = Orchestrator::new(workflow, model, Ledger::new(store.clone()));
            let result = reader.run_iteration();
            if reply == Reply::Indeterminate {
                assert!(result.unwrap_err().to_string().contains("Indeterminate"));
            } else {
                result.unwrap();
            }
            assert_eq!(calls.get(), 2);
            assert_eq!(
                events.borrow().iter().filter(|e| **e == "insert").count(),
                1
            );
            let attempts = resident_client::attempts(&store).unwrap();
            assert_eq!(attempts.len(), 1);
            assert_eq!(attempts[0].reply.as_ref(), Some(&reply));
            let records = ledger.inspect(attempts[0].conversation, false).unwrap();
            let answer = records
                .iter()
                .find_map(|record| match record {
                    Record::Turn(turn) if turn.role == Role::Assistant => Some(turn),
                    _ => None,
                })
                .unwrap();
            assert_eq!(answer.outcome, outcome);
            assert!(answer
                .text
                .as_deref()
                .unwrap()
                .contains("Because the model fixture says so."));
            assert!(records.iter().any(|record| matches!(record,
                Record::OutcomeFact(fact) if fact.turn == answer.id && fact.outcome == outcome && fact.reason == reason
            )));
            assert!(!records
                .iter()
                .any(|record| matches!(record, Record::Binding(_))));
            drop(reader);
            drop(ledger);
            drop(store);
            std::fs::remove_dir_all(base).unwrap();
        }
    }

    #[test]
    fn resident_output_fences_a_different_conversation_after_indeterminate_insertion() {
        // R103 observed an indeterminate insertion with unchanged saved page
        // order. A later conversation is a synthetic follow-on to that run.
        // https://linear.app/magentumdragon/issue/REM-25#comment-ac409995-98cd-4235-887b-afb43cf85904
        for seed_insertion in [false, true] {
            let (mut workflow, store, base, events, _notify) =
                workflow("reconnect-source-handoff", vec![], Duration::ZERO);
            let owner = workflow.resident_source.as_mut().unwrap();
            let health = workflow.service_health.as_mut().unwrap();
            owner.begin(released(Instant::now()), health).unwrap();
            owner.acquire(health).unwrap();
            owner.deliver(health).unwrap();
            let runtime = owner.session.as_ref().unwrap().identity();
            owner.selection = InstalledReaderSelection::fixture_for_output(
                &base.join("output-recovery"),
                &runtime,
            )
            .unwrap();

            let ledger = Ledger::new(store.clone());
            let prior = ConversationId::new_v4();
            let next = ConversationId::new_v4();
            for conversation in [prior, next] {
                ledger
                    .create(conversation, ConversationId::new_v4(), 1)
                    .unwrap();
            }
            let journal = store.paths.data.join("native-insertion-attempts-v1.json");
            let original_journal = if seed_insertion {
                let source = owner.captured_source.clone().unwrap();
                let target = operation().0;
                assert!(!source.order().contains(&target));
                let request = Frame {
                    scope: ConnectionIdentity {
                        runtime,
                        connection: operation().0,
                        server_nonce: Some(operation().0),
                    },
                    sequence: 1,
                    operation: operation(),
                    message: Message::Request(Command::InsertAfter(InsertAfter { source, target })),
                };
                resident_client::seed_indeterminate_insertion(&store, prior, &request).unwrap();
                let attempts = resident_client::attempts(&store).unwrap();
                assert_eq!(attempts.len(), 1);
                assert_eq!(attempts[0].conversation, prior);
                assert_eq!(attempts[0].reply, Some(Reply::Indeterminate));
                Some(std::fs::read(&journal).unwrap())
            } else {
                assert!(!journal.exists());
                None
            };
            let conversation_state = || {
                [prior, next]
                    .map(|id| serde_json::to_value(ledger.inspect(id, false).unwrap()).unwrap())
            };
            let original_state = conversation_state();
            events.borrow_mut().clear();
            let mut backend = RecognitionProbe::default();

            let outcome = owner
                .render_answer(
                    health,
                    &mut backend,
                    &ledger,
                    next,
                    super::super::resident_output::ResidentOutputText::Plain(
                        "answer for the new conversation",
                    ),
                    None,
                    &mut || panic!("unexpected effect callback"),
                )
                .unwrap();

            assert!(matches!(outcome, ResidentOutputOutcome::Unresolved));
            // Acquisition retired its socket, so output must reconnect before
            // observing the unchanged source on the same owned peer.
            if let Some(bytes) = original_journal {
                assert_eq!(&*events.borrow(), &["reconnect", "observe"]);
                assert_eq!(backend.conversation, None);
                assert_eq!(std::fs::read(journal).unwrap(), bytes);
            } else {
                assert_eq!(&*events.borrow(), &["reconnect", "observe", "capture"]);
                assert_eq!(backend.conversation, Some(next));
                assert!(!journal.exists());
            }
            assert_eq!(conversation_state(), original_state);

            drop(workflow);
            drop(ledger);
            drop(store);
            std::fs::remove_dir_all(base).unwrap();
        }
    }

    #[cfg(feature = "installed-resident-output")]
    #[test]
    fn reader_checks_original_insert_loss_before_paying_for_a_different_notebook() {
        use crate::conversation::AttemptReason;
        use remarkable_open_sdk::{native_adapter::Rejection, PageKey};

        for (mode, prior_reply, blocked) in [
            (
                "reconnect-source-handoff-indeterminate",
                Reply::Indeterminate,
                true,
            ),
            (
                "reconnect-source-handoff-indeterminate-last-page",
                Reply::Indeterminate,
                true,
            ),
            (
                "reconnect-source-handoff-rejected",
                Reply::Rejected(Rejection::Unsupported),
                false,
            ),
            (
                "reconnect-source-handoff-indeterminate-successor",
                Reply::Indeterminate,
                false,
            ),
        ] {
            let mut releases = vec![released(Instant::now())];
            if blocked {
                releases.push(ReleasedReaderInput {
                    gesture: ResidentGesture::BlankStart,
                    ..released(Instant::now())
                });
            }
            let (mut workflow, store, base, events, _notify) =
                workflow(mode, releases, Duration::ZERO);
            let fixture = Fixture::new();
            let mut runtime = fixture
                .start_with_store(store.clone(), mode, Duration::from_secs(5))
                .unwrap();
            let source = PageKey {
                document: operation().0,
                page: operation().0,
            };
            let original = operation();
            let prior = ConversationId::new_v4();
            let insertion = InsertAfter {
                source: ReaderFacts::new(source, vec![source.page]).unwrap(),
                target: operation().0,
            };
            assert_eq!(
                runtime
                    .connection()
                    .unwrap()
                    .insert_after(
                        prior,
                        original,
                        insertion,
                        Instant::now() + Duration::from_secs(3)
                    )
                    .unwrap(),
                prior_reply
            );
            runtime.retire_connection();
            let journal = store.paths.data.join("native-insertion-attempts-v1.json");
            let bytes = std::fs::read(&journal).unwrap();
            let history = resident_client::attempts(&store).unwrap();
            assert_eq!(history.len(), 1);
            assert_eq!(history[0].conversation, prior);
            assert_eq!(history[0].request.frame().operation, original);
            assert_eq!(history[0].reply.as_ref(), Some(&prior_reply));
            let owner = workflow.resident_source.as_mut().unwrap();
            owner.selection = InstalledReaderSelection::fixture_with_installed_output();
            owner.session = Some(Box::new(OwnedSession {
                runtime,
                _fixture: fixture,
                events: events.clone(),
                delivery_fault: false,
                #[cfg(feature = "installed-resident-output")]
                writer_slot: None,
            }));
            owner.reconnect = true;
            let calls = Rc::new(Cell::new(0));
            let model = Provider {
                calls: calls.clone(),
                batches: Rc::new(RefCell::new(vec![])),
                images: vec![],
                payload_delay: Duration::ZERO,
                kill_pid: None,
                selection_subject: None,
            };
            let ledger = Ledger::new(store.clone());
            let mut reader = Orchestrator::new(workflow, model, Ledger::new(store.clone()));
            let result = reader.run_iteration();
            if blocked {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains(&original.0.to_string()));
                assert_eq!(calls.get(), 0);
            } else {
                result.unwrap();
                assert_eq!(calls.get(), 1);
            }
            assert_eq!(&*events.borrow(), &["reconnect", "observe", "capture"]);
            assert_eq!(std::fs::read(&journal).unwrap(), bytes);
            let entries = store
                .snapshot_heads_matching(&[Namespace::Source], 2, |_| true)
                .unwrap();
            assert_eq!(entries.len(), 1);
            let Record::ResidentCapture(capture) =
                serde_json::from_value(entries[0].payload.clone()).unwrap()
            else {
                panic!("expected the second notebook capture");
            };
            assert_ne!(
                capture.before.document.as_bytes(),
                source.document.as_bytes()
            );
            assert_ne!(capture.conversation, prior);
            let turns = ledger.inspect(capture.conversation, false).unwrap();
            let reason = if blocked {
                AttemptReason::NativeOutputUnavailable
            } else {
                AttemptReason::NoSelection
            };
            assert!(turns.iter().any(|r| matches!(r, Record::OutcomeFact(f) if f.outcome == Outcome::Failed && f.reason == reason)));
            assert!(!turns.iter().any(
                |r| matches!(r, Record::Turn(t) if t.role == crate::conversation::Role::Assistant)
            ));
            if blocked {
                let saved_heads = || {
                    store
                        .snapshot_heads_matching(
                            &[Namespace::Conversation, Namespace::Source],
                            crate::storage::MAX_ITEMS,
                            |_| true,
                        )
                        .unwrap()
                };
                let before = saved_heads();
                let error = reader.run_iteration().unwrap_err();
                assert!(error.to_string().contains(&original.0.to_string()));
                assert!(
                    saved_heads() == before,
                    "blocked blank start changed Buddy history"
                );
                assert_eq!(calls.get(), 0);
                assert_eq!(std::fs::read(&journal).unwrap(), bytes);
                assert_eq!(
                    &*events.borrow(),
                    &[
                        "reconnect",
                        "observe",
                        "capture",
                        "reconnect",
                        "observe",
                        "capture"
                    ]
                );
            }
            drop(reader);
            drop(ledger);
            drop(store);
            std::fs::remove_dir_all(base).unwrap();
        }
    }

    #[test]
    fn resident_selection_refusal_leaves_no_native_insertion_attempt() {
        use crate::conversation::AttemptReason;

        let (workflow, store, base, events, _notify) = workflow(
            "reconnect-source-handoff",
            vec![released(Instant::now())],
            Duration::ZERO,
        );
        let calls = Rc::new(Cell::new(0));
        let model = Provider {
            calls: calls.clone(),
            batches: Rc::new(RefCell::new(vec![])),
            images: vec![],
            payload_delay: Duration::ZERO,
            kill_pid: None,
            selection_subject: None,
        };
        let mut reader = Orchestrator::new(workflow, model, Ledger::new(store.clone()));

        reader.run_iteration().unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(
            &*events.borrow(),
            &["activate", "spawn", "observe", "capture"]
        );
        assert!(super::super::resident_client::attempts(&store)
            .unwrap()
            .is_empty());

        let stored = store
            .snapshot_heads_matching(&[Namespace::Source], 10, |_| true)
            .unwrap();
        let [entry] = stored.as_slice() else {
            panic!("expected one resident source capture");
        };
        let Record::ResidentCapture(evidence) =
            serde_json::from_value::<Record>(entry.payload.clone()).unwrap()
        else {
            panic!("wrong historical origin");
        };
        let history = Ledger::new(store.clone())
            .inspect(evidence.conversation, false)
            .unwrap();
        let outcomes = history
            .iter()
            .filter_map(|record| match record {
                Record::OutcomeFact(fact) => Some((&fact.outcome, &fact.reason)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(outcomes, [(&Outcome::Failed, &AttemptReason::NoSelection)]);
        assert!(!history.iter().any(|record| matches!(
            record,
            Record::Binding(_)
                | Record::Turn(crate::conversation::Turn {
                    role: crate::conversation::Role::Assistant,
                    ..
                })
        )));

        drop(reader);
        drop(store);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn selected_acquisition_without_release_or_after_original_expiry_never_dispatches() {
        for fault in ["no-release", "expired", "activation-cost", "corrupt"] {
            let old = if fault == "expired" {
                Instant::now() - ACQUISITION - Duration::from_secs(1)
            } else if fault == "activation-cost" {
                Instant::now() - ACQUISITION + Duration::from_millis(20)
            } else {
                Instant::now()
            };
            let releases = if fault == "no-release" {
                vec![]
            } else {
                vec![released(old)]
            };
            let (mut workflow, store, base, events, _notify) = workflow(
                if fault == "corrupt" {
                    "reconnect-source-handoff-corrupt"
                } else {
                    "reconnect-source-handoff"
                },
                releases,
                if fault == "activation-cost" {
                    Duration::from_secs(2)
                } else {
                    Duration::ZERO
                },
            );
            if fault == "activation-cost" {
                // Start the short remaining budget after fixture construction.
                workflow.device = Box::new(
                    super::super::selected_backend::RefusingBackend::with_resident_releases(vec![
                        released(Instant::now() - ACQUISITION + Duration::from_secs(1)),
                    ]),
                );
            }
            let result = workflow
                .wait_for_reader()
                .and_then(|()| workflow.acquire_reader_evidence());
            assert!(result.is_err(), "{fault}");
            let expected: &[&str] = match fault {
                "activation-cost" => &["activate"],
                "corrupt" => &["activate", "spawn", "observe", "capture"],
                _ => &[],
            };
            assert_eq!(events.borrow().as_slice(), expected, "{fault}");
            assert!(store
                .snapshot_heads_matching(&[Namespace::Source], 10, |_| true)
                .unwrap()
                .is_empty());
            drop(workflow);
            drop(store);
            std::fs::remove_dir_all(base).unwrap();
        }
    }
    #[test]
    fn unapproved_unsupervised_selection_never_reaches_factory_or_legacy_dismissal() {
        let base =
            std::env::temp_dir().join(format!("resident-unapproved-{}", uuid::Uuid::new_v4()));
        let store = Arc::new(
            Store::open(StorePaths {
                data: base.join("data"),
                cache: base.join("cache"),
                credentials: base.join("credentials"),
            })
            .unwrap(),
        );
        let backend = super::super::selected_backend::RefusingBackend::new();
        let mut workflow = Workflow::with_device(Box::new(backend), false);
        assert!(workflow
            .configure_resident_source(InstalledReaderSelection::fixture(), store.clone())
            .is_err());
        assert!(workflow.resident_source.is_none());
        assert!(workflow.acquire_reader_evidence().is_err());
        drop(workflow);
        drop(store);
        std::fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn selected_service_finalization_allows_new_release_after_clean_cancellation() {
        use crate::device::interaction::Interaction;
        let mut polls = (0..5).map(|_| Ok(vec![])).collect::<Vec<_>>();
        polls.push(Ok(vec![Interaction::Invalidated]));
        polls.extend((0..4096).map(|_| Ok(vec![])));
        let first = ReleasedReaderInput {
            released_at: Instant::now(),
            gesture: ResidentGesture::Reader,
            input: RequestInput::scripted(polls),
        };
        let (mut workflow, store, base, events, _notify) = workflow(
            "reconnect-source-handoff",
            vec![first, released(Instant::now())],
            Duration::ZERO,
        );
        workflow.wait_for_reader().unwrap();
        let pid = workflow
            .resident_source
            .as_ref()
            .unwrap()
            .session
            .as_ref()
            .unwrap()
            .identity()
            .pid;
        assert!(workflow.acquire_reader_evidence().is_err());
        assert!(workflow.input_failed());
        assert!(!workflow.runtime_failed());
        crate::workflow::selected_resident_reader::finish_selected_request(&mut workflow).unwrap();
        workflow.wait_for_reader().unwrap();
        let acquisition = workflow.acquire_reader_evidence().unwrap();
        let ledger = Ledger::new(store.clone());
        let attempt =
            crate::workflow::reader_attempt::Attempt::prepare(&ledger, acquisition).unwrap();
        let images = attempt.images(&ledger).unwrap();
        assert_eq!(images.len(), 4);
        workflow.deliver_reader_acquisition().unwrap();
        crate::workflow::selected_resident_reader::finish_selected_request(&mut workflow).unwrap();
        assert_eq!(
            workflow
                .resident_source
                .as_ref()
                .unwrap()
                .session
                .as_ref()
                .unwrap()
                .identity()
                .pid,
            pid
        );
        assert_eq!(
            &*events.borrow(),
            &[
                "activate",
                "spawn",
                "observe",
                "reconnect",
                "observe",
                "capture"
            ]
        );
        assert!(crate::workflow::resident_client::attempts(&store)
            .unwrap()
            .is_empty());
        drop(attempt);
        drop(ledger);
        drop(workflow);
        drop(store);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn selected_service_finalization_stops_when_input_is_lost_during_cleanup() {
        let (mut workflow, store, base, _, _notify) = workflow(
            "reconnect-source-handoff",
            vec![released(Instant::now())],
            Duration::ZERO,
        );
        workflow.wait_for_reader().unwrap();
        assert!(!workflow.runtime_failed());
        workflow.resident_source.as_mut().unwrap().input = Some(RequestInput::scripted(vec![Err(
            anyhow::anyhow!("fixture input lost"),
        )]));
        let error =
            crate::workflow::selected_resident_reader::finish_selected_request(&mut workflow)
                .unwrap_err();
        assert!(!error.is::<FinalizedCancellation>());
        assert!(workflow.runtime_failed());
        assert!(workflow.resident_source.as_ref().unwrap().input.is_none());
        drop((workflow, store));
        std::fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn original_acquisition_budget_includes_stored_readback_encoding_and_model_payload() {
        let (workflow, store, base, events, _notify) = workflow(
            "reconnect-source-handoff",
            vec![released(
                Instant::now() - ACQUISITION + Duration::from_secs(5),
            )],
            Duration::ZERO,
        );
        let calls = Rc::new(Cell::new(0));
        let batches = Rc::new(RefCell::new(vec![]));
        let model = Provider {
            calls: calls.clone(),
            batches,
            images: vec![],
            payload_delay: Duration::from_millis(1500),
            kill_pid: None,
            selection_subject: Some("24 / 6"),
        };
        let mut reader = Orchestrator::new(workflow, model, Ledger::new(store.clone()));
        let error = reader.run_iteration().unwrap_err();
        assert!(format!("{error:#}").contains("acquisition expired"));
        assert_eq!(calls.get(), 0, "late provider dispatch");
        assert_eq!(
            &*events.borrow(),
            &["activate", "spawn", "observe", "capture"]
        );
        assert_eq!(
            store
                .snapshot_heads_matching(&[Namespace::Source], 10, |_| true)
                .unwrap()
                .len(),
            1,
            "late acknowledgment preserves retained historical evidence"
        );
        drop(reader);
        drop(store);
        std::fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn closing_input_health_poll_cannot_discharge_an_expired_acquisition_deadline() {
        let deadline = Instant::now() + Duration::from_millis(20);
        let mut provider = false;
        let result = admitted_progress(deadline, || {
            std::thread::sleep(
                deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(2),
            );
            Ok(())
        })
        .map(|()| provider = true);
        assert!(result.is_err());
        assert!(!provider);
    }
    fn kill_owned(pid: u32) {
        assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGKILL) }, 0);
        let end = Instant::now() + Duration::from_secs(1);
        while Instant::now() < end {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
            if stat.is_err()
                || stat
                    .unwrap()
                    .split(") ")
                    .nth(1)
                    .is_some_and(|s| s.starts_with('Z'))
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("owned test child did not terminate");
    }
    #[test]
    #[ignore = "owned companion contract requires root; run explicitly in an owned Linux container"]
    fn retained_reader_or_companion_death_poison_actual_provider_and_idle_owner() {
        for companion in [false, true] {
            for during_provider in [false, true] {
                let (mut workflow, store, base, events, _notify) = workflow(
                    if companion {
                        "source-handoff-companion"
                    } else {
                        "reconnect-source-handoff"
                    },
                    vec![released(Instant::now()), released(Instant::now())],
                    Duration::ZERO,
                );
                workflow.wait_for_reader().unwrap();
                let failed = workflow.service_health.as_ref().unwrap().failure_witness();
                let session = workflow
                    .resident_source
                    .as_ref()
                    .unwrap()
                    .session
                    .as_ref()
                    .unwrap();
                let pid = if companion {
                    session.companion_pid().expect("actual owned companion")
                } else {
                    session.identity().pid
                };
                if during_provider {
                    let calls = Rc::new(Cell::new(0));
                    let model = Provider {
                        calls: calls.clone(),
                        batches: Rc::new(RefCell::new(vec![])),
                        images: vec![],
                        payload_delay: Duration::ZERO,
                        kill_pid: Some(pid),
                        selection_subject: Some("24 / 6"),
                    };
                    let mut reader = Orchestrator::new(workflow, model, Ledger::new(store.clone()));
                    reader.set_trigger_enabled(false); // The real release was already consumed above.
                    assert!(reader.run_iteration().is_err());
                    assert_eq!(calls.get(), 1, "no verification/delivery after owned death");
                    assert!(failed.load(std::sync::atomic::Ordering::Acquire));
                    drop(reader);
                } else {
                    workflow.finish_request().unwrap();
                    kill_owned(pid);
                    assert!(
                        workflow.wait_for_reader().is_err(),
                        "actual idle callback must check retained child"
                    );
                    assert!(workflow.runtime_failed());
                    assert!(failed.load(std::sync::atomic::Ordering::Acquire));
                    assert_eq!(
                        events.borrow().iter().filter(|e| **e == "activate").count(),
                        1
                    );
                    drop(workflow);
                }
                drop(store);
                std::fs::remove_dir_all(base).unwrap();
            }
        }
    }
    #[test]
    fn full_delivery_boundary_policy_loss_refuses_actual_first_provider() {
        let (workflow, store, base, _events, _notify) = workflow(
            "reconnect-source-handoff-delivery-policy",
            vec![released(Instant::now())],
            Duration::ZERO,
        );
        let failed = workflow.service_health.as_ref().unwrap().failure_witness();
        let calls = Rc::new(Cell::new(0));
        let model = Provider {
            calls: calls.clone(),
            batches: Rc::new(RefCell::new(vec![])),
            images: vec![],
            payload_delay: Duration::ZERO,
            kill_pid: None,
            selection_subject: Some("24 / 6"),
        };
        let mut reader = Orchestrator::new(workflow, model, Ledger::new(store.clone()));
        assert!(reader.run_iteration().is_err());
        assert_eq!(calls.get(), 0);
        assert!(failed.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(
            store
                .snapshot_heads_matching(&[Namespace::Source], 10, |_| true)
                .unwrap()
                .len(),
            1
        );
        drop(reader);
        drop(store);
        std::fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn caught_selected_idle_observer_loss_cannot_recreate_input_around_retained_session() {
        let (mut workflow, store, base, events, _notify) = workflow(
            "reconnect-source-handoff",
            vec![released(Instant::now())],
            Duration::ZERO,
        );
        workflow.wait_for_reader().unwrap();
        workflow.finish_request().unwrap();
        let failed = workflow.service_health.as_ref().unwrap().failure_witness();
        workflow.device = Box::new(super::super::selected_backend::RefusingBackend::new());
        assert!(workflow.wait_for_reader().is_err());
        assert!(workflow.runtime_failed());
        assert!(failed.load(std::sync::atomic::Ordering::Acquire));
        workflow.device = Box::new(
            super::super::selected_backend::RefusingBackend::with_resident_releases(vec![
                released(Instant::now()),
            ]),
        );
        assert!(
            workflow.wait_for_reader().is_err(),
            "catching failure cannot renew idle input/session"
        );
        assert!(workflow.acquire_reader_evidence().is_err());
        assert_eq!(events.borrow().as_slice(), ["activate", "spawn"]);
        drop(workflow);
        drop(store);
        std::fs::remove_dir_all(base).unwrap();
    }
}

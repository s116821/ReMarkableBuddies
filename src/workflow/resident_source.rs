//! Real selected Workflow source route. Historical pixels confer no output lease.
#[cfg(test)]
use super::resident_runtime::ReadOnlyAdmission;
use super::{
    resident_request::{FailureKind, RequestFailure},
    resident_runtime::ReadOnlyRequest as Request,
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
    native_adapter::{Reply, RuntimeIdentity},
    OperationId, Uuid,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) const ACQUISITION: Duration = Duration::from_secs(90);
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
    fn identity(&self) -> RuntimeIdentity;
    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    fn take_writer_request(
        &self,
        _document: crate::storage::Uuid,
        _page: crate::storage::Uuid,
    ) -> Result<Option<crate::writer::Request>> {
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
    fn identity(&self) -> RuntimeIdentity {
        self.source_identity()
    }
    #[cfg(all(
        feature = "development-resident-bootstrap",
        feature = "development-input-diagnostics"
    ))]
    fn take_writer_request(
        &self,
        document: crate::storage::Uuid,
        page: crate::storage::Uuid,
    ) -> Result<Option<crate::writer::Request>> {
        Session::take_writer_request(self, document, page)
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
    ) -> Result<()> {
        ensure!(
            !self.fatal && self.session.is_none() && self.input.is_none(),
            "resident startup activation requires an unused owner"
        );
        // Startup observes quiet input without creating a released Reader request.
        let mut input = RequestInput::new(corner, None)?;
        input.require_quiet()?;
        let mut session = self.factory.activate(
            &self.selection,
            self.store.clone(),
            health,
            &mut input,
            Instant::now() + ACQUISITION,
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
        let session = self
            .session
            .as_ref()
            .context("resident Writer session absent")?;
        Ok(session
            .take_writer_request(capture.document(), capture.page())?
            .map(
                |request| super::resident_output::SelectedConversationRequest::WriterRefinement {
                    request,
                    require_current_draft: true,
                },
            ))
    }
    pub(super) fn begin(
        &mut self,
        released: ReleasedReaderInput,
        health: &mut ServiceHealth,
    ) -> Result<()> {
        ensure!(
            !self.fatal && self.input.is_none(),
            "Resident source request cannot restart"
        );
        self.deadline = Some(released.released_at + ACQUISITION);
        self.input = Some(released.input);
        self.gesture = Some(released.gesture);
        self.canceled = false;
        self.captured_source = None;
        self.progress(health)?;
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
        self.acquisition_current(health)?;
        let deadline = self.deadline.unwrap();
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
                if let Some(directory) = &self.development_diagnostics {
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
        before(deadline)?;
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
            Ok(value) => Ok(value),
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
        result
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
        text: &str,
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

        admitted_progress(end, || self.progress(health))?;
        if self.reconnect {
            let reconnect_end = end.min(Instant::now() + Duration::from_secs(30));
            let result = self.session.as_mut().unwrap().reconnect(
                health,
                self.input
                    .as_mut()
                    .context("resident output guard absent")?,
                reconnect_end,
            );
            if let Err(error) = result {
                return self.failure(health, error);
            }
            self.reconnect = false;
        }
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
        renderer.render(text, &mut lease)?;
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
mod tests {
    use super::*;
    use crate::workflow::resident_runtime::{tests::Fixture, ResidentRuntime};
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
        fn identity(&self) -> RuntimeIdentity {
            self.runtime.identity().clone()
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
        fn finish(&mut self) {
            self.runtime.retire_connection();
        }
    }
    struct OwnedFactory {
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
            events: events.clone(),
            mode,
            delay,
        });
        (workflow, store, base, events, receiver)
    }
    struct Provider {
        calls: Rc<Cell<usize>>,
        batches: Rc<RefCell<Vec<Vec<String>>>>,
        images: Vec<String>,
        payload_delay: Duration,
        kill_pid: Option<u32>,
        no_selection: bool,
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
            if self.no_selection {
                return Ok("NONE".into());
            }
            Ok(if index.is_multiple_of(2) { "QUESTION: why?\nQUESTION_BOX: 130,35,200,50\nSELECTION_CENTER: 385,230\n---\nANSWER: Because the model fixture says so." }
                else { "TRANSCRIPTION: why?" }.into())
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
            no_selection: false,
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
            Ok(PageUse::Unresolved)
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
            panic!("unexpected renderer request");
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
                        runtime: owner.session.as_ref().unwrap().identity(),
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
                    "answer for the new conversation",
                    None,
                    &mut || panic!("unexpected effect callback"),
                )
                .unwrap();

            assert!(matches!(outcome, ResidentOutputOutcome::Unresolved));
            if let Some(bytes) = original_journal {
                assert_eq!(&*events.borrow(), &["observe"]);
                assert_eq!(backend.conversation, None);
                assert_eq!(std::fs::read(journal).unwrap(), bytes);
            } else {
                assert_eq!(&*events.borrow(), &["observe", "capture"]);
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
            no_selection: true,
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
    fn selected_workflow_canceled_request_then_new_release_reconnects_same_child_without_reactivation(
    ) {
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
        assert!(workflow.finish_request().is_err());
        workflow.wait_for_reader().unwrap();
        let acquisition = workflow.acquire_reader_evidence().unwrap();
        let ledger = Ledger::new(store.clone());
        let attempt =
            crate::workflow::reader_attempt::Attempt::prepare(&ledger, acquisition).unwrap();
        let images = attempt.images(&ledger).unwrap();
        assert_eq!(images.len(), 4);
        workflow.deliver_reader_acquisition().unwrap();
        workflow.finish_request().unwrap();
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
            no_selection: false,
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
                        no_selection: false,
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
            no_selection: false,
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

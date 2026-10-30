//! Durable resident insertion/selection attempts, distinct from qualified page bindings.
//! Native replies remain historical. No runtime activation or Reader write path
//! is installed by this module; Supervisor and acquisition qualification remain
//! separate. Stored attempts never refresh synthetic Platform observations.
use crate::storage::{files, Store, Uuid};
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use remarkable_open_sdk::native_adapter::{
    Command, Frame, HistoricalFrame, Message, Reply, MAX_FRAME,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

const JOURNAL_FILE: &str = "native-insertion-attempts-v1.json";
const MAX_JOURNAL: usize = 8 * 1024 * 1024;
const MAX_ATTEMPTS: usize = 1024;

#[cfg(test)]
#[derive(Clone, Copy, PartialEq)]
enum PublicationPoint {
    Reservation,
    Outcome,
    AfterOutcome,
}
#[cfg(test)]
thread_local! {
    static PUBLICATION_FAULT: std::cell::Cell<Option<PublicationPoint>> = const { std::cell::Cell::new(None) };
    static AFTER_OUTCOME: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}
#[cfg(all(test, target_os = "linux"))]
pub(super) fn after_next_outcome(action: impl FnOnce() + 'static) {
    AFTER_OUTCOME.with(|slot| {
        assert!(slot.borrow().is_none());
        *slot.borrow_mut() = Some(Box::new(action));
    });
}
#[cfg(test)]
fn publication_fault(point: PublicationPoint) -> Result<()> {
    PUBLICATION_FAULT.with(|fault| {
        if fault.get() == Some(point) {
            fault.set(None);
            bail!("owned journal publication fault");
        }
        Ok(())
    })
}

#[cfg(all(test, target_os = "linux"))]
pub(super) fn fail_next_outcome_publication(after: bool) {
    PUBLICATION_FAULT.with(|fault| {
        fault.set(Some(if after {
            PublicationPoint::AfterOutcome
        } else {
            PublicationPoint::Outcome
        }))
    });
}

#[derive(Clone, Debug)]
pub struct HistoricalAttempt {
    pub conversation: Uuid,
    /// Original wire revision/bytes/digest; never re-encode to validate history.
    pub request: HistoricalFrame,
    /// A local client proof, never reconstructed from lack of a native reply.
    pub known_not_dispatched: bool,
    pub reply: Option<Reply>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
enum Outcome {
    Reserved {},
    NotDispatched {},
    HistoricalReply { frame: String },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attempt {
    conversation: Uuid,
    frame: String,
    sha256: String,
    outcome: Outcome,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    format: u32,
    actor: Uuid,
    attempts: Vec<Attempt>,
}
fn decode(encoded: &str) -> Result<HistoricalFrame> {
    ensure!(
        encoded.len() <= (MAX_FRAME + 4).div_ceil(3) * 4,
        "native frame exceeds bound"
    );
    let bytes = STANDARD
        .decode(encoded)
        .context("invalid native frame encoding")?;
    ensure!(
        bytes.len() >= 4 && bytes.len() <= MAX_FRAME + 4,
        "invalid native frame length"
    );
    ensure!(
        u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize == bytes.len() - 4,
        "native frame prefix mismatch"
    );
    let frame =
        HistoricalFrame::decode(&bytes).map_err(|_| anyhow::anyhow!("invalid native frame"))?;
    ensure!(
        STANDARD.encode(frame.encoded()) == encoded,
        "noncanonical native frame"
    );
    Ok(frame)
}
#[cfg(any(target_os = "linux", test))]
fn encode(frame: &Frame) -> Result<String> {
    Ok(STANDARD.encode(
        frame
            .encode()
            .map_err(|_| anyhow::anyhow!("invalid native frame"))?,
    ))
}
#[cfg(any(target_os = "linux", test))]
fn digest(frame: &Frame) -> Result<String> {
    Ok(crate::storage::digest(
        &frame
            .encode()
            .map_err(|_| anyhow::anyhow!("invalid native frame"))?,
    ))
}
fn validate_reply(original: &HistoricalFrame, response: &HistoricalFrame) -> Result<Reply> {
    ensure!(
        original.wire_revision() == response.wire_revision(),
        "native reply revision mismatch"
    );
    let request = original.frame();
    let reply = response.frame();
    ensure!(
        request.scope == reply.scope
            && request.sequence == reply.sequence
            && request.operation == reply.operation,
        "native reply scope mismatch"
    );
    let Message::Response {
        request_sha256,
        reply,
    } = &reply.message
    else {
        bail!("expected native response")
    };
    ensure!(
        original.fingerprint() == *request_sha256,
        "native reply digest mismatch"
    );
    let Message::Request(command @ (Command::InsertAfter(_) | Command::SelectAdjacent(_))) =
        &request.message
    else {
        bail!("expected native effect")
    };
    match (command, reply) {
        (_, Reply::Rejected(_) | Reply::Indeterminate) => {}
        (
            Command::InsertAfter(insertion),
            Reply::InsertionObserved {
                target,
                after_order,
            },
        ) => {
            ensure!(
                target.document == insertion.source.page().document
                    && target.page == insertion.target
                    && *after_order == insertion.expected_after_order(),
                "native insertion reply mismatch"
            );
        }
        (Command::SelectAdjacent(selection), Reply::TargetObserved(actual)) => {
            ensure!(
                actual.page().document == selection.expected.page().document
                    && actual.page().page == selection.target
                    && actual.order() == selection.expected.order(),
                "native selection reply mismatch"
            );
        }
        _ => bail!("wrong native effect response"),
    }
    Ok(reply.clone())
}
impl Attempt {
    fn historical(&self) -> Result<HistoricalAttempt> {
        ensure!(!self.conversation.is_nil(), "nil native conversation");
        let request = decode(&self.frame)?;
        ensure!(
            matches!(
                request.frame().message,
                Message::Request(Command::InsertAfter(_) | Command::SelectAdjacent(_))
            ),
            "journal contains non-effect"
        );
        ensure!(
            crate::storage::digest(request.encoded()) == self.sha256,
            "native attempt digest mismatch"
        );
        let reply = match &self.outcome {
            Outcome::HistoricalReply { frame } => Some(validate_reply(&request, &decode(frame)?)?),
            _ => None,
        };
        Ok(HistoricalAttempt {
            conversation: self.conversation,
            request,
            known_not_dispatched: matches!(self.outcome, Outcome::NotDispatched {}),
            reply,
        })
    }
}
impl HistoricalAttempt {
    fn known_no_effect(&self) -> bool {
        self.known_not_dispatched || matches!(self.reply, Some(Reply::Rejected(_)))
    }
    fn permits_followup(&self, next: &Frame) -> bool {
        if self.known_no_effect() {
            return true;
        }
        let target = match &self.reply {
            Some(Reply::InsertionObserved { target, .. }) => *target,
            Some(Reply::TargetObserved(facts)) => facts.page(),
            _ => return false,
        };
        matches!(&next.message,
            Message::Request(Command::SelectAdjacent(selection))
            if selection.expected.page().document == target.document && selection.target == target.page)
    }
}
impl State {
    fn validate(&self, actor: Uuid) -> Result<Vec<HistoricalAttempt>> {
        ensure!(
            matches!(self.format, 1 | 2) && self.actor == actor && !actor.is_nil(),
            "native journal owner/schema mismatch"
        );
        ensure!(
            self.attempts.len() <= MAX_ATTEMPTS,
            "native journal capacity exceeded"
        );
        let mut operations = std::collections::HashSet::new();
        let mut reserved = std::collections::HashMap::<Uuid, usize>::new();
        let mut history: Vec<HistoricalAttempt> = Vec::new();
        for attempt in &self.attempts {
            let value = attempt.historical()?;
            ensure!(
                self.format == 2
                    || matches!(
                        value.request.frame().message,
                        Message::Request(Command::InsertAfter(_))
                    ),
                "selection requires native journal format 2"
            );
            ensure!(
                operations.insert(value.request.frame().operation),
                "duplicate native operation"
            );
            if let Some(index) = reserved.get(&value.conversation) {
                ensure!(
                    history[*index].permits_followup(value.request.frame()),
                    "native attempt violates prior conversation reservation"
                );
            }
            if !value.known_no_effect() {
                reserved.insert(value.conversation, history.len());
            }
            history.push(value);
        }
        Ok(history)
    }
}
fn load(path: &Path, actor: Uuid) -> Result<State> {
    files::safe_path(path)?;
    let state = match std::fs::symlink_metadata(path) {
        Ok(_) => files::json(path, MAX_JOURNAL as u64)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => State {
            format: 2,
            actor,
            attempts: Vec::new(),
        },
        Err(error) => return Err(error.into()),
    };
    state.validate(actor)?;
    Ok(state)
}
#[cfg(any(target_os = "linux", test))]
fn persist(path: &Path, state: &State) -> Result<()> {
    state.validate(state.actor)?;
    let bytes = serde_json::to_vec(state)?;
    ensure!(
        bytes.len() <= MAX_JOURNAL,
        "native journal byte capacity exceeded"
    );
    if path.try_exists()? {
        files::atomic(path, &bytes)
    } else {
        files::create_identity(path, state)
    }
}

/// Read-only historical inspection. Missing evidence grants no effect authority.
pub fn attempts(store: &Store) -> Result<Vec<HistoricalAttempt>> {
    store.with_owned_local_io(|root, actor| load(&root.join(JOURNAL_FILE), actor)?.validate(actor))
}

#[cfg(any(target_os = "linux", test))]
fn reserve(store: &Store, conversation: Uuid, request: &Frame, expected: [u8; 32]) -> Result<()> {
    ensure!(
        request.fingerprint().ok() == Some(expected),
        "journal callback digest mismatch"
    );
    let attempt = Attempt {
        conversation,
        frame: encode(request)?,
        sha256: digest(request)?,
        outcome: Outcome::Reserved {},
    };
    attempt.historical()?;
    store.with_owned_local_io(|root, actor| {
        let path = root.join(JOURNAL_FILE);
        let mut state = load(&path, actor)?;
        ensure!(state.attempts.len() < MAX_ATTEMPTS, "native journal full");
        for prior in state.validate(actor)? {
            ensure!(
                prior.request.frame().operation != request.operation,
                "native operation already consumed"
            );
            ensure!(
                prior.conversation != conversation || prior.permits_followup(request),
                "native conversation requires original-target reconciliation"
            );
        }
        state.format = 2;
        state.attempts.push(attempt);
        #[cfg(test)]
        publication_fault(PublicationPoint::Reservation)?;
        persist(&path, &state)
    })
}
#[cfg(any(target_os = "linux", test))]
fn finish(store: &Store, request: &Frame, outcome: Outcome) -> Result<()> {
    store.with_owned_local_io(|root, actor| {
        let path = root.join(JOURNAL_FILE);
        let mut state = load(&path, actor)?;
        let original = encode(request)?;
        let attempt = state
            .attempts
            .iter_mut()
            .find(|attempt| attempt.frame == original)
            .context("native reservation missing")?;
        ensure!(
            matches!(attempt.outcome, Outcome::Reserved {}) || attempt.outcome == outcome,
            "native outcome already recorded"
        );
        attempt.outcome = outcome;
        state.format = 2;
        #[cfg(test)]
        publication_fault(PublicationPoint::Outcome)?;
        persist(&path, &state)?;
        #[cfg(test)]
        if let Some(action) = AFTER_OUTCOME.with(|slot| slot.borrow_mut().take()) {
            action();
        }
        #[cfg(test)]
        publication_fault(PublicationPoint::AfterOutcome)?;
        Ok(())
    })
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use remarkable_open_sdk::{
        native_adapter::{
            transport::{Client, Failure, TransportError},
            ConnectionIdentity, InsertAfter, LostRequest, ReaderFacts, SelectAdjacent,
        },
        OperationId,
    };
    use std::{fs::File, sync::Arc, time::Instant};

    #[derive(Debug)]
    struct ProgressFailure(anyhow::Error);
    impl std::fmt::Display for ProgressFailure {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{:#}", self.0)
        }
    }
    impl std::error::Error for ProgressFailure {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(self.0.as_ref())
        }
    }
    // Boxing anyhow directly can erase its root marker from standard Error
    // traversal. Keep an explicit source link for the Session classifier.
    pub(in crate::workflow) fn progress_error(error: anyhow::Error) -> std::io::Error {
        std::io::Error::other(ProgressFailure(error))
    }

    /// Structured transport evidence survives anyhow context at the product boundary.
    #[derive(Debug)]
    pub struct ConnectionFailure {
        pub cause: Failure,
        pub lost: LostRequest,
    }
    impl From<TransportError> for ConnectionFailure {
        fn from(error: TransportError) -> Self {
            Self {
                cause: error.cause,
                lost: error.lost,
            }
        }
    }
    impl std::fmt::Display for ConnectionFailure {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "resident transport failed: {:?}; {:?}",
                self.cause, self.lost
            )
        }
    }
    impl std::error::Error for ConnectionFailure {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            failure_source(&self.cause)
        }
    }
    fn failure_source(cause: &Failure) -> Option<&(dyn std::error::Error + 'static)> {
        match cause {
            Failure::Io(error) | Failure::Journal(error) | Failure::Progress(error) => {
                // Keep the callback's root marker even when it has no source.
                Some(match error.get_ref() {
                    Some(inner) => inner,
                    None => error,
                })
            }
            _ => None,
        }
    }

    #[derive(Debug)]
    pub enum EffectCause {
        Transport(Failure),
        Retention,
        Unresolved,
    }
    #[derive(Debug)]
    pub struct EffectFailure {
        pub operation: OperationId,
        pub lost: LostRequest,
        pub detail: String,
        pub cause: EffectCause,
    }
    pub type InsertionFailure = EffectFailure;
    impl std::fmt::Display for EffectFailure {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.detail)
        }
    }
    impl std::error::Error for EffectFailure {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            match &self.cause {
                EffectCause::Transport(cause) => failure_source(cause),
                _ => None,
            }
        }
    }
    pub struct ResidentConnection {
        client: Client,
        store: Arc<Store>,
        // A reply-retention failure happens after SDK response correlation has
        // retired its pending request. Keep that original uncertainty here too.
        retention_loss: Option<LostRequest>,
    }
    impl ResidentConnection {
        /// Supervisor must independently verify artifacts/runtime and supply a
        /// fresh scope. This does not activate a runtime or qualify its behavior.
        pub fn connect(
            store: Arc<Store>,
            directory: File,
            uid: u32,
            scope: ConnectionIdentity,
            handshake: OperationId,
            deadline: Instant,
        ) -> Result<Self> {
            Self::connect_with_progress(
                store,
                directory,
                uid,
                scope,
                handshake,
                deadline,
                &mut || Ok(()),
            )
        }
        pub fn connect_with_progress(
            store: Arc<Store>,
            directory: File,
            uid: u32,
            scope: ConnectionIdentity,
            handshake: OperationId,
            deadline: Instant,
            progress: &mut dyn FnMut() -> Result<()>,
        ) -> Result<Self> {
            // Refuse malformed prior evidence before contacting the endpoint.
            attempts(&store)?;
            let client = Client::connect_with_progress(
                directory,
                uid,
                scope,
                handshake,
                deadline,
                &mut || progress().map_err(progress_error),
            )
            .map_err(|cause| ConnectionFailure {
                cause,
                lost: LostRequest::NoPendingMutation,
            })?;
            Ok(Self {
                client,
                store,
                retention_loss: None,
            })
        }
        pub fn observe(&mut self, operation: OperationId, deadline: Instant) -> Result<Reply> {
            self.observe_with_progress(operation, deadline, &mut || Ok(()))
        }
        /// Caller-thread input/health checks; returned facts remain historical.
        pub fn observe_with_progress(
            &mut self,
            operation: OperationId,
            deadline: Instant,
            progress: &mut dyn FnMut() -> Result<()>,
        ) -> Result<Reply> {
            self.client
                .execute_with_progress(
                    operation,
                    Command::Observe,
                    deadline,
                    |_, _| Ok(()),
                    &mut || progress().map_err(progress_error),
                )
                .map_err(|error| self.transport_failure(error).into())
        }
        /// Visual-only evidence cannot enter the mutation journal or establish
        /// Reader identity. Earlier unresolved effects remain untouched.
        pub fn capture_visual_with_progress(
            &mut self,
            operation: OperationId,
            deadline: Instant,
            progress: &mut dyn FnMut() -> Result<()>,
        ) -> Result<Reply> {
            self.client
                .execute_with_progress(
                    operation,
                    Command::CaptureVisual,
                    deadline,
                    |_, _| Ok(()),
                    &mut || progress().map_err(progress_error),
                )
                .map_err(|error| self.transport_failure(error).into())
        }
        /// Capture preserves expected source and the caller's original admission.
        /// The SDK validates the bounded image and both source samples. This
        /// read-only request neither journals nor reconciles an earlier effect.
        pub fn capture_source_with_progress(
            &mut self,
            operation: OperationId,
            expected: ReaderFacts,
            deadline: Instant,
            progress: &mut dyn FnMut() -> Result<()>,
        ) -> Result<Reply> {
            self.client
                .execute_with_progress(
                    operation,
                    Command::CaptureSource(expected),
                    deadline,
                    |_, _| Ok(()),
                    &mut || progress().map_err(progress_error),
                )
                .map_err(|error| self.transport_failure(error).into())
        }
        /// Requires separately qualified product reuse/placement decisions. This
        /// primitive only journals and dispatches; it grants no rendering lease.
        pub fn insert_after(
            &mut self,
            conversation: Uuid,
            operation: OperationId,
            insertion: InsertAfter,
            deadline: Instant,
        ) -> std::result::Result<Reply, InsertionFailure> {
            self.insert_after_with_progress(
                conversation,
                operation,
                insertion,
                deadline,
                &mut || Ok(()),
            )
        }
        pub fn insert_after_with_progress(
            &mut self,
            conversation: Uuid,
            operation: OperationId,
            insertion: InsertAfter,
            deadline: Instant,
            progress: &mut dyn FnMut() -> Result<()>,
        ) -> std::result::Result<Reply, InsertionFailure> {
            self.execute_effect(
                conversation,
                operation,
                Command::InsertAfter(insertion),
                deadline,
                progress,
            )
        }
        /// Selection needs a qualified current/adjacent reuse decision. A stored
        /// success is historical evidence, never a fresh capture or write lease.
        pub fn select_adjacent(
            &mut self,
            conversation: Uuid,
            operation: OperationId,
            selection: SelectAdjacent,
            deadline: Instant,
        ) -> std::result::Result<Reply, EffectFailure> {
            self.select_adjacent_with_progress(
                conversation,
                operation,
                selection,
                deadline,
                &mut || Ok(()),
            )
        }
        pub fn select_adjacent_with_progress(
            &mut self,
            conversation: Uuid,
            operation: OperationId,
            selection: SelectAdjacent,
            deadline: Instant,
            progress: &mut dyn FnMut() -> Result<()>,
        ) -> std::result::Result<Reply, EffectFailure> {
            self.execute_effect(
                conversation,
                operation,
                Command::SelectAdjacent(selection),
                deadline,
                progress,
            )
        }
        fn execute_effect(
            &mut self,
            conversation: Uuid,
            operation: OperationId,
            command: Command,
            deadline: Instant,
            progress: &mut dyn FnMut() -> Result<()>,
        ) -> std::result::Result<Reply, EffectFailure> {
            if let Some(lost) = &self.retention_loss {
                return Err(EffectFailure {
                    operation,
                    lost: lost.clone(),
                    detail: "resident effect requires original uncertainty reconciliation".into(),
                    cause: EffectCause::Unresolved,
                });
            }
            let mut original = None;
            let result = self.client.execute_with_progress(
                operation,
                command,
                deadline,
                |frame, hash| {
                    reserve(&self.store, conversation, frame, hash)
                        .map_err(std::io::Error::other)?;
                    original = Some(frame.clone());
                    Ok(())
                },
                &mut || progress().map_err(progress_error),
            );
            match result {
                Ok(reply) => {
                    let request = original.expect("SDK cannot reply before journal callback");
                    let response = Frame {
                        message: Message::Response {
                            request_sha256: request.fingerprint().unwrap(),
                            reply: reply.clone(),
                        },
                        ..request.clone()
                    };
                    let recorded = encode(&response).and_then(|frame| {
                        finish(&self.store, &request, Outcome::HistoricalReply { frame })
                    });
                    if let Err(error) = recorded {
                        self.client.close();
                        let lost = LostRequest::Indeterminate {
                            operation,
                            request_sha256: request.fingerprint().unwrap(),
                        };
                        self.retention_loss = Some(lost.clone());
                        return Err(EffectFailure {
                            operation,
                            lost,
                            detail: format!("native reply retention failed: {error}"),
                            cause: EffectCause::Retention,
                        });
                    }
                    Ok(reply)
                }
                Err(error) => {
                    let mut detail = format!("resident effect failed: {:?}", error.cause);
                    if error.lost == LostRequest::NoPendingMutation {
                        if let Some(request) = original {
                            if let Err(record_error) =
                                finish(&self.store, &request, Outcome::NotDispatched {})
                            {
                                detail.push_str(&format!(
                                    "; predispatch proof retention failed: {record_error}"
                                ));
                            }
                        }
                    }
                    Err(EffectFailure {
                        operation,
                        lost: self.retention_loss.clone().unwrap_or(error.lost),
                        detail,
                        cause: EffectCause::Transport(error.cause),
                    })
                }
            }
        }
        /// Reuses the most recent stored original source and target. Presence
        /// is historical and never clears the reservation or authorizes replay.
        pub fn reconcile(
            &mut self,
            conversation: Uuid,
            operation: OperationId,
            deadline: Instant,
        ) -> Result<Reply> {
            self.reconcile_with_progress(conversation, operation, deadline, &mut || Ok(()))
        }
        pub fn reconcile_with_progress(
            &mut self,
            conversation: Uuid,
            operation: OperationId,
            deadline: Instant,
            progress: &mut dyn FnMut() -> Result<()>,
        ) -> Result<Reply> {
            let original = attempts(&self.store)?
                .into_iter()
                .rev()
                .find(|a| a.conversation == conversation)
                .context("native attempt missing")?;
            self.reconcile_original(original, operation, deadline, progress)
        }
        /// Inspect one specific consumed operation, including an older attempt.
        /// A mismatched conversation or unknown operation refuses before IO.
        pub fn reconcile_attempt(
            &mut self,
            conversation: Uuid,
            original_operation: OperationId,
            operation: OperationId,
            deadline: Instant,
        ) -> Result<Reply> {
            self.reconcile_attempt_with_progress(
                conversation,
                original_operation,
                operation,
                deadline,
                &mut || Ok(()),
            )
        }
        pub fn reconcile_attempt_with_progress(
            &mut self,
            conversation: Uuid,
            original_operation: OperationId,
            operation: OperationId,
            deadline: Instant,
            progress: &mut dyn FnMut() -> Result<()>,
        ) -> Result<Reply> {
            let original = attempts(&self.store)?
                .into_iter()
                .find(|a| {
                    a.conversation == conversation
                        && a.request.frame().operation == original_operation
                })
                .context("native original attempt missing")?;
            self.reconcile_original(original, operation, deadline, progress)
        }
        fn reconcile_original(
            &mut self,
            original: HistoricalAttempt,
            operation: OperationId,
            deadline: Instant,
            progress: &mut dyn FnMut() -> Result<()>,
        ) -> Result<Reply> {
            ensure!(
                operation != original.request.frame().operation,
                "reconciliation requires new operation"
            );
            let command = match original.request.frame().message.clone() {
                Message::Request(Command::InsertAfter(insertion)) => Command::Reconcile(insertion),
                Message::Request(Command::SelectAdjacent(selection)) => {
                    Command::ReconcileSelection(selection)
                }
                _ => unreachable!("validated journal contains only effects"),
            };
            self.client
                .execute_with_progress(operation, command, deadline, |_, _| Ok(()), &mut || {
                    progress().map_err(progress_error)
                })
                .map_err(|error| self.transport_failure(error).into())
        }
        fn transport_failure(&self, error: TransportError) -> ConnectionFailure {
            ConnectionFailure {
                cause: error.cause,
                lost: self.retention_loss.clone().unwrap_or(error.lost),
            }
        }
        /// Runtime owner carries uncertainty across explicitly renewed sockets.
        pub(in crate::workflow) fn retain_loss(&mut self, lost: LostRequest) {
            if lost != LostRequest::NoPendingMutation {
                self.retention_loss = Some(lost);
            }
        }
        pub fn close(&mut self) -> LostRequest {
            let lost = self.client.close();
            self.retention_loss.clone().unwrap_or(lost)
        }
    }
}
#[cfg(all(test, target_os = "linux"))]
pub(super) use linux::progress_error;
#[cfg(target_os = "linux")]
pub use linux::{
    ConnectionFailure, EffectCause, EffectFailure, InsertionFailure, ResidentConnection,
};

#[cfg(test)]
mod tests;

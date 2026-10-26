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
        publication_fault(PublicationPoint::AfterOutcome)?;
        Ok(())
    })
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use remarkable_open_sdk::{
        native_adapter::{
            transport::Client, ConnectionIdentity, InsertAfter, LostRequest, SelectAdjacent,
        },
        OperationId,
    };
    use std::{fs::File, sync::Arc, time::Instant};

    #[derive(Debug)]
    pub struct EffectFailure {
        pub operation: OperationId,
        pub lost: LostRequest,
        pub detail: String,
    }
    pub type InsertionFailure = EffectFailure;
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
                &mut || progress().map_err(std::io::Error::other),
            )
            .map_err(|error| anyhow::anyhow!("resident connection refused: {error:?}"))?;
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
                    &mut || progress().map_err(std::io::Error::other),
                )
                .map_err(|error| anyhow::anyhow!("resident observation failed: {error:?}"))
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
                &mut || progress().map_err(std::io::Error::other),
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
                    progress().map_err(std::io::Error::other)
                })
                .map_err(|error| anyhow::anyhow!("resident reconciliation failed: {error:?}"))
        }
        pub fn close(&mut self) -> LostRequest {
            let lost = self.client.close();
            self.retention_loss.clone().unwrap_or(lost)
        }
    }
}
#[cfg(target_os = "linux")]
pub use linux::{EffectFailure, InsertionFailure, ResidentConnection};

#[cfg(test)]
mod tests;

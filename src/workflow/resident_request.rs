//! Request lifetime within an already owned resident session. No activation,
//! native qualification, automatic reconnect or effect replay occurs here.
use super::{
    resident_client::{ConnectionFailure, EffectCause, EffectFailure, ResidentConnection},
    resident_runtime::ResidentRuntime,
    resident_supervisor::service::CommandFailure,
};
use anyhow::Result;
use remarkable_open_sdk::{
    native_adapter::{
        transport::Failure, InsertAfter, LostRequest, ProtocolError, Reply, SelectAdjacent,
    },
    OperationId,
};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputProgress {
    Quiet,
    Canceled,
}

pub enum Request {
    Observe,
    Insert {
        conversation: crate::storage::Uuid,
        insertion: InsertAfter,
    },
    Select {
        conversation: crate::storage::Uuid,
        selection: SelectAdjacent,
    },
    Reconcile {
        conversation: crate::storage::Uuid,
        original: OperationId,
    },
}
impl Request {
    fn dispatch(
        self,
        connection: &mut ResidentConnection,
        operation: OperationId,
        deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Reply> {
        match self {
            Self::Observe => connection.observe_with_progress(operation, deadline, progress),
            Self::Insert {
                conversation,
                insertion,
            } => connection
                .insert_after_with_progress(conversation, operation, insertion, deadline, progress)
                .map_err(anyhow::Error::new),
            Self::Select {
                conversation,
                selection,
            } => connection
                .select_adjacent_with_progress(
                    conversation,
                    operation,
                    selection,
                    deadline,
                    progress,
                )
                .map_err(anyhow::Error::new),
            Self::Reconcile {
                conversation,
                original,
            } => connection.reconcile_attempt_with_progress(
                conversation,
                original,
                operation,
                deadline,
                progress,
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    Canceled,
    Deadline,
    Connection,
    Unresolved,
    Refused,
    Integrity,
}
#[derive(Debug)]
pub struct RequestFailure {
    pub kind: FailureKind,
    pub lost: LostRequest,
    pub error: anyhow::Error,
}
impl std::fmt::Display for RequestFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "resident request {:?}: {:#}", self.kind, self.error)
    }
}
impl std::error::Error for RequestFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.error.as_ref())
    }
}

#[derive(Debug)]
pub(crate) struct DeadlineExpired(pub &'static str);
impl std::fmt::Display for DeadlineExpired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for DeadlineExpired {}
pub(crate) fn before(deadline: Instant, label: &'static str) -> Result<()> {
    if Instant::now() >= deadline {
        return Err(DeadlineExpired(label).into());
    }
    Ok(())
}
#[derive(Debug)]
pub(crate) struct IntegrityFailure(pub anyhow::Error);
impl std::fmt::Display for IntegrityFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "resident integrity failed: {:#}", self.0)
    }
}
impl std::error::Error for IntegrityFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}
pub(crate) fn integrity(error: anyhow::Error) -> anyhow::Error {
    IntegrityFailure(error).into()
}
#[derive(Debug)]
struct Canceled;
impl std::fmt::Display for Canceled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("resident request canceled")
    }
}
impl std::error::Error for Canceled {}

fn admission(
    deadline: Instant,
    input: &mut dyn FnMut() -> Result<InputProgress>,
    authority: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    before(deadline, "resident request deadline expired")?;
    if input().map_err(integrity)? == InputProgress::Canceled {
        return Err(Canceled.into());
    }
    before(deadline, "resident request deadline expired")?;
    authority().map_err(|error| {
        // Only an explicitly typed deadline can retire this request. Other
        // authority errors remain fatal even when the clock has since expired.
        if error.chain().any(|cause| cause.is::<DeadlineExpired>()) {
            error
        } else {
            integrity(error)
        }
    })?;
    if input().map_err(integrity)? == InputProgress::Canceled {
        return Err(Canceled.into());
    }
    before(deadline, "resident request deadline expired")
}

fn transport_kind(cause: &Failure) -> FailureKind {
    match cause {
        Failure::Deadline => FailureKind::Deadline,
        Failure::Protocol(ProtocolError::Closed) => FailureKind::Connection,
        Failure::Protocol(ProtocolError::Busy) => FailureKind::Unresolved,
        Failure::Io(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::NotConnected
            ) =>
        {
            FailureKind::Connection
        }
        Failure::Journal(_) => FailureKind::Refused,
        _ => FailureKind::Integrity,
    }
}
fn classify(error: &anyhow::Error) -> FailureKind {
    // Failed cleanup or a verified integrity failure outranks request retirement.
    if error.chain().any(|cause| {
        cause.is::<IntegrityFailure>()
            || cause
                .downcast_ref::<CommandFailure>()
                .is_some_and(|failure| !failure.reaped)
    }) {
        return FailureKind::Integrity;
    }
    if error.chain().any(|cause| cause.is::<Canceled>()) {
        return FailureKind::Canceled;
    }
    if error.chain().any(|cause| cause.is::<DeadlineExpired>()) {
        return FailureKind::Deadline;
    }
    for cause in error.chain() {
        if let Some(failure) = cause.downcast_ref::<ConnectionFailure>() {
            return transport_kind(&failure.cause);
        }
        if let Some(failure) = cause.downcast_ref::<EffectFailure>() {
            return match &failure.cause {
                EffectCause::Transport(cause) => transport_kind(cause),
                EffectCause::Retention | EffectCause::Unresolved => FailureKind::Unresolved,
            };
        }
    }
    FailureKind::Refused
}
fn finish<T>(
    runtime: &mut ResidentRuntime,
    result: Result<T>,
) -> std::result::Result<T, RequestFailure> {
    result.map_err(|error| RequestFailure {
        kind: classify(&error),
        lost: runtime.retire_connection(),
        error,
    })
}

pub(super) fn execute(
    runtime: &mut ResidentRuntime,
    operation: OperationId,
    request: Request,
    deadline: Instant,
    input: &mut dyn FnMut() -> Result<InputProgress>,
    authority: &mut dyn FnMut() -> Result<()>,
) -> std::result::Result<Reply, RequestFailure> {
    let result = runtime.with_connection(
        deadline,
        &mut || admission(deadline, input, authority),
        |connection, progress| request.dispatch(connection, operation, deadline, progress),
    );
    finish(runtime, result)
}
pub(super) fn reconnect(
    runtime: &mut ResidentRuntime,
    deadline: Instant,
    input: &mut dyn FnMut() -> Result<InputProgress>,
    authority: &mut dyn FnMut() -> Result<()>,
) -> std::result::Result<(), RequestFailure> {
    let result = runtime.reconnect(deadline, &mut || admission(deadline, input, authority));
    finish(runtime, result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_callback_causes_survive_transport_context_and_cleanup_precedence() {
        for kind in [
            FailureKind::Canceled,
            FailureKind::Deadline,
            FailureKind::Integrity,
        ] {
            for context in [false, true] {
                for effect in [false, true] {
                    let original: anyhow::Error = match kind {
                        FailureKind::Canceled => Canceled.into(),
                        FailureKind::Deadline => DeadlineExpired("owned deadline").into(),
                        _ => integrity(DeadlineExpired("nested deadline must stay fatal").into()),
                    };
                    let original = if context {
                        original.context("caller context")
                    } else {
                        original
                    };
                    let cause =
                        Failure::Progress(super::super::resident_client::progress_error(original));
                    let error: anyhow::Error = if effect {
                        EffectFailure {
                            operation: OperationId(
                                remarkable_open_sdk::Uuid::parse(
                                    "00000000-0000-4000-8000-000000000001",
                                )
                                .unwrap(),
                            ),
                            cause: EffectCause::Transport(cause),
                            lost: LostRequest::NoPendingMutation,
                            detail: "owned effect callback".into(),
                        }
                        .into()
                    } else {
                        ConnectionFailure {
                            cause,
                            lost: LostRequest::NoPendingMutation,
                        }
                        .into()
                    };
                    assert_eq!(classify(&error.context("outer context")), kind);
                }
            }
        }
        for reaped in [false, true] {
            let error = anyhow::Error::new(CommandFailure {
                pid: 1,
                reaped,
                error: DeadlineExpired("owned command deadline").into(),
            });
            assert_eq!(
                classify(&error),
                if reaped {
                    FailureKind::Deadline
                } else {
                    FailureKind::Integrity
                }
            );
        }
        // Text and elapsed clocks must not upgrade unrelated failures to timeout.
        let error = admission(
            Instant::now() + std::time::Duration::from_secs(1),
            &mut || Ok(InputProgress::Quiet),
            &mut || anyhow::bail!("deadline expired"),
        );
        assert_eq!(classify(&error.unwrap_err()), FailureKind::Integrity);
    }
}

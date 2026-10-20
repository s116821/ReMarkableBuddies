use super::*;
use crate::{
    conversation::{ExportAction, ExportAttempt, ExportBaseline, Ledger, Outcome},
    storage::{digest, Namespace},
};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::watch;

struct Owner<'a>(&'a AtomicBool);
impl<'a> Owner<'a> {
    fn acquire(ledger: &'a Ledger) -> Option<Self> {
        let active = &ledger.store().export_active;
        active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        Some(Self(active))
    }
}
impl Drop for Owner<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

async fn pending<T>(
    future: impl Future<Output = Result<T>>,
    canceled: &mut watch::Receiver<bool>,
    budget: Budget,
) -> Result<Option<T>> {
    if *canceled.borrow() {
        return Ok(None);
    }
    tokio::pin!(future);
    loop {
        tokio::select! {
            biased;
            changed = canceled.changed() => {
                if changed.is_err() || *canceled.borrow() { return Ok(None); }
            }
            result = tokio::time::timeout(budget.remaining()?, &mut future) => {
                return result.map_err(|_| Failure::Deadline)?.map(Some);
            }
        }
    }
}

async fn dispatch(
    future: impl Future<Output = Result<RemoteNote>>,
    canceled: &mut watch::Receiver<bool>,
    budget: Budget,
) -> (Result<Option<RemoteNote>>, bool) {
    let polled = AtomicBool::new(false);
    let observed = async {
        polled.store(true, Ordering::Release);
        future.await
    };
    let result = pending(observed, canceled, budget).await;
    (result, polled.load(Ordering::Acquire))
}

fn report(a: &ExportAssociation, status: ExportStatus) -> ExportReport {
    let id = a.attempt.as_ref().map_or(a.operation, |attempt| attempt.id);
    let mut result = ExportReport::new(status, a.id, id);
    result.native_note = a.native_note.clone();
    result
}
fn store_state(ledger: &Ledger, a: ExportAssociation) -> Result<bool> {
    let root = ledger
        .expected(Namespace::Conversation, a.conversation)
        .map_err(|_| Failure::Storage)?;
    let heads = ledger
        .expected(Namespace::ExportAssociation, a.id)
        .map_err(|_| Failure::Storage)?;
    ledger
        .associate_export_observed(a.operation, root, heads, a)
        .map(|(_, fresh)| fresh)
        .map_err(|_| Failure::Storage)
}
fn finish_no_send(
    ledger: &Ledger,
    mut a: ExportAssociation,
    status: ExportStatus,
    failure: Option<Failure>,
) -> ExportReport {
    let Some(attempt) = a.attempt.as_mut() else {
        let mut result = report(&a, ExportStatus::Refused);
        result.failure = Some(Failure::Storage);
        return result;
    };
    attempt.no_send = true;
    a.outcome = if status == ExportStatus::CanceledNoEffect {
        Outcome::Canceled
    } else {
        Outcome::Failed
    };
    a.operation = attempt.stage_id(if a.outcome == Outcome::Canceled {
        "canceled-no-send"
    } else {
        "failed-no-send"
    });
    let mut result = report(&a, status);
    result.failure = failure;
    if store_state(ledger, a).is_err() {
        result.status = ExportStatus::Refused;
        result.failure = Some(Failure::Storage);
    }
    result
}
fn uncertain(ledger: &Ledger, mut a: ExportAssociation, failure: Option<Failure>) -> ExportReport {
    let mut result = report(&a, ExportStatus::ReconcileRequired);
    result.failure = failure;
    if a.outcome == Outcome::Prepared {
        a.outcome = Outcome::ReconcileRequired;
        if let Some(attempt) = &a.attempt {
            a.operation = attempt.stage_id("uncertain");
        }
        if store_state(ledger, a).is_err() {
            result.failure = Some(Failure::Storage);
        }
    }
    result
}
fn complete(
    ledger: &Ledger,
    destination: &impl NoteDestination,
    mut a: ExportAssociation,
    note: RemoteNote,
) -> Result<ExportReport> {
    let attempt = a.attempt.as_ref().ok_or(Failure::Unsupported)?;
    verify_note(&note, &a, destination.correlation_location())?;
    if note.id != attempt.target
        || digest(note.content.as_bytes()) != attempt.body_sha256
        || attempt.baseline.as_ref().is_some_and(|baseline| {
            note.version < baseline.version
                || (attempt.body_sha256 != baseline.body_sha256 && note.version == baseline.version)
        })
    {
        return Err(Failure::Conflict);
    }
    a.outcome = Outcome::Completed;
    a.native_note = Some(note.id);
    let attempt = a.attempt.as_mut().ok_or(Failure::Unsupported)?;
    attempt.confirmed_version = Some(note.version);
    a.operation = attempt.stage_id("completed");
    store_state(ledger, a.clone())?;
    Ok(report(&a, ExportStatus::VerifiedCurrent))
}

async fn reconcile_owned(
    ledger: &Ledger,
    destination: &impl NoteDestination,
    a: ExportAssociation,
    canceled: &mut watch::Receiver<bool>,
    budget: Budget,
) -> ExportReport {
    let Some(attempt) = a.attempt.as_ref() else {
        let mut result = report(&a, ExportStatus::Refused);
        result.failure = Some(Failure::Unsupported);
        return result;
    };
    if a.backend != destination.backend() {
        let mut result = report(&a, ExportStatus::Conflict);
        result.failure = Some(Failure::Conflict);
        return result;
    }
    if a.outcome == Outcome::Completed {
        return report(&a, ExportStatus::HistoricalCompleted);
    }
    if attempt.no_send && matches!(a.outcome, Outcome::Canceled | Outcome::Failed) {
        return report(
            &a,
            if a.outcome == Outcome::Canceled {
                ExportStatus::CanceledNoEffect
            } else {
                ExportStatus::Refused
            },
        );
    }
    match pending(destination.read(&attempt.target, budget), canceled, budget).await {
        Ok(Some(Some(note))) => match complete(ledger, destination, a.clone(), note) {
            Ok(result) => result,
            Err(failure) => {
                let mut result = report(
                    &a,
                    if failure == Failure::Conflict {
                        ExportStatus::Conflict
                    } else {
                        ExportStatus::ReconcileRequired
                    },
                );
                result.failure = Some(failure);
                result
            }
        },
        Ok(_) => report(&a, ExportStatus::ReconcileRequired),
        Err(failure) => {
            let mut result = report(&a, ExportStatus::ReconcileRequired);
            result.failure = Some(failure);
            result
        }
    }
}

/// Reconcile only by reading a known ID. Never create/update from an old receipt.
pub async fn reconcile(
    ledger: &Ledger,
    destination: &impl NoteDestination,
    conversation: Uuid,
    correlation: Uuid,
    mut canceled: watch::Receiver<bool>,
) -> ExportReport {
    let Some(_owner) = Owner::acquire(ledger) else {
        return ExportReport::new(ExportStatus::Busy, correlation, Uuid::nil());
    };
    let budget = Budget::default();
    match ledger.export_association(conversation, correlation) {
        Ok(Some(a)) => reconcile_owned(ledger, destination, a, &mut canceled, budget).await,
        _ => {
            let mut result = ExportReport::new(ExportStatus::Refused, correlation, Uuid::nil());
            result.failure = Some(Failure::Storage);
            result
        }
    }
}

/// The present explicit request plus a fresh local preparation permits one send.
/// The shared Store owner remains held through every await and result transition.
pub async fn export(
    ledger: &Ledger,
    destination: &impl NoteDestination,
    note: &PreparedNote,
    mut canceled: watch::Receiver<bool>,
) -> ExportReport {
    let mut refusal = ExportReport::new(
        ExportStatus::Refused,
        note.correlation,
        note.effect_operation,
    );
    let Some(_owner) = Owner::acquire(ledger) else {
        refusal.status = ExportStatus::Busy;
        return refusal;
    };
    let budget = Budget::default();
    let body = match note.render(destination.correlation_location()) {
        Ok(body) => body,
        Err(failure) => {
            refusal.failure = Some(failure);
            return refusal;
        }
    };
    match ledger.historical_export_attempt(
        note.conversation,
        note.correlation,
        note.effect_operation,
    ) {
        Ok(Some(previous)) => {
            if previous.backend != destination.backend()
                || previous.source_scope != note.source_scope
                || previous.source_revision != note.source_revision
                || previous
                    .attempt
                    .as_ref()
                    .is_none_or(|a| a.body_sha256 != digest(body.as_bytes()))
            {
                refusal.status = ExportStatus::Conflict;
                return refusal;
            }
            if previous.outcome == Outcome::Completed {
                return report(&previous, ExportStatus::HistoricalCompleted);
            }
            if previous.attempt.as_ref().is_some_and(|a| a.no_send) {
                return report(
                    &previous,
                    if previous.outcome == Outcome::Canceled {
                        ExportStatus::CanceledNoEffect
                    } else {
                        ExportStatus::Refused
                    },
                );
            }
        }
        Ok(None) => {}
        Err(_) => {
            refusal.failure = Some(Failure::Storage);
            return refusal;
        }
    }
    let heads = match ledger.expected(Namespace::ExportAssociation, note.correlation) {
        Ok(heads) => heads,
        Err(_) => {
            refusal.failure = Some(Failure::Storage);
            return refusal;
        }
    };
    let old = match ledger.export_association(note.conversation, note.correlation) {
        Ok(old) => old,
        Err(_) => {
            refusal.failure = Some(Failure::Storage);
            return refusal;
        }
    };
    let body_sha256 = digest(body.as_bytes());
    if let Some(old) = &old {
        if old.backend != destination.backend() || old.source_scope != note.source_scope {
            refusal.status = ExportStatus::Conflict;
            return refusal;
        }
        let Some(attempt) = &old.attempt else {
            refusal.failure = Some(Failure::Unsupported);
            return refusal;
        };
        if attempt.id == note.effect_operation {
            if attempt.body_sha256 != body_sha256 || old.source_revision != note.source_revision {
                refusal.status = ExportStatus::Conflict;
                return refusal;
            }
            return reconcile_owned(ledger, destination, old.clone(), &mut canceled, budget).await;
        }
        if old.outcome != Outcome::Completed
            && !(attempt.no_send && matches!(old.outcome, Outcome::Canceled | Outcome::Failed))
        {
            return report(old, ExportStatus::ReconcileRequired);
        }
    }
    let baseline = old.as_ref().and_then(|old| {
        old.attempt.as_ref().and_then(|attempt| {
            if old.outcome == Outcome::Completed {
                attempt.confirmed_version.map(|version| ExportBaseline {
                    version,
                    body_sha256: attempt.body_sha256.clone(),
                })
            } else {
                attempt.baseline.clone()
            }
        })
    });
    let native_note = old.as_ref().and_then(|a| a.native_note.clone());
    let target = native_note
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let action = if native_note.is_some() {
        ExportAction::Update
    } else {
        ExportAction::Create
    };
    let attempt = ExportAttempt {
        id: note.effect_operation,
        action,
        body_sha256,
        target,
        baseline,
        confirmed_version: None,
        no_send: false,
    };
    let a = ExportAssociation {
        id: note.correlation,
        conversation: note.conversation,
        backend: destination.backend().to_owned(),
        native_note,
        source_scope: note.source_scope.clone(),
        source_revision: note.source_revision.clone(),
        operation: attempt.stage_id("prepared"),
        outcome: Outcome::Prepared,
        attempt: Some(attempt),
    };
    let root = match ledger.expected(Namespace::Conversation, a.conversation) {
        Ok(root) => root,
        Err(_) => {
            refusal.failure = Some(Failure::Storage);
            return refusal;
        }
    };
    match ledger.associate_export_observed(a.operation, root, heads, a.clone()) {
        Ok((_, true)) => {}
        Ok((_, false)) => {
            return match ledger.export_association(a.conversation, a.id) {
                Ok(Some(current)) => {
                    reconcile_owned(ledger, destination, current, &mut canceled, budget).await
                }
                _ => {
                    refusal.failure = Some(Failure::Storage);
                    refusal
                }
            }
        }
        Err(_) => {
            refusal.failure = Some(Failure::Storage);
            return refusal;
        }
    }
    if *canceled.borrow() {
        return finish_no_send(ledger, a, ExportStatus::CanceledNoEffect, None);
    }
    let Some(attempt) = a.attempt.as_ref() else {
        return refusal;
    };
    enum Preflight {
        Ready,
        Current(RemoteNote),
        Refuse(ExportStatus),
    }
    let preflight = async {
        match attempt.action {
            ExportAction::Create => match destination.discover(a.id, budget).await? {
                Discovery::Missing => {
                    if destination.read(&attempt.target, budget).await?.is_some() {
                        Ok(Preflight::Refuse(ExportStatus::Conflict))
                    } else {
                        Ok(Preflight::Ready)
                    }
                }
                Discovery::Incomplete => Ok(Preflight::Refuse(ExportStatus::Incomplete)),
                Discovery::Unique(_) | Discovery::Duplicate => {
                    Ok(Preflight::Refuse(ExportStatus::Conflict))
                }
            },
            ExportAction::Update => {
                let current = destination
                    .read(&attempt.target, budget)
                    .await?
                    .ok_or(Failure::Conflict)?;
                verify_note(&current, &a, destination.correlation_location())?;
                let baseline = attempt.baseline.as_ref().ok_or(Failure::Conflict)?;
                if current.id != attempt.target {
                    return Err(Failure::Conflict);
                }
                if digest(current.content.as_bytes()) == attempt.body_sha256 {
                    return Ok(Preflight::Current(current));
                }
                if current.version != baseline.version
                    || digest(current.content.as_bytes()) != baseline.body_sha256
                {
                    return Err(Failure::Conflict);
                }
                Ok(Preflight::Ready)
            }
        }
    };
    match pending(preflight, &mut canceled, budget).await {
        Ok(Some(Preflight::Ready)) => {}
        Ok(Some(Preflight::Current(current))) => {
            return match complete(ledger, destination, a.clone(), current) {
                Ok(result) => result,
                Err(failure) => finish_no_send(ledger, a, ExportStatus::Conflict, Some(failure)),
            }
        }
        Ok(Some(Preflight::Refuse(status))) => return finish_no_send(ledger, a, status, None),
        Ok(None) => return finish_no_send(ledger, a, ExportStatus::CanceledNoEffect, None),
        Err(failure) => {
            return finish_no_send(
                ledger,
                a,
                if failure == Failure::Conflict {
                    ExportStatus::Conflict
                } else {
                    ExportStatus::Refused
                },
                Some(failure),
            )
        }
    }
    if *canceled.borrow() {
        return finish_no_send(ledger, a, ExportStatus::CanceledNoEffect, None);
    }
    if let Err(failure) = budget.remaining() {
        return finish_no_send(ledger, a, ExportStatus::Refused, Some(failure));
    }
    let (result, polled) = match attempt.action {
        ExportAction::Create => {
            dispatch(
                destination.create(&attempt.target, &body, a.id, budget),
                &mut canceled,
                budget,
            )
            .await
        }
        ExportAction::Update => {
            dispatch(
                destination.update(
                    &attempt.target,
                    &body,
                    attempt.baseline.as_ref().map_or(0, |b| b.version),
                    a.id,
                    budget,
                ),
                &mut canceled,
                budget,
            )
            .await
        }
    };
    if !polled {
        return match result {
            Ok(None) => finish_no_send(ledger, a, ExportStatus::CanceledNoEffect, None),
            Err(failure) => finish_no_send(ledger, a, ExportStatus::Refused, Some(failure)),
            Ok(Some(_)) => finish_no_send(
                ledger,
                a,
                ExportStatus::Refused,
                Some(Failure::InvalidResponse),
            ),
        };
    }
    match result {
        Ok(Some(remote)) => match complete(ledger, destination, a.clone(), remote) {
            Ok(result) => result,
            Err(failure) => uncertain(ledger, a, Some(failure)),
        },
        Ok(None) => uncertain(ledger, a, None),
        Err(failure) => uncertain(ledger, a, Some(failure)),
    }
}

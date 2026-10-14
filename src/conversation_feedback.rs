//! Static descriptions of historical facts, never rendering or cancellation authority.
use crate::conversation::{AttemptReason, Outcome, OutcomeFact};
use crate::storage::Uuid;
use anyhow::{ensure, Result};
use serde::Serialize;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FeedbackKind {
    Error,
    Canceled,
    Reconciliation,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FeedbackDescription {
    pub fact: Uuid,
    pub conversation: Uuid,
    pub turn: Uuid,
    pub kind: FeedbackKind,
    pub code: &'static str,
    pub text: &'static str,
}
/// Describe already validated owned facts. No logs, request/answer text, current
/// page identity, provider access or native action is consumed or authorized.
/// Completed history produces no new error or success receipt. The caller must
/// separately admit any later presentation and persist a pending message when no
/// usable Buddy page exists. Responsive cancellation remains a runtime concern.
pub fn describe(fact: &OutcomeFact) -> Result<Option<FeedbackDescription>> {
    ensure!(
        !fact.id.is_nil() && !fact.conversation.is_nil() && !fact.turn.is_nil(),
        "feedback identity unavailable"
    );
    if fact.outcome == Outcome::Completed {
        return Ok(None);
    }
    ensure!(
        matches!(
            fact.outcome,
            Outcome::Failed | Outcome::Canceled | Outcome::ReconcileRequired
        ),
        "unsupported feedback state"
    );
    let uncertain = fact.outcome == Outcome::ReconcileRequired
        || matches!(
            fact.reason,
            AttemptReason::OutputPending
                | AttemptReason::SubmittedUnverified
                | AttemptReason::DeviceUncertain
        );
    let (kind, code, text) = if uncertain {
        (
            FeedbackKind::Reconciliation,
            "RMB-ACTION-UNCERTAIN",
            "The previous action needs checking.",
        )
    } else if fact.outcome == Outcome::Canceled {
        (
            FeedbackKind::Canceled,
            "RMB-REQUEST-CANCELED",
            "This request was canceled.",
        )
    } else {
        let (code, text) = match fact.reason {
            AttemptReason::NoSelection => {
                ("RMB-NO-SELECTION", "No usable source selection was found.")
            }
            AttemptReason::TranscriptionDisagreement => (
                "RMB-READ-UNCERTAIN",
                "The selected text could not be read reliably.",
            ),
            AttemptReason::ProviderFailure => (
                "RMB-RESPONSE-UNAVAILABLE",
                "The response service could not complete this request.",
            ),
            AttemptReason::NativeOutputUnavailable => (
                "RMB-OUTPUT-UNAVAILABLE",
                "Output is not available for this request.",
            ),
            AttemptReason::NoSuccessor => (
                "RMB-PAGE-UNAVAILABLE",
                "A usable Buddy page is unavailable.",
            ),
            AttemptReason::InvalidSuccessor => (
                "RMB-PAGE-UNCONFIRMED",
                "The Buddy page could not be confirmed.",
            ),
            AttemptReason::RequestFailed | AttemptReason::Canceled => {
                ("RMB-REQUEST-FAILED", "This request could not be completed.")
            }
            AttemptReason::OutputPending
            | AttemptReason::SubmittedUnverified
            | AttemptReason::DeviceUncertain => unreachable!("uncertainty handled first"),
        };
        (FeedbackKind::Error, code, text)
    };
    Ok(Some(FeedbackDescription {
        fact: fact.id,
        conversation: fact.conversation,
        turn: fact.turn,
        kind,
        code,
        text,
    }))
}

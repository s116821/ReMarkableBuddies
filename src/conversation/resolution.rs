//! Read-only declared inheritance. Never active intent or current effect authority.
use super::{receiving::*, Ledger, Mode, Outcome, Record, Role, Turn};
use crate::{retrieval::IndexKey, storage::Uuid};
use anyhow::{ensure, Result};
use std::fmt;

#[derive(Clone, Debug)]
pub enum SourceDirective {
    Explicit(RequestSource),
    /// The caller chooses an operation; the ledger never guesses the active one.
    Inherit {
        request_turn: Uuid,
        operation: Option<Uuid>,
        /// Independently obtained key, not the key saved in historical evidence.
        expected: Option<IndexKey>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceResolutionRefusal {
    InvalidRequest,
    HistoryUnavailable,
    AnchorRequired,
    AnchorUnavailable,
    NotEarlier,
    IndependentKeyRequired,
    UnexpectedKey,
    InvalidSource,
    ChangedSource,
}
impl fmt::Display for SourceResolutionRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidRequest => "invalid source-resolution request",
            Self::HistoryUnavailable => "source-resolution history unavailable",
            Self::AnchorRequired => "explicit source inheritance operation required",
            Self::AnchorUnavailable => "source inheritance anchor unavailable",
            Self::NotEarlier => "source inheritance request is not earlier",
            Self::IndependentKeyRequired => "independent source index key required",
            Self::UnexpectedKey => "no-document inheritance cannot carry an index key",
            Self::InvalidSource => "source-resolution association or key invalid",
            Self::ChangedSource => "inherited source content changed",
        })
    }
}
impl std::error::Error for SourceResolutionRefusal {}

fn request(records: &[Record], conversation: Uuid, id: Uuid) -> Result<&Turn> {
    records
        .iter()
        .find_map(|record| match record {
            Record::Turn(t)
                if t.id == id
                    && t.conversation == conversation
                    && t.mode == Mode::Reader
                    && t.role == Role::User
                    && t.outcome == Outcome::Interpreted
                    && t.text
                        .as_ref()
                        .is_some_and(|s| !s.trim().is_empty() && s.len() <= 64 * 1024) =>
            {
                Some(t)
            }
            _ => None,
        })
        .ok_or_else(|| SourceResolutionRefusal::InvalidRequest.into())
}

impl Ledger {
    /// Resolve the caller's declaration as ordinary historical data only.
    /// A subsequent receiver must validate its own snapshot and Provider key.
    pub fn resolve_request_source(
        &self,
        conversation: Uuid,
        request_turn: Uuid,
        directive: SourceDirective,
    ) -> Result<RequestSource> {
        use SourceResolutionRefusal as Refusal;
        ensure!(
            !conversation.is_nil() && !request_turn.is_nil(),
            Refusal::InvalidRequest
        );
        let anchor = match &directive {
            SourceDirective::Inherit {
                request_turn,
                operation,
                ..
            } => {
                let operation = operation.ok_or(Refusal::AnchorRequired)?;
                ensure!(
                    !request_turn.is_nil() && !operation.is_nil(),
                    Refusal::AnchorUnavailable
                );
                let anchor = self
                    .request_evidence(conversation, *request_turn, operation)
                    .map_err(|_| Refusal::AnchorUnavailable)?
                    .ok_or(Refusal::AnchorUnavailable)?;
                #[cfg(test)]
                AFTER_HISTORY.with(|hook| {
                    let callback = hook.borrow_mut().take();
                    if let Some(callback) = callback {
                        callback();
                    }
                });
                Some(anchor.record)
            }
            SourceDirective::Explicit(_) => None,
        };
        let records = self
            .inspect(conversation, false)
            .map_err(|_| Refusal::HistoryUnavailable)?;
        let current = request(&records, conversation, request_turn)?;
        match directive {
            SourceDirective::Explicit(choice) => {
                match original(&records, current, choice, 512 * 1024)
                    .map_err(|_| Refusal::InvalidSource)?
                {
                    SourceAssociation::NoDocument => Ok(RequestSource::NoDocument),
                    SourceAssociation::Original {
                        origin_turn,
                        source_record,
                        expected,
                    } => Ok(RequestSource::Original {
                        origin_turn,
                        source_record: source_id(&source_record).ok_or(Refusal::InvalidSource)?,
                        expected,
                    }),
                }
            }
            SourceDirective::Inherit { expected, .. } => {
                let anchor = anchor.ok_or(Refusal::AnchorUnavailable)?;
                // Historical readback and current inspection are separate reads.
                // A disappeared/replaced anchor must not survive that boundary.
                ensure!(
                    records.iter().any(
                        |r| matches!(r, Record::RequestEvidence(fact) if fact.as_ref() == &anchor)
                    ),
                    Refusal::AnchorUnavailable
                );
                let prior = request(&records, conversation, anchor.request_turn)?;
                ensure!(prior.sequence < current.sequence, Refusal::NotEarlier);
                match recorded_source(&anchor.frame_json).map_err(|_| Refusal::AnchorUnavailable)? {
                    SourceAssociation::NoDocument => {
                        ensure!(expected.is_none(), Refusal::UnexpectedKey);
                        Ok(RequestSource::NoDocument)
                    }
                    SourceAssociation::Original {
                        origin_turn,
                        source_record,
                        ..
                    } => {
                        let source_identity =
                            source_id(&source_record).ok_or(Refusal::InvalidSource)?;
                        let choice = RequestSource::Original {
                            origin_turn,
                            source_record: source_identity,
                            expected: expected.ok_or(Refusal::IndependentKeyRequired)?,
                        };
                        let SourceAssociation::Original {
                            origin_turn,
                            source_record: current_source,
                            expected,
                        } = original(&records, current, choice, 512 * 1024)
                            .map_err(|_| Refusal::InvalidSource)?
                        else {
                            return Err(Refusal::InvalidSource.into());
                        };
                        ensure!(current_source == source_record, Refusal::ChangedSource);
                        Ok(RequestSource::Original {
                            origin_turn,
                            source_record: source_identity,
                            expected,
                        })
                    }
                }
            }
        }
    }
}

#[cfg(test)]
thread_local! {
    static AFTER_HISTORY: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
mod tests;

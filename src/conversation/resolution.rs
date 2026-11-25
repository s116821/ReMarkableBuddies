//! Read-only declared inheritance. Never active intent or current effect authority.
use super::{evidence::RequestEvidence, receiving::*, Ledger, Mode, Outcome, Record, Role, Turn};
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
    #[cfg(all(target_os = "linux", feature = "installed-resident-output"))]
    pub(crate) fn bound_resident_reader_source(
        &self,
        request: &Turn,
        binding: &super::BindingReceipt,
    ) -> Result<SourceDirective> {
        use std::collections::BTreeMap;
        use SourceResolutionRefusal as Refusal;

        ensure!(
            binding.conversation == request.conversation,
            Refusal::InvalidSource
        );
        let records = self.inspect(request.conversation, false)?;
        let prior = records
            .iter()
            .filter_map(|record| match record {
                Record::Turn(turn)
                    if turn.role == Role::User && turn.sequence < request.sequence =>
                {
                    Some((turn.id, turn))
                }
                _ => None,
            })
            .collect::<BTreeMap<_, _>>();
        let latest = records
            .iter()
            .filter_map(|record| match record {
                Record::RequestEvidence(evidence) => prior
                    .get(&evidence.request_turn)
                    .filter(|turn| {
                        turn.mode == Mode::Reader && turn.outcome == Outcome::Interpreted
                    })
                    .map(|turn| (turn.sequence, evidence.recorded_ms, evidence.id, evidence)),
                _ => None,
            })
            .max_by_key(|(sequence, time, id, _)| (*sequence, *time, *id));

        if let Some((_, _, _, evidence)) = latest {
            return match recorded_source(&evidence.frame_json)? {
                SourceAssociation::NoDocument => {
                    Ok(SourceDirective::Explicit(RequestSource::NoDocument))
                }
                SourceAssociation::CapturedPage { .. } => Ok(SourceDirective::Inherit {
                    request_turn: evidence.request_turn,
                    operation: Some(evidence.operation),
                    expected: None,
                }),
                SourceAssociation::Original {
                    origin_turn,
                    source_record,
                    ..
                } => {
                    let Record::ResidentCapture(capture) = source_record.as_ref() else {
                        return Err(Refusal::InvalidSource.into());
                    };
                    ensure!(
                        records.contains(source_record.as_ref()),
                        Refusal::ChangedSource
                    );
                    Ok(SourceDirective::Explicit(RequestSource::CapturedPage {
                        origin_turn,
                        source_record: capture.id,
                    }))
                }
            };
        }

        // Older installed conversations may have no request-evidence record.
        // Their insertion receipt identifies the only eligible original page.
        // A blank-start receipt adds a page but has no originating Reader image.
        let original = records
            .iter()
            .filter_map(|record| match record {
                Record::ResidentCapture(capture)
                    if capture.before.document == binding.source.document
                        && capture.before.page == binding.source.page
                        && capture.before.page != binding.observed_page =>
                {
                    prior
                        .get(&capture.turn)
                        .map(|turn| (turn.sequence, capture))
                }
                _ => None,
            })
            .max_by_key(|(sequence, _)| *sequence);
        Ok(SourceDirective::Explicit(original.map_or(
            RequestSource::NoDocument,
            |(_, capture)| RequestSource::CapturedPage {
                origin_turn: capture.turn,
                source_record: capture.id,
            },
        )))
    }

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
        from_records(
            conversation,
            request_turn,
            directive,
            &records,
            anchor.as_ref(),
            false,
            false,
        )
    }
}

/// Only callers that have validated the complete ordinary snapshot may use this.
fn from_records(
    conversation: Uuid,
    request_turn: Uuid,
    directive: SourceDirective,
    records: &[Record],
    historical: Option<&RequestEvidence>,
    exact_original_bytes: bool,
    allow_notebook: bool,
) -> Result<RequestSource> {
    use SourceResolutionRefusal as Refusal;
    ensure!(
        !conversation.is_nil() && !request_turn.is_nil(),
        Refusal::InvalidRequest
    );
    let current = request(records, conversation, request_turn)?;
    match directive {
        SourceDirective::Explicit(choice) => {
            original(records, current, choice, 512 * 1024, allow_notebook)
                .map_err(|_| Refusal::InvalidSource)?
                .request_source()
                .map_err(|_| Refusal::InvalidSource.into())
        }
        SourceDirective::Inherit {
            request_turn: prior_request,
            operation,
            expected,
        } => {
            let operation = operation.ok_or(Refusal::AnchorRequired)?;
            ensure!(
                !prior_request.is_nil() && !operation.is_nil(),
                Refusal::AnchorUnavailable
            );
            let anchor = records
                .iter()
                .find_map(|record| match record {
                    Record::RequestEvidence(fact)
                        if fact.operation == operation
                            && fact.request_turn == prior_request
                            && fact.conversation == conversation
                            && fact.id == RequestEvidence::identity(operation) =>
                    {
                        Some(fact.as_ref())
                    }
                    _ => None,
                })
                .ok_or(Refusal::AnchorUnavailable)?;
            if let Some(historical) = historical {
                // Preserve standalone readback/current-snapshot equality.
                ensure!(anchor == historical, Refusal::AnchorUnavailable);
            }
            let prior = request(records, conversation, anchor.request_turn)?;
            ensure!(prior.sequence < current.sequence, Refusal::NotEarlier);
            match recorded_source(&anchor.frame_json).map_err(|_| Refusal::AnchorUnavailable)? {
                SourceAssociation::NoDocument => {
                    ensure!(expected.is_none(), Refusal::UnexpectedKey);
                    Ok(RequestSource::NoDocument)
                }
                prior @ SourceAssociation::CapturedPage { .. } => {
                    ensure!(expected.is_none(), Refusal::UnexpectedKey);
                    let choice = prior.request_source()?;
                    let source = original(records, current, choice, 512 * 1024, allow_notebook)
                        .map_err(|_| Refusal::InvalidSource)?;
                    ensure!(source == prior, Refusal::ChangedSource);
                    source.request_source()
                }
                SourceAssociation::Original {
                    origin_turn,
                    source_record,
                    expected: original_key,
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
                    } = original(records, current, choice, 512 * 1024, allow_notebook)
                        .map_err(|_| Refusal::InvalidSource)?
                    else {
                        return Err(Refusal::InvalidSource.into());
                    };
                    ensure!(current_source == source_record, Refusal::ChangedSource);
                    if matches!(source_record.as_ref(), Record::ArchivedNormalizedSource(_)) {
                        ensure!(expected == original_key, Refusal::ChangedSource);
                    } else if exact_original_bytes {
                        ensure!(
                            expected.document == original_key.document
                                && expected.format == original_key.format
                                && expected.source_sha256 == original_key.source_sha256,
                            Refusal::ChangedSource
                        );
                    }
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

/// Explicit frames retain their original receiving validation/order/refusals.
pub(super) fn for_frame(
    records: &[Record],
    conversation: Uuid,
    request_turn: Uuid,
    directive: SourceDirective,
    exact_original_bytes: bool,
    allow_notebook: bool,
) -> Result<RequestSource> {
    match directive {
        SourceDirective::Explicit(source) => Ok(source),
        inherited => from_records(
            conversation,
            request_turn,
            inherited,
            records,
            None,
            exact_original_bytes,
            allow_notebook,
        ),
    }
}

#[cfg(test)]
thread_local! {
    static AFTER_HISTORY: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
mod tests;

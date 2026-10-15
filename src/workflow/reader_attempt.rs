//! Durable preparation and fact recording; never owns native side effects.
use super::AcquiredEvidence;
use crate::{
    conversation::*,
    storage::{digest, Media, Namespace, Uuid},
};
use anyhow::{Context, Result};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) struct Attempt {
    pub user: Turn,
    pub assistant: Option<Turn>,
    evidence: Uuid,
    pub legacy_output: bool,
    development_evidence: bool,
    reader_preparation: Option<ReaderPreparation>,
}
fn now() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()
        .context("timestamp overflow")
}

/// Drives private Attempt methods with a persisted Generated draft supplied by
/// the actual selected-store fixture. This does not model input acquisition.
#[cfg(test)]
pub(crate) fn assert_attachment_behavior(
    preparation: ReaderPreparation,
    assistant: Turn,
    evidence: Uuid,
    ledger: &Ledger,
    workflow: &mut super::Workflow,
    fresh: bool,
) {
    let mut user = assistant.clone();
    user.id = Uuid::new_v4();
    user.role = Role::User;
    let mut attempt = Attempt {
        user,
        assistant: Some(assistant),
        evidence,
        legacy_output: false,
        development_evidence: false,
        reader_preparation: None,
    };
    assert!(attempt
        .dispatch_selected(workflow, 0, &ReaderHandoff::NextPage)
        .is_err());
    attempt.install_reader_preparation(preparation).unwrap();
    assert!(attempt.interpret(ledger, "changed input").is_err());
    assert!(attempt.generated(ledger, "changed plan".into()).is_err());
    assert!(attempt
        .outcome(
            ledger,
            true,
            Outcome::Completed,
            AttemptReason::SubmittedUnverified
        )
        .is_err());
    let result = attempt.dispatch_selected(workflow, 0, &ReaderHandoff::NextPage);
    assert_eq!(result.is_ok(), fresh);
    assert!(attempt
        .dispatch_selected(workflow, 0, &ReaderHandoff::NextPage)
        .is_err());
}
impl Attempt {
    pub fn prepare(ledger: &Ledger, acquired: AcquiredEvidence) -> Result<Self> {
        let timestamp = now()?;
        let conversation = Uuid::new_v4();
        let user = Turn {
            id: Uuid::new_v4(),
            conversation,
            exchange: Uuid::new_v4(),
            sequence: 0,
            role: Role::User,
            mode: Mode::Reader,
            outcome: Outcome::Prepared,
            text: None,
            sources: vec![],
            correction_of: None,
            created_ms: timestamp,
            updated_ms: timestamp,
            completion: None,
        };
        ledger.create(conversation, Uuid::new_v4(), timestamp)?;
        let expected = ledger.expected(Namespace::Conversation, conversation)?;
        let (evidence, legacy_output, development_evidence) = match acquired {
            AcquiredEvidence::Legacy { images } => {
                let images = images
                    .into_iter()
                    .enumerate()
                    .map(|(i, bytes)| -> Result<_> {
                        let format = image::guess_format(&bytes)?;
                        let mime = match format {
                            image::ImageFormat::Png => "image/png",
                            image::ImageFormat::Jpeg => "image/jpeg",
                            _ => anyhow::bail!("unsupported legacy image format"),
                        };
                        let dimensions =
                            image::ImageReader::with_format(std::io::Cursor::new(&bytes), format)
                                .into_dimensions()?;
                        Ok(LegacyImageInput {
                            descriptor: LegacyImage {
                                media: Media {
                                    sha256: digest(&bytes),
                                    bytes: bytes.len() as u64,
                                    media_type: mime.into(),
                                },
                                dimensions: [dimensions.0, dimensions.1],
                                role: if i == 0 {
                                    LegacyImageRole::Overview
                                } else {
                                    LegacyImageRole::Detail
                                },
                                provider_ordinal: Some(i.try_into()?),
                            },
                            bytes,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let prepared = ledger.prepare_legacy(
                    Uuid::new_v4(),
                    expected,
                    user.clone(),
                    Uuid::new_v4(),
                    None,
                    images,
                )?;
                (prepared.evidence.id, true, false)
            }
            AcquiredEvidence::Development(capture) => {
                let prepared = ledger.prepare_development(
                    Uuid::new_v4(),
                    expected,
                    user.clone(),
                    Uuid::new_v4(),
                    &capture,
                )?;
                (prepared.evidence.id, false, true)
            }
            AcquiredEvidence::Sdk(batch) => {
                let prepared =
                    ledger.prepare_sdk_fixture(Uuid::new_v4(), expected, user.clone(), &batch)?;
                (prepared.capture, false, false)
            }
        };
        let mut attempt = Self {
            user,
            assistant: None,
            evidence,
            legacy_output,
            development_evidence,
            reader_preparation: None,
        };
        attempt.user = attempt.load_turn(ledger, attempt.user.id)?;
        Ok(attempt)
    }
    /// A live context arrives only from the controlled new Pending branch.
    /// SDK fixture preparation supplies none and remains non-output.
    pub(super) fn install_reader_preparation(
        &mut self,
        preparation: ReaderPreparation,
    ) -> Result<()> {
        anyhow::ensure!(
            self.reader_preparation.is_none() && !self.legacy_output && !self.development_evidence,
            "Reader preparation already attached, legacy or development"
        );
        let assistant = self
            .assistant
            .as_ref()
            .context("Reader generated draft absent")?;
        anyhow::ensure!(
            assistant.outcome == Outcome::Generated,
            "Reader draft is not Generated"
        );
        match &preparation {
            ReaderPreparation::Fresh(context) => anyhow::ensure!(
                context.matches_attempt(self.user.conversation, assistant.id, self.evidence),
                "Reader context foreign to Attempt draft/evidence"
            ),
            ReaderPreparation::Historical(original) => anyhow::ensure!(
                original.receipt.acknowledgment.conversation == self.user.conversation
                    && original
                        .receipt
                        .admitted_intent
                        .as_ref()
                        .is_some_and(|intent| intent.turn == assistant.id),
                "Reader historical preparation foreign to Attempt"
            ),
        }
        self.reader_preparation = Some(preparation);
        Ok(())
    }
    pub(super) fn dispatch_selected(
        &self,
        workflow: &mut super::Workflow,
        ordinal: usize,
        handoff: &ReaderHandoff,
    ) -> Result<()> {
        let Some(ReaderPreparation::Fresh(context)) = &self.reader_preparation else {
            anyhow::bail!("Historical or unqualified Attempt cannot dispatch Reader output")
        };
        workflow.dispatch_reader(Some(context), ordinal, handoff)
    }
    fn load_turn(&self, ledger: &Ledger, id: Uuid) -> Result<Turn> {
        ledger
            .inspect(self.user.conversation, false)?
            .into_iter()
            .find_map(|r| match r {
                Record::Turn(t) if t.id == id => Some(t),
                _ => None,
            })
            .context("attempt turn absent")
    }
    pub fn images(&self, ledger: &Ledger) -> Result<Vec<Vec<u8>>> {
        if self.development_evidence {
            Ok(ledger.stored_development_images(self.evidence)?.images)
        } else if self.legacy_output {
            Ok(ledger.stored_legacy_images(self.evidence)?.images)
        } else {
            Ok(ledger
                .stored_sdk_images(self.evidence)?
                .images
                .into_iter()
                .map(|i| i.bytes)
                .collect())
        }
    }
    pub fn interpret(&mut self, ledger: &Ledger, text: &str) -> Result<()> {
        anyhow::ensure!(
            self.reader_preparation.is_none(),
            "Selected Reader preparation requires selected domain mutation/settlement"
        );
        let mut turn = self.user.clone();
        turn.text = Some(text.into());
        turn.outcome = Outcome::Interpreted;
        turn.updated_ms = now()?;
        ledger.advance(
            Uuid::new_v4(),
            ledger.expected(Namespace::Conversation, turn.conversation)?,
            ledger.expected(Namespace::Conversation, turn.id)?,
            turn.clone(),
        )?;
        self.user = turn;
        Ok(())
    }
    pub fn generated(&mut self, ledger: &Ledger, text: String) -> Result<()> {
        anyhow::ensure!(
            self.reader_preparation.is_none(),
            "Selected Reader preparation requires selected domain mutation/settlement"
        );
        let timestamp = now()?;
        let turn = Turn {
            id: Uuid::new_v4(),
            conversation: self.user.conversation,
            exchange: self.user.exchange,
            sequence: 0,
            role: Role::Assistant,
            mode: Mode::Reader,
            outcome: Outcome::Generated,
            text: Some(text),
            sources: self.user.sources.clone(),
            correction_of: None,
            created_ms: timestamp,
            updated_ms: timestamp,
            completion: None,
        };
        ledger.append(
            Uuid::new_v4(),
            ledger.expected(Namespace::Conversation, turn.conversation)?,
            turn.clone(),
            vec![],
        )?;
        self.assistant = Some(self.load_turn(ledger, turn.id)?);
        Ok(())
    }
    pub fn outcome(
        &mut self,
        ledger: &Ledger,
        assistant: bool,
        outcome: Outcome,
        reason: AttemptReason,
    ) -> Result<()> {
        anyhow::ensure!(
            self.reader_preparation.is_none(),
            "Selected Reader preparation requires selected domain mutation/settlement"
        );
        let original = if assistant {
            self.assistant.as_ref().context("draft absent")?
        } else {
            &self.user
        };
        let mut turn = original.clone();
        turn.outcome = outcome;
        turn.updated_ms = now()?;
        ledger.record_attempt_outcome(
            Uuid::new_v4(),
            ledger.expected(Namespace::Conversation, turn.conversation)?,
            ledger.expected(Namespace::Conversation, turn.id)?,
            turn.clone(),
            reason,
        )?;
        if assistant {
            self.assistant = Some(turn)
        } else {
            self.user = turn
        };
        Ok(())
    }
}

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
}
fn now() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()
        .context("timestamp overflow")
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
        let (evidence, legacy_output) = match acquired {
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
                (prepared.evidence.id, true)
            }
            AcquiredEvidence::Sdk(batch) => {
                let prepared =
                    ledger.prepare_sdk_fixture(Uuid::new_v4(), expected, user.clone(), &batch)?;
                (prepared.capture, false)
            }
        };
        let mut attempt = Self {
            user,
            assistant: None,
            evidence,
            legacy_output,
        };
        attempt.user = attempt.load_turn(ledger, attempt.user.id)?;
        Ok(attempt)
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
        if self.legacy_output {
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

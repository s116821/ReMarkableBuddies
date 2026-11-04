//! Current acknowledged data only. This state never supplies native permission.
use super::*;
use crate::storage::selection::SelectionToken;
use crate::storage::sync::mode::SyncModeLease;
use std::sync::Arc;

struct Frozen {
    accepted: SelectionToken,
    request: SelectedReaderWrite,
    acknowledged: Option<SelectedReaderAcknowledgment>,
}
pub(super) struct Selected {
    admission: Arc<SelectedAdmission>,
    token: SelectionToken,
    root: IntentRecordRef,
    user: IntentRecordRef,
    capture: Uuid,
    mode: Option<SyncModeLease>,
    interpretation: Option<Frozen>,
    generation: Option<Frozen>,
}
impl Selected {
    #[cfg(test)]
    pub(super) fn frozen_operation(&self, generated: bool) -> Uuid {
        let frozen = if generated {
            &self.generation
        } else {
            &self.interpretation
        };
        frozen.as_ref().unwrap().request.operation
    }
    pub(super) fn new(
        ledger: &Ledger,
        workflow: &super::super::Workflow,
        token: SelectionToken,
        root: IntentRecordRef,
        user: IntentRecordRef,
        capture: Uuid,
    ) -> Result<(Self, Turn)> {
        let admission = workflow.selected_admission()?;
        anyhow::ensure!(
            ledger.store().unavailable_commits()? == 0,
            "selected input Store incomplete"
        );
        anyhow::ensure!(
            admission.uses_store(ledger.store()),
            "selected Attempt has foreign Store"
        );
        let mode = admission.reader_mode_lease()?;
        let selected = Self {
            admission,
            token,
            root,
            user,
            capture,
            mode,
            interpretation: None,
            generation: None,
        };
        let turn = selected.current_user()?;
        anyhow::ensure!(
            turn.role == Role::User
                && turn.mode == Mode::Reader
                && turn.outcome == Outcome::Prepared
                && turn.text.is_none()
                && turn.sources == [capture],
            "selected input is not acknowledged Prepared capture"
        );
        selected.images()?;
        Ok((selected, turn))
    }
    fn mode(&self) -> Result<()> {
        if let Some(mode) = &self.mode {
            mode.require_shared_scope(self.token.scope())?;
        } else {
            anyhow::ensure!(
                self.admission.reader_mode_lease()?.is_none(),
                "unconfigured selected mode changed"
            );
        }
        Ok(())
    }
    fn current_user(&self) -> Result<Turn> {
        self.mode()?;
        self.admission.with_current(&self.token, |snapshot| {
            let projection = SelectedDomainProjection::from_snapshot(snapshot)?;
            projection.inspect(self.root.record_id)?;
            let Record::Root(root) = projection.exact_reference(&self.root)? else {
                anyhow::bail!("selected input root variant")
            };
            let Record::Turn(user) = projection.exact_reference(&self.user)? else {
                anyhow::bail!("selected input user variant")
            };
            for reference in [&self.root, &self.user] {
                let (head, Some(_)) = projection
                    .head(reference.namespace, reference.record_id)
                    .context("selected input head absent")?
                else {
                    anyhow::bail!("selected input head deleted")
                };
                anyhow::ensure!(
                    head.revision_id == reference.revision_id,
                    "selected input head replaced"
                );
            }
            anyhow::ensure!(
                user.conversation == root.id && user.sources == [self.capture],
                "selected input capture link differs"
            );
            Ok(user.clone())
        })
    }
    pub(super) fn check(&self) -> Result<()> {
        self.current_user().map(|_| ())
    }
    pub(super) fn images(&self) -> Result<Vec<Vec<u8>>> {
        self.check()?;
        let images = self.admission.capture_images(&self.token, self.capture)?;
        self.check()?;
        Ok(match images {
            SelectedCaptureImages::Sdk(images) => {
                images.images.into_iter().map(|image| image.bytes).collect()
            }
            SelectedCaptureImages::Development(images) => images.images,
            SelectedCaptureImages::Legacy(images) => images.images,
        })
    }
    pub(super) fn write(
        &mut self,
        text: &str,
        generated: bool,
    ) -> Result<SelectedReaderAcknowledgment> {
        let slot = if generated {
            &mut self.generation
        } else {
            &mut self.interpretation
        };
        if let Some(frozen) = slot.as_ref() {
            anyhow::ensure!(
                frozen.request.text == text,
                "selected Attempt mutation changed"
            );
            if let Some(acknowledgment) = frozen.acknowledged.clone() {
                self.check()?;
                return Ok(acknowledgment);
            }
        } else {
            *slot = Some(Frozen {
                accepted: self.token.clone(),
                request: SelectedReaderWrite {
                    operation: Uuid::new_v4(),
                    root: self.root.clone(),
                    user: self.user.clone(),
                    updated_ms: now()?,
                    text: text.into(),
                    kind: if generated {
                        SelectedReaderText::Generate {
                            assistant: Uuid::new_v4(),
                        }
                    } else {
                        SelectedReaderText::Interpret
                    },
                },
                acknowledged: None,
            });
        }
        let frozen = if generated {
            self.generation.as_ref()
        } else {
            self.interpretation.as_ref()
        }
        .unwrap();
        self.mode()?;
        let result = self
            .admission
            .publish_reader_text(&frozen.accepted, frozen.request.clone())?;
        let SelectedReaderPublication::Published {
            publication,
            acknowledgment,
        } = result
        else {
            anyhow::bail!(
                "historical selected acknowledgment cannot install current Attempt state"
            );
        };
        self.token = publication.token;
        self.root = acknowledgment.root.clone();
        if !generated {
            self.user = acknowledgment.turn.clone();
        }
        let slot = if generated {
            &mut self.generation
        } else {
            &mut self.interpretation
        };
        slot.as_mut().unwrap().acknowledged = Some((*acknowledgment).clone());
        self.check()?;
        Ok(*acknowledgment)
    }
}

//! Current acknowledged data only. This state never supplies native permission.
use super::*;
use crate::storage::selection::SelectionToken;
use crate::storage::sync::mode::SyncModeLease;
#[cfg(target_os = "linux")]
use crate::storage::sync::selected::{ReaderPhase, ReaderPreferences};
#[cfg(target_os = "linux")]
use crate::storage::Store;
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
    #[cfg(target_os = "linux")]
    resident: Option<ReaderPreferences>,
    #[cfg(target_os = "linux")]
    model_stage: std::cell::Cell<u8>,
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
        let mut selected = Self::input(ledger, workflow, token, root, user, capture)?;
        selected.mode = selected.admission.reader_mode_lease()?;
        let turn = selected.current_user()?;
        Self::require_prepared(&turn, capture)?;
        selected.images()?;
        Ok((selected, turn))
    }

    fn input(
        ledger: &Ledger,
        workflow: &super::super::Workflow,
        token: SelectionToken,
        root: IntentRecordRef,
        user: IntentRecordRef,
        capture: Uuid,
    ) -> Result<Self> {
        let admission = workflow.selected_admission()?;
        anyhow::ensure!(
            ledger.store().unavailable_commits()? == 0,
            "selected input Store incomplete"
        );
        anyhow::ensure!(
            admission.uses_store(ledger.store()),
            "selected Attempt has foreign Store"
        );
        Ok(Self {
            admission,
            token,
            root,
            user,
            capture,
            mode: None,
            interpretation: None,
            generation: None,
            #[cfg(target_os = "linux")]
            resident: None,
            #[cfg(target_os = "linux")]
            model_stage: std::cell::Cell::new(0),
        })
    }

    fn require_prepared(turn: &Turn, capture: Uuid) -> Result<()> {
        anyhow::ensure!(
            turn.role == Role::User
                && turn.mode == Mode::Reader
                && turn.outcome == Outcome::Prepared
                && turn.text.is_none()
                && turn.sources == [capture],
            "selected input is not acknowledged Prepared capture"
        );
        Ok(())
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
            self.project_user(&SelectedDomainProjection::from_snapshot(snapshot)?)
        })
    }

    fn project_user(&self, projection: &SelectedDomainProjection) -> Result<Turn> {
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
    }
    pub(super) fn check(&self) -> Result<()> {
        #[cfg(target_os = "linux")]
        if let Some(policy) = &self.resident {
            return self.with_resident(policy, ReaderPhase::Result, |_, _| Ok(()));
        }
        self.current_user().map(|_| ())
    }
    pub(super) fn document(&self) -> Result<Option<Uuid>> {
        let document = |snapshot: &crate::storage::selection::SelectedSnapshot| {
            let owners = SelectedDomainProjection::from_snapshot(snapshot)?.document_ownership()?;
            Ok(match owners.get(&self.root.record_id) {
                Some(DocumentOwnership::Document(document)) => Some(*document),
                _ => None,
            })
        };
        #[cfg(target_os = "linux")]
        if let Some(policy) = &self.resident {
            return self.with_resident(policy, ReaderPhase::Result, |store, reads| {
                let snapshot = store
                    .selected_snapshot_with_budget(
                        self.admission.scope(),
                        crate::storage::MAX_ITEMS,
                        reads,
                    )?
                    .context("selected Reader document unavailable")?
                    .snapshot;
                anyhow::ensure!(
                    snapshot.token == self.token,
                    "selected Reader document changed"
                );
                document(&snapshot)
            });
        }
        self.check()?;
        self.admission.with_current(&self.token, document)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn query_memory(
        &self,
        memory: &dyn crate::memory::Provider,
        query: &crate::memory::Query,
    ) -> Result<crate::memory::QueryResult> {
        let Some(policy) = &self.resident else {
            return memory.query(query);
        };
        let reserved = {
            let mut reads = policy.reads(ReaderPhase::Result)?;
            crate::memory::selected::reserve_reader_query(&mut reads)?
        };
        memory.query_selected_reader(query, reserved)
    }
    pub(super) fn images(&self) -> Result<Vec<Vec<u8>>> {
        let images = self.capture_images()?;
        Ok(match images {
            SelectedCaptureImages::Sdk(images) => {
                images.images.into_iter().map(|image| image.bytes).collect()
            }
            SelectedCaptureImages::Development(images) => images.images,
            SelectedCaptureImages::Legacy(images) => images.images,
            SelectedCaptureImages::Resident(images) => images.images,
        })
    }

    fn capture_images(&self) -> Result<SelectedCaptureImages> {
        #[cfg(target_os = "linux")]
        if let Some(policy) = &self.resident {
            return self.with_resident(policy, ReaderPhase::Result, |store, reads| {
                SelectedAdmission::capture_images_locked(store, &self.token, self.capture, reads)
            });
        }
        self.check()?;
        let images = self.admission.capture_images(&self.token, self.capture)?;
        self.check()?;
        Ok(images)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn new_resident(
        ledger: &Ledger,
        workflow: &super::super::Workflow,
        token: SelectionToken,
        acknowledgment: crate::conversation::selected_reader_prepare::ResidentPreparedAcknowledgment,
        policy: ReaderPreferences,
    ) -> Result<(Self, Turn)> {
        let mut selected = Self::input(
            ledger,
            workflow,
            token,
            acknowledgment.root,
            acknowledgment.user,
            acknowledgment.capture.record_id,
        )?;
        let turn = selected.with_resident(&policy, ReaderPhase::Acquisition, |store, reads| {
            let snapshot = store
                .selected_snapshot_with_budget(
                    selected.admission.scope(),
                    crate::storage::MAX_ITEMS,
                    reads,
                )?
                .context("selected Reader Prepared input absent")?
                .snapshot;
            anyhow::ensure!(
                snapshot.token == selected.token,
                "selected Reader Prepared input changed"
            );
            let projection = SelectedDomainProjection::from_snapshot(&snapshot)?;
            anyhow::ensure!(
                projection.current_reference(acknowledgment.capture.namespace, selected.capture)?
                    == acknowledgment.capture
                    && matches!(
                        projection.exact_reference(&acknowledgment.capture)?,
                        Record::ResidentCapture(_)
                    ),
                "selected Reader Prepared source differs"
            );
            let turn = selected.project_user(&projection)?;
            Self::require_prepared(&turn, selected.capture)?;
            Ok(turn)
        })?;
        selected.resident = Some(policy);
        Ok((selected, turn))
    }

    #[cfg(target_os = "linux")]
    pub(super) fn is_resident(&self) -> bool {
        self.resident.is_some()
    }

    #[cfg(target_os = "linux")]
    fn with_resident<T>(
        &self,
        policy: &ReaderPreferences,
        phase: ReaderPhase,
        consume: impl FnOnce(&Store, &mut crate::storage::selection::SelectedReadBudget) -> Result<T>,
    ) -> Result<T> {
        let mut reads = policy.reads(phase)?;
        let [catalog, preference] = policy.gates();
        SelectedAdmission::with_gates(&[self.admission.as_ref(), catalog, preference], |store| {
            policy.verify_locked(store, &self.admission, &self.token, &mut reads)?;
            policy.require_phase_time(phase)?;
            consume(store, &mut reads)
        })
    }

    #[cfg(target_os = "linux")]
    pub(super) fn progress(&self) -> Result<()> {
        if let Some(policy) = &self.resident {
            return policy.check_progress(&self.admission, &self.token);
        }
        self.check()
    }

    #[cfg(target_os = "linux")]
    pub(super) fn execute_model(
        &self,
        engine: &mut dyn crate::LLMEngine,
        progress: &mut dyn FnMut() -> Result<()>,
        memory: Option<(&dyn crate::memory::Provider, &crate::memory::QueryResult)>,
    ) -> Result<String> {
        let Some(policy) = &self.resident else {
            return engine.execute_with_progress(progress);
        };
        let stage = self.model_stage.get();
        anyhow::ensure!(stage < 3, "selected Reader model allowance consumed");
        let mut submitted = false;
        let result = engine.execute_with_deadline(
            progress,
            &mut |submit| {
                anyhow::ensure!(
                    self.model_stage.get() == stage,
                    "selected Reader handoff already consumed"
                );
                self.model_stage.set(stage + 1);
                let mut submit_once = || {
                    anyhow::ensure!(!submitted, "selected Reader model already submitted");
                    if stage == 0 {
                        policy.require_acquisition_time()?;
                    }
                    anyhow::ensure!(
                        std::time::Instant::now() < policy.deadline(),
                        "Reader request expired"
                    );
                    policy.start_model_wait()?;
                    submit()?;
                    submitted = true;
                    Ok(())
                };
                if let Some((provider, view)) = memory {
                    let mut reads = policy.reads(ReaderPhase::Submission)?;
                    provider.with_selected_reader_handoff(
                        view,
                        &self.admission,
                        &policy.gates(),
                        &mut |store| {
                            policy.verify_locked(store, &self.admission, &self.token, &mut reads)
                        },
                        &mut submit_once,
                    )
                } else {
                    self.with_resident(policy, ReaderPhase::Submission, |_, _| submit_once())
                }
            },
            policy.deadline(),
        );
        let value = result?;
        anyhow::ensure!(
            submitted,
            "selected Reader result has no guarded submission"
        );
        policy.finish_model_wait();
        Ok(value)
    }
    pub(super) fn write(
        &mut self,
        text: &str,
        generated: bool,
        memory: Option<(&dyn crate::memory::Provider, &crate::memory::QueryResult)>,
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
        #[cfg(target_os = "linux")]
        let guarded = if let Some(policy) = &self.resident {
            anyhow::ensure!(
                !self
                    .admission
                    .recover_reader_text(&frozen.accepted, &frozen.request)?,
                "selected Reader text is already historical"
            );
            anyhow::ensure!(
                self.model_stage.get() >= 2,
                "selected Reader verification not submitted"
            );
            let mut reads = policy.reads(ReaderPhase::Result)?;
            let mut verify =
                |store: &Store, reads: &mut crate::storage::selection::SelectedReadBudget| {
                    policy.verify_locked(store, &self.admission, &frozen.accepted, reads)?;
                    policy.require_result_time()
                };
            let gates = policy.gates();
            Some(match memory {
                Some((provider, view)) => provider.publish_selected_reader_text_joined(
                    view,
                    &self.admission,
                    &frozen.accepted,
                    frozen.request.clone(),
                    (&gates, &mut reads),
                    &mut verify,
                )?,
                None => self.admission.publish_resident_reader_text_joined(
                    &frozen.accepted,
                    frozen.request.clone(),
                    &gates,
                    &mut reads,
                    verify,
                )?,
            })
        } else {
            None
        };
        #[cfg(not(target_os = "linux"))]
        let guarded = None;
        let result = if let Some(result) = guarded {
            result
        } else {
            self.mode()?;
            match memory {
                Some((provider, view)) => provider.publish_selected_reader_text(
                    view,
                    &self.admission,
                    &frozen.accepted,
                    frozen.request.clone(),
                )?,
                None => self
                    .admission
                    .publish_reader_text(&frozen.accepted, frozen.request.clone())?,
            }
        };
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

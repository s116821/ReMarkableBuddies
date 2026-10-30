//! Read-only input ownership for one resident request, including activation.
//! This guard has no page identity, output capability or input-injection method.
use super::{input_observer::InputObserver, touch::TriggerCorner};
use anyhow::{ensure, Result};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputProgress {
    Quiet,
    Canceled,
}

pub struct RequestInput {
    observer: InputObserver,
    corner: TriggerCorner,
    owned_sysfs: Option<PathBuf>,
    canceled: bool,
    lost: bool,
}

impl RequestInput {
    /// Create only after the initiating gesture is released. The first poll
    /// still checks seeded contacts and all activity since descriptor creation.
    pub fn new(corner: TriggerCorner, owned_sysfs: Option<&Path>) -> Result<Self> {
        Ok(Self {
            observer: InputObserver::new(corner.clone(), owned_sysfs)?,
            corner,
            owned_sysfs: owned_sysfs.map(Path::to_path_buf),
            canceled: false,
            lost: false,
        })
    }

    pub fn poll(&mut self) -> Result<InputProgress> {
        ensure!(!self.lost, "Resident request input integrity lost");
        // Do not short-circuit after cancellation: a later observer error still
        // means integrity loss, not another ordinary canceled request.
        let events = match self.observer.poll() {
            Ok(events) => events,
            Err(error) => {
                self.lost = true;
                return Err(error);
            }
        };
        self.canceled |= !events.is_empty() || !self.observer.quiescent();
        Ok(if self.canceled {
            InputProgress::Canceled
        } else {
            InputProgress::Quiet
        })
    }

    pub(crate) fn require_quiet(&mut self) -> Result<()> {
        ensure!(
            self.poll()? == InputProgress::Quiet,
            "Input canceled resident activation"
        );
        Ok(())
    }

    /// Renew descriptors after activation without an unobserved input gap.
    /// The request's cancellation and integrity state cannot be reset here.
    pub(crate) fn reacquire(&mut self) -> Result<InputProgress> {
        let corner = self.corner.clone();
        let owned = self.owned_sysfs.clone();
        self.reacquire_with(|| InputObserver::new(corner, owned.as_deref()))
    }

    fn reacquire_with(
        &mut self,
        create: impl FnOnce() -> Result<InputObserver>,
    ) -> Result<InputProgress> {
        let result = (|| {
            if self.poll()? == InputProgress::Canceled {
                return Ok(InputProgress::Canceled);
            }
            let mut fresh = create()?;
            let prior = self.poll()?;
            let events = fresh.poll()?;
            self.canceled |=
                prior == InputProgress::Canceled || !events.is_empty() || !fresh.quiescent();
            self.observer = fresh;
            Ok(if self.canceled {
                InputProgress::Canceled
            } else {
                InputProgress::Quiet
            })
        })();
        self.lost |= result.is_err();
        result
    }

    #[cfg(test)]
    pub(crate) fn scripted(polls: Vec<Result<Vec<super::interaction::Interaction>>>) -> Self {
        Self {
            observer: InputObserver::scripted(polls),
            corner: TriggerCorner::LowerLeft,
            owned_sysfs: None,
            canceled: false,
            lost: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::interaction::Interaction;

    fn quiet() -> Result<Vec<Interaction>> {
        Ok(vec![])
    }

    #[test]
    fn cancellation_is_sticky_but_does_not_mask_later_observer_loss() {
        for interaction in [
            Interaction::Reader,
            Interaction::Undo,
            Interaction::Redo,
            Interaction::Invalidated,
        ] {
            let mut guard = RequestInput::scripted(vec![
                Ok(vec![interaction]),
                quiet(),
                Err(anyhow::anyhow!("lost observer")),
            ]);
            assert_eq!(guard.poll().unwrap(), InputProgress::Canceled);
            assert_eq!(guard.poll().unwrap(), InputProgress::Canceled);
            assert!(guard.poll().is_err());
            assert!(guard.poll().is_err());
        }
    }

    #[test]
    fn unfinished_contact_cancels_without_waiting_for_a_gesture() {
        let mut guard = RequestInput::scripted(vec![]);
        guard.observer = InputObserver::replay_request(vec![(
            true,
            vec![(3, 47, 0), (3, 57, 1), (3, 53, 80), (3, 54, 90)],
        )]);
        assert_eq!(guard.poll().unwrap(), InputProgress::Canceled);
        assert_eq!(guard.poll().unwrap(), InputProgress::Canceled);
    }

    #[test]
    fn dropped_raw_input_is_integrity_loss() {
        let mut guard = RequestInput::scripted(vec![]);
        guard.observer = InputObserver::replay_request(vec![(true, vec![(0, 3, 0)])]);
        assert!(guard.poll().is_err());
        assert!(guard.poll().is_err());
    }

    #[test]
    fn replacement_overlaps_old_and_new_observers_and_keeps_cancellation() {
        for source in ["quiet", "old", "new"] {
            let mut guard = RequestInput::scripted(vec![
                quiet(),
                Ok(if source == "old" {
                    vec![Interaction::Invalidated]
                } else {
                    vec![]
                }),
            ]);
            let observed = guard
                .reacquire_with(|| {
                    Ok(InputObserver::scripted(vec![
                        Ok(if source == "new" {
                            vec![Interaction::Invalidated]
                        } else {
                            vec![]
                        }),
                        quiet(),
                    ]))
                })
                .unwrap();
            let expected = if source == "quiet" {
                InputProgress::Quiet
            } else {
                InputProgress::Canceled
            };
            assert_eq!(observed, expected);
            assert_eq!(guard.poll().unwrap(), expected);
        }
    }

    #[test]
    fn replacement_failure_permanently_invalidates_guard_without_retry() {
        for source in ["old-before", "create", "old-after", "new"] {
            let mut guard = RequestInput::scripted(vec![
                if source == "old-before" {
                    Err(anyhow::anyhow!("old-before"))
                } else {
                    quiet()
                },
                if source == "old-after" {
                    Err(anyhow::anyhow!("old-after"))
                } else {
                    quiet()
                },
            ]);
            assert!(guard
                .reacquire_with(|| {
                    ensure!(source != "create", "creation failed");
                    Ok(InputObserver::scripted(vec![if source == "new" {
                        Err(anyhow::anyhow!("new"))
                    } else {
                        quiet()
                    }]))
                })
                .is_err());
            assert!(guard.poll().is_err());
            assert!(guard.reacquire_with(|| panic!("must not retry")).is_err());
        }
    }

    #[test]
    fn canceled_guard_cannot_renew_into_a_new_request() {
        let mut guard = RequestInput::scripted(vec![Ok(vec![Interaction::Reader]), quiet()]);
        assert_eq!(guard.poll().unwrap(), InputProgress::Canceled);
        assert_eq!(
            guard.reacquire_with(|| panic!("canceled renewal")).unwrap(),
            InputProgress::Canceled
        );
    }
}

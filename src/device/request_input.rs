//! Read-only input ownership for one resident request, including activation.
//! This guard has no page identity, output capability or input-injection method.
use super::interaction::Interaction;
use super::{input_observer::InputObserver, touch::TriggerCorner};
use anyhow::{ensure, Result};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputProgress {
    Quiet,
    Canceled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResidentGesture {
    Reader,
    BlankStart,
}

/// A genuine resident gesture release plus overlapping request input ownership.
/// No public constructor accepts a boolean or legacy held-contact trigger.
pub struct ReleasedReaderInput {
    pub(crate) input: RequestInput,
    pub(crate) released_at: Instant,
    pub(crate) gesture: ResidentGesture,
}

pub struct RequestInput {
    observer: InputObserver,
    corner: TriggerCorner,
    owned_sysfs: Option<PathBuf>,
    canceled: bool,
    lost: bool,
}

impl RequestInput {
    pub(crate) fn wait_released_reader(
        corner: TriggerCorner,
        blank_start: bool,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<ReleasedReaderInput> {
        let mut input = InputObserver::new(corner.clone(), None)?;
        if blank_start {
            input.enable_blank_start();
        }
        Self::wait_released_with(
            &mut input,
            corner.clone(),
            progress,
            || InputObserver::new(corner.clone(), None),
            || std::thread::sleep(Duration::from_millis(10)),
        )
    }
    fn wait_released_with(
        prior: &mut InputObserver,
        corner: TriggerCorner,
        progress: &mut dyn FnMut() -> Result<()>,
        mut fresh: impl FnMut() -> Result<InputObserver>,
        mut pace: impl FnMut(),
    ) -> Result<ReleasedReaderInput> {
        loop {
            progress()?;
            let events = prior.poll()?;
            progress()?;
            if let Some(event) = events
                .iter()
                .copied()
                .find(|event| matches!(event, Interaction::Reader | Interaction::BlankStart))
            {
                let released_at = Instant::now();
                ensure!(
                    events == [event] && prior.quiescent(),
                    "Resident trigger lacks a quiet complete release"
                );
                let mut next = fresh()?;
                ensure!(
                    prior.poll()?.is_empty() && prior.quiescent(),
                    "Input changed during resident release handoff"
                );
                ensure!(
                    next.poll()?.is_empty() && next.quiescent(),
                    "New resident input observer not quiet"
                );
                progress()?;
                return Ok(ReleasedReaderInput {
                    released_at,
                    gesture: if event == Interaction::BlankStart {
                        ResidentGesture::BlankStart
                    } else {
                        ResidentGesture::Reader
                    },
                    input: Self {
                        observer: next,
                        corner,
                        owned_sysfs: None,
                        canceled: false,
                        lost: false,
                    },
                });
            }
            // Idle input is not a request. It cannot authorize activation or
            // manufacture release; loss is fatal rather than a Touch fallback.
            pace();
        }
    }
    /// Construct a guard for a released request or explicitly selected activation.
    /// The first poll checks seeded contacts and activity since descriptor creation.
    pub fn new(corner: TriggerCorner, owned_sysfs: Option<&Path>) -> Result<Self> {
        Ok(Self {
            observer: InputObserver::new(corner.clone(), owned_sysfs)?,
            corner,
            owned_sysfs: owned_sysfs.map(Path::to_path_buf),
            canceled: false,
            lost: false,
        })
    }

    /// Explicit development output excludes only its actual owned virtual keyboard,
    /// with overlapping old/new observers. Cancellation/loss is never renewed.
    #[cfg(feature = "development-resident-bootstrap")]
    pub(crate) fn bind_owned_keyboard(&mut self, path: &Path) -> Result<()> {
        self.require_quiet()?;
        let mut next = InputObserver::new(self.corner.clone(), Some(path))?;
        self.require_quiet()?;
        ensure!(
            next.poll()?.is_empty() && next.quiescent(),
            "input changed during owned keyboard handoff"
        );
        self.observer = next;
        self.owned_sysfs = Some(path.to_path_buf());
        Ok(())
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
    #[test]
    fn resident_release_handoff_requires_quiet_overlap_without_touch_fallback() {
        use std::cell::Cell;
        for fault in ["none", "held", "old", "new", "loss"] {
            let events = if fault == "loss" {
                vec![Err(anyhow::anyhow!("lost observer"))]
            } else {
                vec![
                    Ok(vec![Interaction::Reader]),
                    if fault == "old" {
                        Ok(vec![Interaction::Invalidated])
                    } else {
                        quiet()
                    },
                ]
            };
            let mut prior = InputObserver::scripted(events);
            if fault == "held" {
                prior = InputObserver::replay_request(vec![(
                    true,
                    vec![(3, 47, 0), (3, 57, 1), (3, 53, 80), (3, 54, 90)],
                )]);
            }
            let polls = Cell::new(0);
            let result = RequestInput::wait_released_with(
                &mut prior,
                TriggerCorner::LowerLeft,
                &mut || {
                    polls.set(polls.get() + 1);
                    ensure!(polls.get() < 20, "bounded fixture idle");
                    Ok(())
                },
                || {
                    Ok(InputObserver::scripted(vec![
                        if fault == "new" {
                            Ok(vec![Interaction::Invalidated])
                        } else {
                            quiet()
                        },
                        quiet(),
                        quiet(),
                    ]))
                },
                || {},
            );
            if fault == "none" {
                assert_eq!(result.unwrap().input.poll().unwrap(), InputProgress::Quiet);
            } else {
                assert!(result.is_err(), "{fault}");
            }
        }
    }
    #[test]
    fn resident_release_refuses_seeded_and_dropped_raw_input() {
        for dropped in [false, true] {
            let raw = if dropped {
                vec![(0, 3, 0)]
            } else {
                vec![(3, 47, 0), (3, 57, 1), (3, 53, 80), (3, 54, 90), (0, 0, 0)]
            };
            let mut prior = if dropped {
                InputObserver::replay_request(vec![(true, raw)])
            } else {
                InputObserver::replay_seeded_release()
            };
            let mut polls = 0;
            let mut opens = 0;
            assert!(RequestInput::wait_released_with(
                &mut prior,
                TriggerCorner::LowerLeft,
                &mut || {
                    polls += 1;
                    ensure!(polls < 10, "bounded raw input fixture");
                    Ok(())
                },
                || {
                    opens += 1;
                    Ok(InputObserver::scripted(vec![]))
                },
                || {}
            )
            .is_err());
            assert_eq!(opens, 0);
        }
    }
}

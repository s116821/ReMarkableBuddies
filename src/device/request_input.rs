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
    pub(crate) released_boot_second: Option<u64>,
    pub(crate) gesture: ResidentGesture,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Interruption {
    ObservedAction,
    NonquietContact,
    InputLost,
}

impl Interruption {
    fn label(self) -> &'static str {
        match self {
            Self::ObservedAction => "observed-action",
            Self::NonquietContact => "nonquiet-contact",
            Self::InputLost => "input-loss",
        }
    }
}

pub struct RequestInput {
    observer: InputObserver,
    corner: TriggerCorner,
    owned_sysfs: Option<PathBuf>,
    first_interruption: Option<Interruption>,
    lost: bool,
}

impl RequestInput {
    pub(crate) fn wait_released_reader(
        corner: TriggerCorner,
        blank_start: bool,
        timestamp_release: bool,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<ReleasedReaderInput> {
        Self::wait_for_gesture_with(
            corner.clone(),
            blank_start,
            timestamp_release,
            progress,
            |clock| InputObserver::open(corner.clone(), None, clock),
            || std::thread::sleep(Duration::from_millis(10)),
        )
    }

    pub(super) fn wait_for_gesture_with(
        corner: TriggerCorner,
        blank_start: bool,
        timestamp_release: bool,
        progress: &mut dyn FnMut() -> Result<()>,
        mut open: impl FnMut(bool) -> Result<InputObserver>,
        mut pace: impl FnMut(),
    ) -> Result<ReleasedReaderInput> {
        let mut use_clock = timestamp_release;
        loop {
            let mut input = open(use_clock)?;
            if blank_start {
                input.enable_blank_start();
            }
            let result = Self::wait_released_with(
                &mut input,
                corner.clone(),
                progress,
                || open(false),
                &mut pace,
            );
            if !use_clock || !input.lost_initial_clock_frame() {
                return result;
            }
            // No gesture was ever observed on this client. Drop it completely
            // and reseed once on a fresh descriptor for the next gesture.
            use_clock = false;
        }
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
            if let Some(event) = events
                .iter()
                .copied()
                .find(|event| matches!(event, Interaction::Reader | Interaction::BlankStart))
            {
                let released_at = Instant::now();
                let released_boot_second = prior.reader_release_second();
                progress()?;
                ensure!(
                    events == [event] && prior.quiescent(),
                    "Resident trigger lacks a quiet complete release"
                );
                let mut next = fresh()?;
                ensure!(
                    prior.poll_request()?.is_empty() && prior.quiescent(),
                    "Input changed during resident release handoff"
                );
                ensure!(
                    next.poll_request()?.is_empty() && next.quiescent(),
                    "New resident input observer not quiet"
                );
                progress()?;
                return Ok(ReleasedReaderInput {
                    released_at,
                    released_boot_second,
                    gesture: if event == Interaction::BlankStart {
                        ResidentGesture::BlankStart
                    } else {
                        ResidentGesture::Reader
                    },
                    input: Self {
                        observer: next,
                        corner,
                        owned_sysfs: None,
                        first_interruption: None,
                        lost: false,
                    },
                });
            }
            // Idle input is not a request. It cannot authorize activation or
            // manufacture release; loss is fatal rather than a Touch fallback.
            progress()?;
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
            first_interruption: None,
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
        let events = match next.poll_request() {
            Ok(events) => events,
            Err(error) => {
                self.record_interruption(Interruption::InputLost);
                self.lost = true;
                return Err(error);
            }
        };
        ensure!(
            self.observe_activity(&events, next.quiescent()),
            "input changed during owned keyboard handoff"
        );
        self.observer = next;
        self.owned_sysfs = Some(path.to_path_buf());
        Ok(())
    }

    pub fn poll(&mut self) -> Result<InputProgress> {
        // Poll even after cancellation so a later observer failure stays fatal.
        self.sample_quiet()?;
        Ok(self.progress_state())
    }

    // Read-only startup can wait on an abandoned guard without renewing its
    // authority. Sampling never clears sticky cancellation or input loss.
    pub(crate) fn sample_quiet(&mut self) -> Result<bool> {
        ensure!(!self.lost, "Resident request input integrity lost");
        let events = match self.observer.poll_request() {
            Ok(events) => events,
            Err(error) => {
                self.record_interruption(Interruption::InputLost);
                self.lost = true;
                return Err(error);
            }
        };
        Ok(self.observe_activity(&events, self.observer.quiescent()))
    }

    fn progress_state(&self) -> InputProgress {
        if self.first_interruption.is_some() {
            InputProgress::Canceled
        } else {
            InputProgress::Quiet
        }
    }

    fn observe_activity(&mut self, events: &[Interaction], quiescent: bool) -> bool {
        if !events.is_empty() {
            self.record_interruption(Interruption::ObservedAction);
        } else if !quiescent {
            self.record_interruption(Interruption::NonquietContact);
        }
        events.is_empty() && quiescent
    }

    fn record_interruption(&mut self, reason: Interruption) {
        if self.first_interruption.is_none() {
            self.first_interruption = Some(reason);
            log::info!(
                "Resident request input interrupted reason={}",
                reason.label()
            );
        }
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
            self.poll()?;
            let events = fresh.poll_request()?;
            self.observe_activity(&events, fresh.quiescent());
            self.observer = fresh;
            Ok(self.progress_state())
        })();
        if result.is_err() {
            self.record_interruption(Interruption::InputLost);
            self.lost = true;
        }
        result
    }

    #[cfg(test)]
    pub(crate) fn scripted(polls: Vec<Result<Vec<super::interaction::Interaction>>>) -> Self {
        Self {
            observer: InputObserver::scripted(polls),
            corner: TriggerCorner::LowerLeft,
            owned_sysfs: None,
            first_interruption: None,
            lost: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn replay(batches: Vec<(bool, Vec<super::owned_pen_window::Event>)>) -> Self {
        let mut guard = Self::scripted(vec![]);
        guard.observer = InputObserver::replay_request(batches);
        guard
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
            for sampled in [false, true] {
                let mut guard = RequestInput::scripted(vec![
                    Ok(vec![interaction]),
                    quiet(),
                    quiet(),
                    Err(anyhow::anyhow!("lost observer")),
                ]);
                if sampled {
                    assert!(!guard.sample_quiet().unwrap());
                    assert!(guard.sample_quiet().unwrap());
                } else {
                    assert_eq!(guard.poll().unwrap(), InputProgress::Canceled);
                    assert_eq!(guard.poll().unwrap(), InputProgress::Canceled);
                }
                assert_eq!(guard.poll().unwrap(), InputProgress::Canceled);
                assert!(guard.poll().is_err());
                assert!(guard.poll().is_err());
                assert_eq!(guard.first_interruption, Some(Interruption::ObservedAction));
            }
        }
    }

    #[test]
    fn passive_device_packets_do_not_cancel_resident_input() {
        use evdev::{EventType, MiscCode};

        let timestamp = (EventType::MISC.0, MiscCode::MSC_TIMESTAMP.0, 17);
        let mut unfiltered =
            InputObserver::replay_request(vec![(false, vec![timestamp, (0, 0, 0)])]);
        assert_eq!(unfiltered.poll().unwrap(), [Interaction::Invalidated]);

        let mut guard = RequestInput::replay(vec![
            (true, vec![timestamp]), // No SYN_REPORT has arrived on this descriptor.
            (
                false,
                vec![
                    timestamp,
                    (EventType::LED.0, 0, 1),
                    (EventType::SOUND.0, 1, 0),
                    (EventType::REPEAT.0, 0, 100),
                    (0, 0, 0),
                ],
            ),
        ]);
        assert_eq!(guard.poll().unwrap(), InputProgress::Quiet);
        assert_eq!(guard.first_interruption, None);

        // The same packets cannot hide the genuine key event in their batch.
        let mut guard = RequestInput::replay(vec![(
            false,
            vec![
                timestamp,
                (EventType::KEY.0, 28, 1),
                (EventType::LED.0, 0, 1),
                (0, 0, 0),
            ],
        )]);
        assert_eq!(guard.poll().unwrap(), InputProgress::Canceled);
        assert_eq!(guard.first_interruption, Some(Interruption::ObservedAction));
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
        assert_eq!(
            guard.first_interruption,
            Some(Interruption::NonquietContact)
        );
    }

    #[test]
    fn dropped_raw_input_is_integrity_loss() {
        for touch in [true, false] {
            let mut guard = RequestInput::replay(vec![(touch, vec![(0, 3, 0)])]);
            assert!(guard.poll().is_err());
            assert!(guard.poll().is_err());
            assert_eq!(guard.first_interruption, Some(Interruption::InputLost));
        }
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

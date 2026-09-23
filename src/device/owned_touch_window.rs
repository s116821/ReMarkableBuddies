//! One exact outside-panel tap. This policy never authorizes menu-item input.
use super::{
    contact_frames::{ContactFrames, Observation},
    owned_pen_window::{WindowIo, MAX_EVENTS},
};
use anyhow::{ensure, Result};
use std::time::Duration;

pub(super) trait TouchWindowIo: WindowIo {
    fn commit_decoder(&mut self, decoder: &ContactFrames);
}

pub(super) fn finish(
    io: &mut impl TouchWindowIo,
    started: Duration,
    point: (i32, i32),
    decoder: &mut ContactFrames,
) -> Result<()> {
    let drain_started = io.now();
    let deadline = |io: &dyn WindowIo| -> Result<()> {
        ensure!(
            io.now().saturating_sub(started) < Duration::from_secs(1),
            "Owned touch window exceeded one second"
        );
        ensure!(
            io.now().saturating_sub(drain_started) < Duration::from_millis(50),
            "Owned touch drain exceeded 50ms"
        );
        Ok(())
    };
    ensure!(
        decoder.ready_for_timer()
            && decoder
                .contacts()
                .is_some_and(|contacts| contacts.is_empty()),
        "Touch window must start released"
    );
    deadline(io)?;
    io.validate_source()?;
    let (mut count, mut frames) = (0usize, 0usize);
    let (mut starts, mut stops) = (0usize, 0usize);
    let (mut down, mut released) = (false, false);
    loop {
        deadline(io)?;
        let events = io.next_events()?;
        deadline(io)?;
        let Some(events) = events else { break };
        ensure!(!events.is_empty(), "Empty nonterminal touch batch");
        count = count
            .checked_add(events.len())
            .ok_or_else(|| anyhow::anyhow!("Touch event count overflow"))?;
        ensure!(count <= MAX_EVENTS, "Owned touch event limit");
        for (kind, code, value) in events {
            let expected = match (kind, code, value) {
                (0, 0, 0)
                | (3, 47, 0)
                | (3, 57, -1 | 1)
                | (3, 58, 100)
                | (3, 48 | 49, 17)
                | (3, 52, 4) => true,
                (3, 53, x) => x == point.0,
                (3, 54, y) => y == point.1,
                _ => false,
            };
            ensure!(
                expected,
                "Unexpected owned touch event kind={kind} code={code}"
            );
            if (kind, code) == (3, 57) {
                if value == 1 {
                    starts += 1;
                } else {
                    stops += 1;
                }
                ensure!(
                    starts == 1 && stops <= 1,
                    "Repeated or unstarted owned touch"
                );
            }
            match decoder.feed(kind, code, value) {
                Observation::Lost => anyhow::bail!("Owned touch frame lost"),
                Observation::Pending => {}
                Observation::Frame(contacts) => {
                    frames += 1;
                    if contacts.is_empty() {
                        if down {
                            released = true;
                        }
                    } else {
                        ensure!(
                            !released && contacts.len() == 1,
                            "Repeated or multiple owned contacts"
                        );
                        let contact = contacts[0];
                        ensure!(
                            contact.slot == 0
                                && contact.tracking == 1
                                && (contact.x, contact.y) == point,
                            "Owned touch left its exact point"
                        );
                        down = true;
                    }
                }
            }
        }
    }
    ensure!(
        starts == 1
            && stops == 1
            && down
            && released
            && frames >= 2
            && decoder.ready_for_timer()
            && decoder
                .contacts()
                .is_some_and(|contacts| contacts.is_empty()),
        "Owned touch lacks complete positive down/release delivery"
    );
    io.released_snapshot()?;
    io.validate_source()?;
    io.commit_decoder(decoder);
    io.check_other_input()?;
    deadline(io)?;
    log::debug!("Owned touch event drain observed events={count} frames={frames}");
    deadline(io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{contact_frames::Slot, owned_pen_window::Event};
    use std::collections::VecDeque;
    const POINT: (i32, i32) = (702, 1);
    fn decoder() -> ContactFrames {
        ContactFrames::seeded(
            vec![Slot {
                tracking: None,
                x: Some(POINT.0),
                y: Some(POINT.1),
            }],
            0,
        )
        .unwrap()
    }
    fn tap() -> Vec<Event> {
        vec![
            (3, 47, 0),
            (3, 57, 1),
            (3, 53, 702),
            (3, 54, 1),
            (3, 58, 100),
            (3, 48, 17),
            (3, 49, 17),
            (3, 52, 4),
            (0, 0, 0),
            (3, 47, 0),
            (3, 57, -1),
            (0, 0, 0),
        ]
    }
    struct Io {
        batches: VecDeque<Vec<Event>>,
        now: Duration,
        cost: Duration,
        external: bool,
        held: bool,
        wrong: bool,
    }
    impl WindowIo for Io {
        fn now(&self) -> Duration {
            self.now
        }
        fn validate_source(&mut self) -> Result<()> {
            ensure!(!self.wrong, "identity");
            Ok(())
        }
        fn next_events(&mut self) -> Result<Option<Vec<Event>>> {
            self.now += self.cost;
            Ok(self.batches.pop_front())
        }
        fn released_snapshot(&mut self) -> Result<()> {
            ensure!(!self.held, "held");
            Ok(())
        }
        fn check_other_input(&mut self) -> Result<()> {
            ensure!(!self.external, "external");
            Ok(())
        }
    }
    impl TouchWindowIo for Io {
        fn commit_decoder(&mut self, _: &ContactFrames) {}
    }
    fn io(events: Vec<Event>) -> Io {
        Io {
            batches: vec![events].into(),
            now: Duration::from_millis(102),
            cost: Duration::ZERO,
            external: false,
            held: false,
            wrong: false,
        }
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn linux_adapter_keeps_completed_other_input_and_latches_raw_touch_failure() {
        use crate::device::input_observer::InputObserver;
        let mut quiet = InputObserver::replay_owned_touch(vec![], tap());
        assert!(quiet.finish_owned_touch().is_ok());
        assert!(quiet.quiescent());
        for batches in [
            vec![(false, vec![(1, 30, 1), (0, 0, 0), (1, 30, 0), (0, 0, 0)])],
            vec![(true, vec![(3, 57, 9), (0, 0, 0), (3, 57, -1), (0, 0, 0)])],
            vec![(false, vec![(1, 320, 1), (0, 0, 0), (1, 320, 0), (0, 0, 0)])],
        ] {
            let mut state = InputObserver::replay_owned_touch(batches, tap());
            assert!(state.finish_owned_touch().is_err());
            assert!(state.poll().is_err());
            assert!(!state.quiescent());
        }
        let mut malformed = InputObserver::replay_owned_touch(vec![], vec![(0, 3, 0)]);
        assert!(malformed.finish_owned_touch().is_err());
        assert!(malformed.poll().is_err());
    }

    #[test]
    fn exact_single_contact_accepts_split_batches_and_kernel_axis_deduplication() {
        let mut full = io(tap());
        assert!(finish(&mut full, Duration::ZERO, POINT, &mut decoder()).is_ok());
        // evdev may omit unchanged ABS slot/coordinates; the retained seed is
        // authoritative, not an assumed zero position.
        let mut dedup = io(vec![(3, 57, 1), (0, 0, 0), (3, 57, -1), (0, 0, 0)]);
        dedup.batches = vec![vec![(3, 57, 1)], vec![(0, 0, 0), (3, 57, -1), (0, 0, 0)]].into();
        assert!(finish(&mut dedup, Duration::ZERO, POINT, &mut decoder()).is_ok());
    }
    #[test]
    fn malformed_wrong_point_repeated_or_missing_delivery_refuses() {
        let mut repeated = tap();
        repeated.extend(tap());
        for events in [
            vec![],
            vec![(0, 0, 0)],
            tap()[..9].to_vec(),
            tap()[..11].to_vec(),
            repeated,
            vec![(3, 57, 1), (3, 53, 701), (0, 0, 0), (3, 57, -1), (0, 0, 0)],
            vec![(3, 47, 1)],
            vec![(0, 3, 0)],
            vec![(1, 330, 1)],
            vec![(3, 57, 2)],
        ] {
            assert!(finish(&mut io(events), Duration::ZERO, POINT, &mut decoder()).is_err());
        }
    }
    #[test]
    fn deadlines_identity_kernel_contacts_and_completed_external_input_refuse() {
        for case in 0..5 {
            let mut state = io(tap());
            match case {
                0 => state.now = Duration::from_secs(1),
                1 => state.cost = Duration::from_millis(25),
                2 => state.wrong = true,
                3 => state.held = true,
                _ => state.external = true,
            }
            assert!(finish(&mut state, Duration::ZERO, POINT, &mut decoder()).is_err());
        }
    }
}

//! The sole event-drain policy for a bounded, explicitly owned pen contact.
//! Other sources are never drained here; the native adapter polls them normally.
use anyhow::{ensure, Result};
use std::time::Duration;

pub(super) const MAX_EVENTS: usize = 8192;
const WINDOW: Duration = Duration::from_secs(1);
const DRAIN: Duration = Duration::from_millis(50);
pub(super) type Event = (u16, u16, i32);

pub(super) trait WindowIo {
    fn now(&self) -> Duration;
    fn validate_source(&mut self) -> Result<()>;
    fn next_events(&mut self) -> Result<Option<Vec<Event>>>;
    fn released_snapshot(&mut self) -> Result<()>;
    fn check_other_input(&mut self) -> Result<()>;
}

pub(super) fn finish(io: &mut impl WindowIo, started: Duration) -> Result<()> {
    let drain_started = io.now();
    let deadline = |io: &dyn WindowIo| -> Result<()> {
        ensure!(
            io.now().saturating_sub(started) < WINDOW,
            "Owned pen window exceeded one second"
        );
        ensure!(
            io.now().saturating_sub(drain_started) < DRAIN,
            "Owned pen drain exceeded 50ms"
        );
        Ok(())
    };
    deadline(io)?;
    io.validate_source()?;
    let mut count = 0usize;
    let mut frames = 0usize;
    let mut partial = false;
    // The entry guard establishes all keys released. Check complete frames;
    // an event-class whitelist alone would accept malformed contact sequences.
    let (mut pen, mut rubber, mut touch) = (false, false, false);
    let mut pressure = 0;
    loop {
        deadline(io)?;
        let events = io.next_events()?;
        deadline(io)?;
        let Some(events) = events else { break };
        ensure!(!events.is_empty(), "Empty nonterminal owned event batch");
        count = count
            .checked_add(events.len())
            .ok_or_else(|| anyhow::anyhow!("Owned event count overflow"))?;
        ensure!(count <= MAX_EVENTS, "Owned pen event limit");
        for (kind, code, value) in events {
            match (kind, code, value) {
                (0, 0, 0) => {
                    frames += 1;
                    ensure!(
                        !(pen && rubber) && (!touch || pen || rubber) && (touch == (pressure > 0)),
                        "Malformed owned contact frame"
                    );
                    partial = false;
                }
                (1, 320, 0..=1) => {
                    pen = value == 1;
                    partial = true;
                }
                (1, 321, 0..=1) => {
                    rubber = value == 1;
                    partial = true;
                }
                (1, 330, 0..=1) => {
                    touch = value == 1;
                    partial = true;
                }
                (3, 24, 0..=4095) => {
                    pressure = value;
                    partial = true;
                }
                (3, 0 | 1, 0..=32767) | (3, 25, 0..=255) => {
                    partial = true;
                }
                _ => anyhow::bail!("Unexpected or dropped owned pen event kind={kind} code={code}"),
            }
        }
    }
    ensure!(
        !partial && !pen && !rubber && !touch && pressure == 0,
        "Owned pen stream is incomplete or held"
    );
    io.released_snapshot()?;
    io.validate_source()?;
    // This also processes any pen events delivered after the closed drain.
    // They are ordinary external activity, never silently discarded later.
    io.check_other_input()?;
    deadline(io)?;
    log::debug!(
        "Owned pen event drain observed events={count} frames={frames} window_us={} drain_us={}",
        io.now().saturating_sub(started).as_micros(),
        io.now().saturating_sub(drain_started).as_micros()
    );
    deadline(io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    struct Io {
        now: Duration,
        fetch_cost: Duration,
        batches: VecDeque<Vec<Event>>,
        wrong_identity: bool,
        held: bool,
        external: bool,
        calls: Vec<&'static str>,
    }
    impl WindowIo for Io {
        fn now(&self) -> Duration {
            self.now
        }
        fn validate_source(&mut self) -> Result<()> {
            self.calls.push("identity");
            ensure!(!self.wrong_identity, "writer/reader/inventory mismatch");
            Ok(())
        }
        fn next_events(&mut self) -> Result<Option<Vec<Event>>> {
            self.calls.push("drain");
            self.now += self.fetch_cost;
            Ok(self.batches.pop_front())
        }
        fn released_snapshot(&mut self) -> Result<()> {
            self.calls.push("released");
            ensure!(!self.held, "kernel contact/key held");
            Ok(())
        }
        fn check_other_input(&mut self) -> Result<()> {
            self.calls.push("other-input");
            ensure!(
                !self.external,
                "completed unrelated gesture or delayed pen event"
            );
            Ok(())
        }
    }
    fn io(batches: Vec<Vec<Event>>) -> Io {
        Io {
            now: Duration::from_millis(100),
            fetch_cost: Duration::ZERO,
            batches: batches.into(),
            wrong_identity: false,
            held: false,
            external: false,
            calls: vec![],
        }
    }
    fn pen_frames() -> Vec<Event> {
        vec![
            (3, 0, 100),
            (3, 1, 200),
            (1, 320, 1),
            (0, 0, 0),
            (1, 330, 1),
            (3, 24, 2630),
            (0, 0, 0),
            (3, 24, 0),
            (1, 330, 0),
            (1, 320, 0),
            (0, 0, 0),
        ]
    }
    #[test]
    fn complete_frames_across_batches_keep_other_sources_for_final_check() {
        let frames = pen_frames();
        let mut state = io(vec![frames[..5].to_vec(), frames[5..].to_vec()]);
        finish(&mut state, Duration::ZERO).unwrap();
        assert_eq!(
            state.calls,
            [
                "identity",
                "drain",
                "drain",
                "drain",
                "released",
                "identity",
                "other-input"
            ]
        );
        for field in 0..3 {
            let mut state = io(vec![pen_frames()]);
            match field {
                0 => state.wrong_identity = true,
                1 => state.held = true,
                _ => state.external = true,
            };
            assert!(finish(&mut state, Duration::ZERO).is_err());
            if field == 0 {
                assert_eq!(state.calls, ["identity"]);
            }
        }
    }
    #[test]
    fn dropped_unexpected_partial_or_held_pen_streams_fail_closed() {
        for events in [
            vec![(0, 3, 0)],
            vec![(1, 331, 1)],
            vec![(3, 24, 5000)],
            vec![(3, 2, 0)],
            vec![(3, 0, -1)],
            vec![(1, 320, 2)],
            vec![(1, 330, 1), (0, 0, 0)],
            vec![(1, 320, 1), (1, 321, 1), (0, 0, 0)],
            vec![(1, 320, 1), (0, 0, 0)],
            vec![(3, 0, 100)],
            vec![],
        ] {
            let mut state = io(vec![events]);
            assert!(finish(&mut state, Duration::ZERO).is_err());
            assert!(!state.calls.contains(&"other-input"));
        }
    }
    #[test]
    fn count_and_both_deadlines_are_checked_after_observations() {
        let mut overflow = io(vec![vec![(0, 0, 0); MAX_EVENTS + 1]]);
        assert!(finish(&mut overflow, Duration::ZERO).is_err());
        let mut accumulated = io(vec![vec![(0, 0, 0); MAX_EVENTS], vec![(0, 0, 0)]]);
        assert!(finish(&mut accumulated, Duration::ZERO).is_err());
        let mut late = io(vec![pen_frames()]);
        late.fetch_cost = DRAIN;
        assert!(finish(&mut late, Duration::ZERO).is_err());
        let mut expired = io(vec![pen_frames()]);
        expired.now = WINDOW;
        assert!(finish(&mut expired, Duration::ZERO).is_err());
        assert!(expired.calls.is_empty());
    }
    #[test]
    fn write_and_rearm_failure_latch_prevents_any_later_mutation() {
        use super::super::status_style::{guarded_injection, WaitCancellation};
        let mut state = (
            WaitCancellation::default(),
            io(vec![vec![(0, 3, 0)]]),
            0usize,
        );
        let error = guarded_injection(
            &mut state,
            |s| &mut s.0,
            |s| {
                s.2 += 1;
                Err(std::io::Error::from_raw_os_error(5).into())
            },
            |s| finish(&mut s.1, Duration::ZERO),
        )
        .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(5)
        );
        assert!(format!("{error:#}").contains("Unexpected or dropped"));
        assert_eq!(state.2, 1);
        assert!(state.1.calls.contains(&"drain"));
        assert!(guarded_injection(
            &mut state,
            |s| &mut s.0,
            |s| {
                s.2 += 1;
                Ok(())
            },
            |_| panic!("rearm after latch")
        )
        .is_err());
        assert_eq!(state.2, 1);
    }
}

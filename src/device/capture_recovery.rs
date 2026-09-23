//! Bounded read-only recovery; this interface exposes no mutation operation.
use super::screenshot::VanishedDiscoveryCandidate;
use anyhow::Result;
use std::time::Duration;

const BUDGET: Duration = Duration::from_millis(500);

pub(super) trait ObservationIo {
    type Owner: PartialEq;
    type Frame;
    fn now(&self) -> Duration;
    fn owner(&mut self) -> Result<Self::Owner>;
    fn check_input(&mut self) -> Result<()>;
    fn capture(&mut self) -> Result<Self::Frame>;
}

pub(super) fn observe<I: ObservationIo>(io: &mut I) -> Result<I::Frame> {
    let started = io.now();
    io.check_input()?;
    let owner = io.owner()?;
    let check = |io: &mut I| -> Result<()> {
        io.check_input()?;
        anyhow::ensure!(
            io.owner()? == owner,
            "Owner/session changed during capture recovery"
        );
        io.check_input()?;
        anyhow::ensure!(
            io.now().saturating_sub(started) < BUDGET,
            "Status capture exceeded 500ms observation budget"
        );
        Ok(())
    };
    check(io)?;
    let frame = match io.capture() {
        Ok(frame) => frame,
        Err(first) if first.downcast_ref::<VanishedDiscoveryCandidate>().is_some() => {
            // Re-enter the complete capture, never continue a partially scanned map.
            let retry = (|| {
                check(io)?;
                log::debug!("One fresh read-only status capture after vanished discovery candidate: {first:#}");
                let frame = io.capture()?;
                check(io)?;
                Ok(frame)
            })();
            return retry.map_err(|error: anyhow::Error| {
                first.context(format!("Fresh capture recovery failed: {error:#}"))
            });
        }
        Err(error) => return Err(error),
    };
    check(io)?;
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    struct Io {
        elapsed: Duration,
        cost: Duration,
        guard_cost: Duration,
        reads: usize,
        owner_change: usize,
        change_at: usize,
        input_after_read: bool,
        outcomes: VecDeque<Result<u8>>,
    }
    impl ObservationIo for Io {
        type Owner = [u8; 3];
        type Frame = u8;
        fn now(&self) -> Duration {
            self.elapsed
        }
        fn owner(&mut self) -> Result<Self::Owner> {
            self.elapsed += self.guard_cost;
            let mut owner = [1, 2, 3];
            if self.owner_change > 0 && self.reads >= self.change_at {
                owner[self.owner_change - 1] += 1;
            }
            Ok(owner)
        }
        fn check_input(&mut self) -> Result<()> {
            anyhow::ensure!(
                !(self.input_after_read && self.reads > 0),
                "external input/cancelled"
            );
            Ok(())
        }
        fn capture(&mut self) -> Result<u8> {
            self.reads += 1;
            self.elapsed += self.cost;
            self.outcomes.pop_front().expect("unexpected extra capture")
        }
    }
    fn vanished() -> anyhow::Error {
        anyhow::Error::from(std::io::Error::from_raw_os_error(5))
            .context(VanishedDiscoveryCandidate)
    }
    fn io(outcomes: Vec<Result<u8>>) -> Io {
        Io {
            elapsed: Duration::ZERO,
            cost: Duration::from_millis(120),
            guard_cost: Duration::ZERO,
            reads: 0,
            owner_change: 0,
            change_at: 1,
            input_after_read: false,
            outcomes: outcomes.into(),
        }
    }
    #[test]
    fn exactly_one_fresh_read_and_no_retry_for_untyped_errors() {
        let mut state = io(vec![Err(vanished()), Ok(7)]);
        assert_eq!(observe(&mut state).unwrap(), 7);
        assert_eq!(state.reads, 2);
        for error in [
            anyhow::anyhow!("ambiguous allocation"),
            std::io::Error::from_raw_os_error(5).into(),
            std::io::Error::from_raw_os_error(13).into(),
        ] {
            let mut state = io(vec![Err(error)]);
            assert!(observe(&mut state).is_err());
            assert_eq!(state.reads, 1);
        }
    }
    #[test]
    fn changed_owner_or_input_prevents_retry_and_rejects_first_success() {
        for failure in [false, true] {
            for changed_owner in 0..=3 {
                let mut state = io(vec![if failure { Err(vanished()) } else { Ok(1) }]);
                state.owner_change = changed_owner;
                state.input_after_read = changed_owner == 0;
                assert!(observe(&mut state).is_err());
                assert_eq!(state.reads, 1);
            }
        }
    }
    #[test]
    fn persistent_failure_keeps_original_os_chain() {
        let mut state = io(vec![
            Err(vanished()),
            Err(anyhow::anyhow!("second capture failure")),
        ]);
        let error = observe(&mut state).unwrap_err();
        assert_eq!(state.reads, 2);
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(5)
        );
        assert!(format!("{error:#}").contains("second capture failure"));
    }
    #[test]
    fn budget_includes_first_capture_and_rejects_late_success() {
        let mut slow_valid = io(vec![Ok(3)]);
        slow_valid.cost = Duration::from_millis(499);
        assert_eq!(observe(&mut slow_valid).unwrap(), 3);
        for first_failure in [false, true] {
            let mut state = io(vec![if first_failure {
                Err(vanished())
            } else {
                Ok(1)
            }]);
            state.cost = Duration::from_millis(500);
            assert!(observe(&mut state).is_err());
            assert_eq!(state.reads, 1);
        }
        let mut late = io(vec![Err(vanished()), Ok(9)]);
        late.cost = Duration::from_millis(250);
        assert!(observe(&mut late).is_err());
        assert_eq!(late.reads, 2);
        let mut late_guard = io(vec![Err(vanished())]);
        late_guard.guard_cost = Duration::from_millis(130);
        assert!(observe(&mut late_guard).is_err());
        assert_eq!(
            late_guard.reads, 1,
            "Owner/input revalidation consumes the same budget"
        );
        for changed_owner in 1..=3 {
            let mut changed = io(vec![Err(vanished()), Ok(9)]);
            changed.owner_change = changed_owner;
            changed.change_at = 2;
            assert!(observe(&mut changed).is_err());
            assert_eq!(changed.reads, 2);
        }
    }
}

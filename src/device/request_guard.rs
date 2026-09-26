//! Read-only source ownership across capture and inference; no status lease.
use anyhow::{ensure, Result};

pub(super) trait RequestIo {
    type Owner: PartialEq;
    fn owner(&mut self) -> Result<Self::Owner>;
    fn input_clear(&mut self) -> Result<()>;
}

pub(super) struct RequestGuard<O> {
    owner: O,
    lost: bool,
}

impl<O: PartialEq> RequestGuard<O> {
    pub fn begin(io: &mut impl RequestIo<Owner = O>) -> Result<Self> {
        io.input_clear()?;
        let owner = io.owner()?;
        Self::from_observed(owner, io)
    }

    pub fn from_observed(owner: O, io: &mut impl RequestIo<Owner = O>) -> Result<Self> {
        let mut guard = Self { owner, lost: false };
        guard.check(io)?;
        Ok(guard)
    }

    pub fn check(&mut self, io: &mut impl RequestIo<Owner = O>) -> Result<()> {
        ensure!(!self.lost, "Request ownership already lost");
        let result = (|| {
            io.input_clear()?;
            ensure!(io.owner()? == self.owner, "Request page/session changed");
            io.input_clear()?;
            Ok(())
        })();
        self.lost |= result.is_err();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Io {
        owner: [u8; 4],
        input: bool,
        polls: usize,
        change_on_poll: usize,
    }
    impl RequestIo for Io {
        type Owner = [u8; 4];
        fn owner(&mut self) -> Result<Self::Owner> {
            Ok(self.owner)
        }
        fn input_clear(&mut self) -> Result<()> {
            self.polls += 1;
            if self.polls == self.change_on_poll {
                self.owner[0] += 1;
                self.input = true;
            }
            ensure!(!self.input, "Completed external input");
            Ok(())
        }
    }
    fn io() -> Io {
        Io {
            owner: [1, 2, 3, 4],
            input: false,
            polls: 0,
            change_on_poll: usize::MAX,
        }
    }
    #[test]
    fn every_identity_dimension_and_completed_input_are_sticky() {
        for dimension in 0..5 {
            let mut io = io();
            let mut guard = RequestGuard::begin(&mut io).unwrap();
            guard.check(&mut io).unwrap();
            if dimension < 4 {
                io.owner[dimension] += 1;
            } else {
                io.input = true;
            }
            assert!(guard.check(&mut io).is_err());
            io.owner = [1, 2, 3, 4];
            io.input = false;
            assert!(guard.check(&mut io).is_err());
        }
    }
    #[test]
    fn input_between_owner_observation_and_second_poll_refuses() {
        let mut io = io();
        io.change_on_poll = 3;
        assert!(RequestGuard::begin(&mut io).is_err());
    }
    #[test]
    fn transferred_owner_is_checked_without_repinning_or_discarding_input() {
        for boundary in 0..3 {
            let mut io = io();
            let verified = io.owner;
            if boundary == 1 {
                io.owner[1] += 1;
            }
            if boundary == 2 {
                io.input = true;
            }
            assert_eq!(
                RequestGuard::from_observed(verified, &mut io).is_ok(),
                boundary == 0
            );
        }
    }
}

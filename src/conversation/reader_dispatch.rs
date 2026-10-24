//! Private once-only accounting. This state alone never permits backend I/O.
use super::{ReaderHandoff, ReaderPlan};
use anyhow::{ensure, Result};
use std::cell::Cell;

#[derive(Default)]
pub(super) struct DispatchState {
    next: Cell<usize>,
    busy: Cell<bool>,
    stopped: Cell<bool>,
}

impl DispatchState {
    pub(super) fn stop(&self) {
        self.stopped.set(true);
    }
    /// Reserve before acquiring admission so reentrant dispatch cannot wait on
    /// its own gate. Native and Store validation must precede `enter`.
    pub(super) fn reserve<'a>(
        &'a self,
        plan: &ReaderPlan,
        ordinal: usize,
        handoff: &ReaderHandoff,
    ) -> Result<Reservation<'a>> {
        let valid = !self.stopped.get()
            && !self.busy.get()
            && ordinal == self.next.get()
            && plan.steps().get(ordinal) == Some(handoff);
        if !valid {
            self.stopped.set(true);
        }
        ensure!(
            valid,
            "Reader dispatch missing, changed, consumed or stopped"
        );
        self.busy.set(true);
        Ok(Reservation { state: self })
    }
}

pub(super) struct Reservation<'a> {
    state: &'a DispatchState,
}
impl<'a> Reservation<'a> {
    pub(super) fn enter(self) -> Result<Entered<'a>> {
        ensure!(
            !self.state.stopped.get(),
            "Reader dispatch stopped during validation"
        );
        let state = self.state;
        // Transfer the single reservation into the entered guard. There is no
        // reset or independent state copy; dropping either guard stops dispatch.
        std::mem::forget(self);
        Ok(Entered {
            state,
            submitted: false,
        })
    }
}
impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.state.stopped.set(true);
        self.state.busy.set(false);
    }
}

pub(super) struct Entered<'a> {
    state: &'a DispatchState,
    submitted: bool,
}
impl Entered<'_> {
    pub(super) fn ensure_current(&self) -> Result<()> {
        ensure!(
            !self.state.stopped.get(),
            "Reader dispatch invalidated during composite"
        );
        Ok(())
    }
    /// Backend return is submission accounting only, never durable completion.
    pub(super) fn submitted(mut self) -> Result<()> {
        ensure!(
            !self.state.stopped.get(),
            "Reader dispatch invalidated during submission"
        );
        let next = self
            .state
            .next
            .get()
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Reader ordinal overflow"))?;
        self.state.next.set(next);
        self.submitted = true;
        Ok(())
    }
}
impl Drop for Entered<'_> {
    fn drop(&mut self) {
        if !self.submitted {
            self.state.stopped.set(true);
        }
        self.state.busy.set(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plan() -> ReaderPlan {
        ReaderPlan::new(vec![
            ReaderHandoff::NextPage,
            ReaderHandoff::Text("answer".into()),
        ])
        .unwrap()
    }
    #[test]
    fn distinct_steps_advance_once_and_duplicate_stops_following_step() {
        let plan = plan();
        let state = DispatchState::default();
        state
            .reserve(&plan, 0, &plan.steps()[0])
            .unwrap()
            .enter()
            .unwrap()
            .submitted()
            .unwrap();
        assert!(state.reserve(&plan, 0, &plan.steps()[0]).is_err());
        assert!(state.reserve(&plan, 1, &plan.steps()[1]).is_err());
        let state = DispatchState::default();
        for (ordinal, step) in plan.steps().iter().enumerate() {
            state
                .reserve(&plan, ordinal, step)
                .unwrap()
                .enter()
                .unwrap()
                .submitted()
                .unwrap();
        }
        assert!(state.reserve(&plan, 2, &ReaderHandoff::NextPage).is_err());
    }
    #[test]
    fn changed_arguments_and_abandoned_validation_are_sticky() {
        let plan = plan();
        let state = DispatchState::default();
        assert!(state
            .reserve(&plan, 0, &ReaderHandoff::PreviousPage)
            .is_err());
        assert!(state.reserve(&plan, 0, &plan.steps()[0]).is_err());
        let state = DispatchState::default();
        drop(state.reserve(&plan, 0, &plan.steps()[0]).unwrap());
        assert!(state.reserve(&plan, 0, &plan.steps()[0]).is_err());
    }
    #[test]
    fn reentrant_attempt_invalidates_original_and_panic_stops_later_steps() {
        let plan = plan();
        let state = DispatchState::default();
        let entered = state
            .reserve(&plan, 0, &plan.steps()[0])
            .unwrap()
            .enter()
            .unwrap();
        assert!(state.reserve(&plan, 0, &plan.steps()[0]).is_err());
        assert!(entered.submitted().is_err());
        assert!(state.reserve(&plan, 1, &plan.steps()[1]).is_err());
        let state = DispatchState::default();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _entered = state
                .reserve(&plan, 0, &plan.steps()[0])
                .unwrap()
                .enter()
                .unwrap();
            panic!("backend panic");
        }));
        assert!(panic.is_err());
        assert!(state.reserve(&plan, 1, &plan.steps()[1]).is_err());
    }
}

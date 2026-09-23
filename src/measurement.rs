//! Bounded, content-free timing events. These measure host execution, not visible
//! pixels or simulated device time. An end event includes failed/unwound scopes;
//! it does not assert successful completion. Nested/overlapping spans are not additive.
use std::{
    cell::Cell,
    marker::PhantomData,
    rc::Rc,
    sync::{
        atomic::{AtomicU64, Ordering},
        OnceLock,
    },
    time::Instant,
};

static CLOCK: OnceLock<Instant> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);
thread_local! { static RUN: Cell<u64> = const { Cell::new(0) }; }

pub(crate) fn context() -> u64 {
    RUN.get()
}

/// Explicit propagation into the existing provider worker; never a global active
/// run, so concurrent simulators cannot mix their operation attribution.
pub(crate) struct Run {
    previous: u64,
    _thread: PhantomData<Rc<()>>,
}
impl Run {
    pub(crate) fn new() -> Self {
        Self::enter(NEXT.fetch_add(1, Ordering::Relaxed))
    }
    pub(crate) fn enter(run: u64) -> Self {
        Self {
            previous: RUN.replace(run),
            _thread: PhantomData,
        }
    }
}
impl Drop for Run {
    fn drop(&mut self) {
        RUN.set(self.previous);
    }
}

pub(crate) struct Span {
    phase: &'static str,
    run: u64,
    operation: u64,
    start: Instant,
    enabled: bool,
}
impl Span {
    pub(crate) fn new(phase: &'static str) -> Self {
        let origin = CLOCK.get_or_init(Instant::now);
        let span = Self {
            phase,
            run: context(),
            operation: NEXT.fetch_add(1, Ordering::Relaxed),
            start: Instant::now(),
            enabled: log::log_enabled!(log::Level::Debug),
        };
        if span.enabled {
            log::debug!(
                "timing pid={} run={} op={} phase={} event=begin at_us={}",
                std::process::id(),
                span.run,
                span.operation,
                phase,
                origin.elapsed().as_micros()
            );
        }
        span
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        if self.enabled {
            log::debug!(
                "timing pid={} run={} op={} phase={} event=end at_us={} elapsed_us={}",
                std::process::id(),
                self.run,
                self.operation,
                self.phase,
                CLOCK.get().unwrap().elapsed().as_micros(),
                self.start.elapsed().as_micros()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn worker_context_is_explicit_and_unwind_restores_outer_run() {
        let _outer = Run::new();
        let outer = context();
        std::thread::scope(|scope| {
            scope
                .spawn(move || {
                    assert_eq!(context(), 0);
                    {
                        let _worker = Run::enter(outer);
                        assert_eq!(context(), outer);
                        let _ = std::panic::catch_unwind(|| {
                            let _nested = Run::new();
                            assert_ne!(context(), outer);
                            panic!("interrupted measurement");
                        });
                        assert_eq!(context(), outer);
                    }
                    assert_eq!(context(), 0);
                })
                .join()
                .unwrap();
        });
        assert_eq!(context(), outer);
    }
}

//! Persistence polling shared by native history and deterministic timing tests.
use crate::workflow::history::PageState;
use anyhow::{ensure, Result};
use std::time::Duration;

/// Outer errors are cancellation/ownership failures and stop immediately. Inner
/// errors are unsettled native observations; they reset the matching sequence.
pub(super) fn wait_settled(
    mut observe: impl FnMut() -> Result<Result<PageState>>,
    mut now: impl FnMut() -> Duration,
    mut pace: impl FnMut(Duration),
) -> Result<PageState> {
    let deadline = now().saturating_add(Duration::from_secs(30));
    let mut previous = None;
    let mut repeats = 0;
    let mut last_error = "Native state did not settle".to_owned();
    loop {
        ensure!(now() < deadline, "{last_error}");
        let observed = observe()?;
        // Includes all native reads and the caller's final input guard. Even a
        // third exact match is not completion if it arrived at/after the bound.
        ensure!(
            now() < deadline,
            "Native history observation exceeded deadline: {last_error}"
        );
        match observed {
            Ok(mut page) => {
                if previous.as_ref() == Some(&page) {
                    repeats += 1;
                } else {
                    repeats = 0;
                }
                if repeats >= 2 {
                    page.supported = true;
                    return Ok(page);
                }
                previous = Some(page);
            }
            Err(error) => {
                previous = None;
                repeats = 0;
                last_error = error.to_string();
            }
        }
        pace(Duration::from_millis(100).min(deadline.saturating_sub(now())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{device::native_text::NativeText, workflow::history::Owner};
    use std::cell::Cell;

    fn page() -> PageState {
        PageState {
            owner: Owner {
                document: "doc".into(),
                page: "page".into(),
                visit: "1:2".into(),
                session: "session".into(),
            },
            content: NativeText {
                paragraphs: vec![],
                root_layout: vec![],
                scene_records: vec![],
            },
            seal: vec![1],
            supported: false,
        }
    }

    #[test]
    fn accepts_three_fresh_matches_but_not_late_or_boundary_third_match() {
        for third_end in [
            Duration::from_millis(29999),
            Duration::from_secs(30),
            Duration::from_millis(30001),
        ] {
            let clock = Cell::new(Duration::ZERO);
            let calls = Cell::new(0);
            let result = wait_settled(
                || {
                    calls.set(calls.get() + 1);
                    if calls.get() == 3 {
                        clock.set(third_end);
                    }
                    Ok(Ok(page()))
                },
                || clock.get(),
                |delay| clock.set(clock.get() + delay),
            );
            assert_eq!(calls.get(), 3);
            assert_eq!(result.is_ok(), third_end < Duration::from_secs(30));
            if let Ok(page) = result {
                assert!(page.supported);
            }
        }
    }

    #[test]
    fn changes_and_pending_errors_reset_matches_and_cancellation_is_fatal() {
        let clock = Cell::new(Duration::ZERO);
        let calls = Cell::new(0);
        let ready = wait_settled(
            || {
                calls.set(calls.get() + 1);
                if calls.get() == 3 {
                    return Ok(Err(anyhow::anyhow!("pending persistence")));
                }
                let mut observed = page();
                if calls.get() == 5 {
                    observed.seal = vec![2];
                }
                Ok(Ok(observed))
            },
            || clock.get(),
            |delay| clock.set(clock.get() + delay),
        )
        .unwrap();
        assert!(ready.supported);
        assert_eq!(calls.get(), 8);
        calls.set(0);
        let result = wait_settled(
            || {
                calls.set(calls.get() + 1);
                if calls.get() == 3 {
                    return Err(anyhow::anyhow!("input cancelled after observation"));
                }
                Ok(Ok(page()))
            },
            || clock.get(),
            |delay| clock.set(clock.get() + delay),
        );
        assert!(result.unwrap_err().to_string().contains("input cancelled"));
        assert_eq!(calls.get(), 3);
    }
}

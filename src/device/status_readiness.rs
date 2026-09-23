//! Read-only readiness before a status lease exists. Never refresh an active
//! lease baseline, mutate a toolbar, or infer completion from elapsed time.
use super::status_style::Observation;
use anyhow::{ensure, Result};
use image::GrayImage;
use std::time::Duration;

const LIMIT: Duration = Duration::from_secs(5);
const PACE: Duration = Duration::from_millis(50);

fn footer_ready(image: &GrayImage) -> bool {
    image.dimensions() == (768, 1024)
        && (984..1024).all(|y| {
            let white = (61..768).all(|x| image.get_pixel(x, y)[0] >= 248);
            white || (y == 1009 && (61..768).all(|x| image.get_pixel(x, y)[0] <= 8))
        })
}

/// The caller supplies fresh synchronous observations and checks external input
/// both sides of capture. Timeout safely declines optional status before journal
/// creation; cancellation or changed ownership/content fails the operation.
pub(super) fn wait_ready(
    mut observe: impl FnMut() -> Result<Observation>,
    mut guard: impl FnMut() -> Result<()>,
    mut now: impl FnMut() -> Duration,
    mut pause: impl FnMut(Duration),
    debug_dump: bool,
) -> Result<Option<Observation>> {
    let _timing = crate::measurement::Span::new("status.readiness");
    let deadline = now().saturating_add(LIMIT);
    let mut initial: Option<Observation> = None;
    loop {
        guard()?;
        if now() >= deadline {
            return Ok(None);
        }
        let current = observe()?;
        guard()?;
        ensure!(
            current.image.dimensions() == (768, 1024),
            "Unknown status readiness dimensions"
        );
        if let Some(first) = &initial {
            let same_owner = current.identity == first.identity;
            let same_content = (0..984).all(|y| {
                (0..768).all(|x| {
                    current.image.get_pixel(x, y)[0].abs_diff(first.image.get_pixel(x, y)[0]) <= 8
                })
            });
            if debug_dump && (!same_owner || !same_content) {
                // Exactly the observations about to be rejected; fixed bounded
                // opt-in artifacts, never a recapture or replacement baseline.
                for (name, observation) in [("before", first), ("rejected", &current)] {
                    let result = (|| -> Result<()> {
                        observation
                            .image
                            .save(format!("/tmp/reader-buddy-readiness-{name}.png"))?;
                        std::fs::write(
                            format!("/tmp/reader-buddy-readiness-{name}.json"),
                            serde_json::to_vec_pretty(&observation.identity)?,
                        )?;
                        Ok(())
                    })();
                    if let Err(error) = result {
                        log::warn!("Could not save readiness diagnostic: {error}");
                    }
                }
            }
            ensure!(same_owner, "Page/session changed during status readiness");
            ensure!(
                same_content,
                "Page or toolbar changed during status readiness"
            );
        }
        // A slow observation arriving after the deadline cannot establish readiness.
        if now() >= deadline {
            return Ok(None);
        }
        if footer_ready(&current.image) {
            return Ok(Some(current));
        }
        if initial.is_none() {
            initial = Some(current);
        }
        pause(PACE.min(deadline.saturating_sub(now())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::status_style::Identity;
    use std::{cell::Cell, collections::VecDeque};

    fn frame(ready: bool) -> Observation {
        let data: &[u8] = if ready {
            include_bytes!("../../tests/fixtures/status-style/native-footer-after.png")
        } else {
            include_bytes!("../../tests/fixtures/status-style/native-footer-before.png")
        };
        Observation {
            identity: Identity {
                document: "doc".into(),
                page: "page".into(),
                visit: "1:2".into(),
                session: "3:4".into(),
            },
            preferences: Default::default(),
            image: image::load_from_memory(data).unwrap().to_luma8(),
        }
    }

    fn run(
        frames: Vec<Observation>,
        cancel_at: Option<usize>,
        cost: Duration,
    ) -> (Result<Option<Observation>>, usize, usize) {
        let mut frames: VecDeque<_> = frames.into();
        let last = frames.back().unwrap().clone();
        let clock = Cell::new(Duration::ZERO);
        let captures = Cell::new(0);
        let guards = Cell::new(0);
        let pauses = Cell::new(0);
        let result = wait_ready(
            || {
                captures.set(captures.get() + 1);
                clock.set(clock.get() + cost);
                Ok(frames.pop_front().unwrap_or_else(|| last.clone()))
            },
            || {
                guards.set(guards.get() + 1);
                ensure!(cancel_at != Some(guards.get()), "Cancelled input ownership");
                Ok(())
            },
            || clock.get(),
            |duration| {
                assert!(duration <= PACE);
                pauses.set(pauses.get() + 1);
                clock.set(clock.get() + duration);
            },
            false,
        );
        (result, captures.get(), pauses.get())
    }

    #[test]
    fn immediate_and_delayed_footer_readiness_use_fresh_final_frame() {
        let (result, captures, pauses) = run(vec![frame(true)], None, Duration::ZERO);
        assert!(result.unwrap().is_some());
        assert_eq!((captures, pauses), (1, 0));
        let ready = frame(true);
        let (result, captures, pauses) = run(
            vec![frame(false), frame(false), ready.clone()],
            None,
            Duration::ZERO,
        );
        assert_eq!(result.unwrap().unwrap().image, ready.image);
        assert_eq!((captures, pauses), (3, 2));
    }

    #[test]
    fn repeated_overlay_unknown_ink_and_late_ready_frame_are_not_success() {
        let (result, captures, _) = run(vec![frame(false)], None, Duration::from_millis(100));
        assert!(result.unwrap().is_none());
        assert!(captures <= 34);
        let mut ink = frame(true);
        ink.image.put_pixel(400, 1000, image::Luma([0]));
        assert!(run(vec![ink], None, Duration::from_secs(1))
            .0
            .unwrap()
            .is_none());
        assert!(run(vec![frame(true)], None, LIMIT).0.unwrap().is_none());
    }

    #[test]
    fn owner_content_or_input_changes_cancel_without_accepting_later_ready_frame() {
        for field in 0..4 {
            let mut wrong = frame(true);
            match field {
                0 => wrong.identity.document.push('x'),
                1 => wrong.identity.page.push('x'),
                2 => wrong.identity.visit.push('x'),
                _ => wrong.identity.session.push('x'),
            }
            let (result, count, _) =
                run(vec![frame(false), wrong, frame(true)], None, Duration::ZERO);
            assert!(result.is_err());
            assert_eq!(count, 2);
        }
        let mut edited = frame(true);
        edited.image.put_pixel(400, 700, image::Luma([0]));
        assert!(run(
            vec![frame(false), edited, frame(true)],
            None,
            Duration::ZERO
        )
        .0
        .is_err());
        for guard in [1, 2, 3, 4] {
            assert!(
                run(vec![frame(false), frame(true)], Some(guard), Duration::ZERO)
                    .0
                    .is_err()
            );
        }
    }
}

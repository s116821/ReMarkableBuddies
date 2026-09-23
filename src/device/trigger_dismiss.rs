//! Qualified outside-panel dismissal policy; no menu-item selection or retries.
use super::status_style::{controls, Identity, WaitCancellation};
use anyhow::{ensure, Result};
use image::GrayImage;
use std::{sync::LazyLock, time::Duration};

pub(super) const OUTSIDE_POINT: (i32, i32) = (384, 1023);
static OPEN: LazyLock<GrayImage> = LazyLock::new(|| {
    image::load_from_memory(include_bytes!(
        "../../tests/fixtures/trigger-dismiss/open.png"
    ))
    .expect("qualified native trigger panel")
    .to_luma8()
});

#[derive(Clone)]
pub(super) struct Snapshot {
    pub identity: Identity,
    /// Absence is explicit; a subsequently created file is a change.
    pub native: Option<Vec<u8>>,
    pub image: GrayImage,
}

#[derive(Debug, PartialEq, Eq)]
enum Panel {
    Closed,
    Open,
    Unknown,
}

fn panel(image: &GrayImage) -> Panel {
    if image.dimensions() != (768, 1024) {
        return Panel::Unknown;
    }
    // Positive full exposed seam/closed-control check excludes this native
    // panel and partial transitions. This does not recognize every popover.
    if controls(image).is_some_and(|ui| !ui.menu) {
        return Panel::Closed;
    }
    let selected = |y: u32| {
        [(5, y - 10), (55, y + 10), (5, y + 20), (55, y + 23)]
            .into_iter()
            .all(|(x, y)| image.get_pixel(x, y)[0] <= 8)
    };
    if selected(91) == selected(153) {
        return Panel::Unknown;
    }
    let upper_seam = (62..655).all(|y| (61..65).all(|x| image.get_pixel(x, y)[0] >= 248));
    let exact_panel = (655..1024)
        .all(|y| (0..280).all(|x| image.get_pixel(x, y)[0].abs_diff(OPEN.get_pixel(x, y)[0]) <= 8));
    if upper_seam && exact_panel {
        Panel::Open
    } else {
        Panel::Unknown
    }
}

pub(super) trait DismissIo {
    fn observe(&mut self) -> Result<Snapshot>;
    fn guard(&mut self) -> Result<()>;
    /// Adapter must release and finish the owned touch window even after write
    /// failure, preserving original errors. Policy never invokes this twice.
    fn tap_once(&mut self, point: (i32, i32)) -> Result<()>;
    fn now(&self) -> Duration;
    fn pace(&mut self, duration: Duration);
}

pub(super) fn dismiss(io: &mut impl DismissIo, cancelled: &mut WaitCancellation) -> Result<bool> {
    cancelled.check()?;
    let result = (|| {
        let deadline = io.now().saturating_add(Duration::from_secs(5));
        io.guard()?;
        let initial = io.observe()?;
        io.guard()?;
        ensure!(io.now() < deadline, "Trigger observation exceeded deadline");
        match panel(&initial.image) {
            Panel::Closed => return Ok(false),
            Panel::Unknown => anyhow::bail!("Unknown trigger overlay; no dismissal input"),
            Panel::Open => {}
        }
        io.guard()?;
        ensure!(io.now() < deadline, "Late trigger dismissal input guard");
        io.tap_once(OUTSIDE_POINT)?;
        loop {
            ensure!(
                io.now() < deadline,
                "Trigger panel did not dismiss before deadline"
            );
            io.guard()?;
            let current = io.observe()?;
            io.guard()?;
            ensure!(io.now() < deadline, "Late trigger dismissal observation");
            ensure!(
                current.identity == initial.identity && current.native == initial.native,
                "Trigger dismissal owner/native content changed"
            );
            ensure!(
                current.image.dimensions() == initial.image.dimensions(),
                "Trigger viewport changed"
            );
            ensure!(
                (0..1024).all(|y| (0..768).all(|x| {
                    (x < 280 && y >= 655)
                        || initial.image.get_pixel(x, y)[0]
                            .abs_diff(current.image.get_pixel(x, y)[0])
                            <= 8
                })),
                "Trigger dismissal changed exposed content or tool"
            );
            match panel(&current.image) {
                Panel::Closed => return Ok(true),
                Panel::Unknown => anyhow::bail!("Unknown trigger dismissal postcondition"),
                Panel::Open => {
                    io.pace(Duration::from_millis(50).min(deadline.saturating_sub(io.now())))
                }
            }
        }
    })();
    if result.is_err() {
        cancelled.latch();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;
    use std::collections::VecDeque;
    fn snapshot(open: bool) -> Snapshot {
        Snapshot {
            identity: Identity {
                document: "d".into(),
                page: "p".into(),
                visit: "1:1".into(),
                session: "2:3".into(),
            },
            native: Some(vec![1, 2, 3]),
            image: if open {
                OPEN.clone()
            } else {
                image::load_from_memory(include_bytes!(
                    "../../tests/fixtures/trigger-dismiss/closed.png"
                ))
                .unwrap()
                .to_luma8()
            },
        }
    }
    struct Io {
        observations: VecDeque<Snapshot>,
        last: Snapshot,
        clock: Duration,
        cost: Duration,
        taps: usize,
        cancelled: bool,
        fail_tap: bool,
    }
    impl DismissIo for Io {
        fn observe(&mut self) -> Result<Snapshot> {
            self.clock += self.cost;
            if let Some(next) = self.observations.pop_front() {
                self.last = next;
            }
            Ok(self.last.clone())
        }
        fn guard(&mut self) -> Result<()> {
            ensure!(!self.cancelled, "external input");
            Ok(())
        }
        fn tap_once(&mut self, point: (i32, i32)) -> Result<()> {
            assert_eq!(point, OUTSIDE_POINT);
            self.taps += 1;
            ensure!(!self.fail_tap, "touch write/release error");
            Ok(())
        }
        fn now(&self) -> Duration {
            self.clock
        }
        fn pace(&mut self, duration: Duration) {
            self.clock += duration;
        }
    }
    fn io(frames: Vec<Snapshot>) -> Io {
        Io {
            last: frames.last().unwrap().clone(),
            observations: frames.into(),
            clock: Duration::ZERO,
            cost: Duration::ZERO,
            taps: 0,
            cancelled: false,
            fail_tap: false,
        }
    }
    #[test]
    fn actual_panel_and_closed_pair_qualify_without_requiring_fineliner() {
        assert_eq!(panel(&snapshot(true).image), Panel::Open);
        assert_eq!(panel(&snapshot(false).image), Panel::Closed);
        for open in [false, true] {
            let mut frame = snapshot(open);
            // Alter the pen glyph/color inside the selected tile; eligibility
            // for optional ink is deliberately not a dismissal prerequisite.
            for y in 70..100 {
                for x in 20..45 {
                    frame.image.put_pixel(x, y, Luma([90]));
                }
            }
            assert_eq!(
                panel(&frame.image),
                if open { Panel::Open } else { Panel::Closed }
            );
        }
    }
    #[test]
    fn partial_or_shifted_native_panel_never_becomes_closed() {
        for (x, y) in [(100, 685), (279, 750), (30, 990)] {
            let mut frame = snapshot(true);
            let old = frame.image.get_pixel(x, y)[0];
            frame.image.put_pixel(x, y, Luma([255 - old]));
            assert_eq!(panel(&frame.image), Panel::Unknown);
            let mut state = io(vec![frame]);
            assert!(dismiss(&mut state, &mut WaitCancellation::default()).is_err());
            assert_eq!(state.taps, 0);
        }
    }
    #[test]
    fn closed_skips_and_delayed_known_panel_taps_only_once() {
        let mut closed = io(vec![snapshot(false)]);
        assert!(!dismiss(&mut closed, &mut WaitCancellation::default()).unwrap());
        assert_eq!(closed.taps, 0);
        let mut delayed = io(vec![
            snapshot(true),
            snapshot(true),
            snapshot(true),
            snapshot(false),
        ]);
        assert!(dismiss(&mut delayed, &mut WaitCancellation::default()).unwrap());
        assert_eq!(delayed.taps, 1);
    }
    #[test]
    fn changed_owner_native_presence_content_or_tool_is_sticky() {
        for change in 0..5 {
            let mut after = snapshot(false);
            match change {
                0 => after.identity.visit = "1:2".into(),
                1 => after.native = None,
                2 => after.native = Some(vec![9]),
                3 => after.image.put_pixel(300, 300, Luma([0])),
                _ => after.image.put_pixel(45, 75, Luma([255])),
            }
            let mut state = io(vec![snapshot(true), after]);
            let mut cancelled = WaitCancellation::default();
            assert!(dismiss(&mut state, &mut cancelled).is_err());
            assert!(dismiss(&mut state, &mut cancelled).is_err());
            assert_eq!(state.taps, 1);
        }
    }
    #[test]
    fn absent_native_file_stays_explicit_and_unmodified() {
        let mut before = snapshot(true);
        before.native = None;
        let mut after = snapshot(false);
        after.native = None;
        assert!(dismiss(
            &mut io(vec![before, after]),
            &mut WaitCancellation::default()
        )
        .unwrap());
    }
    #[test]
    fn timeout_late_success_input_or_write_failure_never_repeats_tap() {
        for failure in 0..4 {
            let mut state = io(if failure == 0 {
                vec![snapshot(true)]
            } else {
                vec![snapshot(true), snapshot(false)]
            });
            match failure {
                1 => state.cost = Duration::from_millis(2500),
                2 => state.cancelled = true,
                3 => state.fail_tap = true,
                _ => {}
            }
            let mut cancelled = WaitCancellation::default();
            assert!(dismiss(&mut state, &mut cancelled).is_err());
            let taps = state.taps;
            assert!(dismiss(&mut state, &mut cancelled).is_err());
            assert_eq!(state.taps, taps);
            assert!(taps <= 1);
        }
    }
}

use crate::device::{touch::Touch, DeviceModel};
use anyhow::{ensure, Result};
#[cfg(feature = "development-input-diagnostics")]
use log::debug;
use remarkable_open_sdk::navigation::{
    gesture::{plan_gesture, GesturePlan, Orientation, TabletModel},
    LogicalDirection,
};
use std::{thread::sleep, time::Duration};

pub struct XochitlIntegration;
impl XochitlIntegration {
    /// Diagnostic dispatch only; the caller explicitly supplies orientation.
    pub fn navigate_to_page(
        touch: &mut Touch,
        direction: NavigationDirection,
        orientation: Orientation,
    ) -> Result<()> {
        Self::swipe_with_orientation(touch, direction, orientation)
    }
    /// Geometry does not establish native page identity or completion.
    pub fn swipe_with_orientation(
        touch: &mut Touch,
        direction: NavigationDirection,
        orientation: Orientation,
    ) -> Result<()> {
        ensure!(
            touch.model() == DeviceModel::detect(),
            "Touch device model changed"
        );
        dispatch(touch, touch_model(touch.model()), orientation, direction)
    }

    /// Gesture only; completion is verified separately on supported layouts.
    #[cfg(feature = "development-input-diagnostics")]
    pub fn swipe(touch: &mut Touch, direction: NavigationDirection) -> Result<()> {
        match direction {
            NavigationDirection::Next => Self::swipe_left(touch)?,
            NavigationDirection::Previous => Self::swipe_right(touch)?,
        }
        Ok(())
    }

    /// One explicitly selected development geometry; never a completion claim.
    #[cfg(feature = "development-input-diagnostics")]
    pub fn swipe_fixed_gap10_development(
        touch: &mut Touch,
        direction: NavigationDirection,
    ) -> Result<()> {
        match direction {
            NavigationDirection::Next => Self::horizontal(touch, 700, 110),
            NavigationDirection::Previous => Self::horizontal(touch, 100, 690),
        }
    }

    /// Swipe left (go to next page)
    #[cfg(feature = "development-input-diagnostics")]
    fn swipe_left(touch: &mut Touch) -> Result<()> {
        debug!("Swiping left to next page");
        Self::horizontal(touch, 700, 100)
    }

    /// Swipe right (go to previous page)
    #[cfg(feature = "development-input-diagnostics")]
    fn swipe_right(touch: &mut Touch) -> Result<()> {
        debug!("Swiping right to previous page");
        Self::horizontal(touch, 100, 700)
    }

    #[cfg(feature = "development-input-diagnostics")]
    fn horizontal(touch: &mut Touch, start_x: i32, end_x: i32) -> Result<()> {
        let gesture = (|| -> Result<()> {
            touch.touch_start((start_x, 512))?;
            sleep(Duration::from_millis(50));
            for i in 1..=15 {
                // Preserve the old per-direction truncation arithmetic exactly.
                let offset = (((end_x - start_x).abs() as f32) * (i as f32 / 15.0)) as i32;
                let x = start_x + (end_x - start_x).signum() * offset;
                touch.goto_xy((x, 512))?;
                sleep(Duration::from_millis(10));
            }
            Ok(())
        })();
        let released = touch.touch_stop();
        gesture?;
        released?;
        Ok(())
    }
}
fn touch_model(model: DeviceModel) -> TabletModel {
    match model {
        DeviceModel::Remarkable2 => TabletModel::Remarkable2,
        DeviceModel::RemarkablePaperPro => TabletModel::PaperPro,
        DeviceModel::Unknown => TabletModel::Unknown,
    }
}
trait GestureIo {
    fn start(&mut self, xy: (i32, i32)) -> Result<()>;
    fn movement(&mut self, xy: (i32, i32)) -> Result<()>;
    fn release(&mut self) -> Result<()>;
    fn wait(&mut self, duration: Duration);
}
impl GestureIo for Touch {
    fn start(&mut self, xy: (i32, i32)) -> Result<()> {
        self.touch_start(xy)
    }
    fn movement(&mut self, xy: (i32, i32)) -> Result<()> {
        self.goto_xy(xy)
    }
    fn release(&mut self) -> Result<()> {
        self.touch_stop()
    }
    fn wait(&mut self, duration: Duration) {
        sleep(duration);
    }
}
fn dispatch(
    io: &mut impl GestureIo,
    model: TabletModel,
    orientation: Orientation,
    direction: NavigationDirection,
) -> Result<()> {
    let logical = match direction {
        NavigationDirection::Next => LogicalDirection::Next,
        NavigationDirection::Previous => LogicalDirection::Previous,
    };
    let plan = plan_gesture(model, orientation, logical)
        .map_err(|reason| anyhow::anyhow!("Unsupported navigation gesture: {reason:?}"))?;
    emit(io, &plan)
}
fn emit(io: &mut impl GestureIo, plan: &GesturePlan) -> Result<()> {
    let gesture: Result<()> = (|| {
        io.start(plan.start())?;
        io.wait(plan.hold_duration());
        for &xy in plan.movements() {
            io.movement(xy)?;
            io.wait(plan.movement_interval());
        }
        Ok(())
    })();
    // Even a partial start or failed movement must attempt release once.
    let released = io.release();
    gesture?;
    released
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationDirection {
    Next,
    Previous,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Fake {
        starts: Vec<(i32, i32)>,
        moves: Vec<(i32, i32)>,
        waits: Vec<Duration>,
        releases: usize,
        fail_start: bool,
        fail_move: bool,
        fail_release: bool,
    }
    impl GestureIo for Fake {
        fn start(&mut self, xy: (i32, i32)) -> Result<()> {
            self.starts.push(xy);
            ensure!(!self.fail_start, "start failed");
            Ok(())
        }
        fn movement(&mut self, xy: (i32, i32)) -> Result<()> {
            self.moves.push(xy);
            ensure!(!self.fail_move, "movement failed");
            Ok(())
        }
        fn release(&mut self) -> Result<()> {
            self.releases += 1;
            ensure!(!self.fail_release, "release failed");
            Ok(())
        }
        fn wait(&mut self, duration: Duration) {
            self.waits.push(duration);
        }
    }
    #[test]
    fn actual_dispatch_consumes_both_sdk_plans_once() {
        for direction in [NavigationDirection::Next, NavigationDirection::Previous] {
            let mut io = Fake::default();
            dispatch(
                &mut io,
                TabletModel::Remarkable2,
                Orientation::Portrait,
                direction,
            )
            .unwrap();
            let logical = if direction == NavigationDirection::Next {
                LogicalDirection::Next
            } else {
                LogicalDirection::Previous
            };
            let plan =
                plan_gesture(TabletModel::Remarkable2, Orientation::Portrait, logical).unwrap();
            assert_eq!(io.starts, vec![plan.start()]);
            assert_eq!(io.moves, plan.movements());
            assert_eq!(io.releases, 1);
            assert_eq!(io.waits[0], plan.hold_duration());
            assert_eq!(&io.waits[1..], &[plan.movement_interval(); 15]);
        }
    }
    #[test]
    fn unsupported_admission_emits_nothing() {
        for (model, orientation) in [
            (TabletModel::Unknown, Orientation::Portrait),
            (TabletModel::Remarkable2, Orientation::Unknown),
            (TabletModel::PaperPro, Orientation::Portrait),
            (TabletModel::Remarkable2, Orientation::LandscapeLeft),
        ] {
            let mut io = Fake::default();
            assert!(dispatch(&mut io, model, orientation, NavigationDirection::Next).is_err());
            assert!(io.starts.is_empty() && io.moves.is_empty() && io.waits.is_empty());
            assert_eq!(io.releases, 0);
        }
    }
    #[test]
    fn partial_start_or_movement_attempts_release_once_and_release_failure_is_error() {
        for (start, movement, release) in [
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            let mut io = Fake {
                fail_start: start,
                fail_move: movement,
                fail_release: release,
                ..Fake::default()
            };
            assert!(dispatch(
                &mut io,
                TabletModel::Remarkable2,
                Orientation::Portrait,
                NavigationDirection::Next
            )
            .is_err());
            assert_eq!(io.releases, 1);
        }
    }
}

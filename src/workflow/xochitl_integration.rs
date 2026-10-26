use anyhow::Result;
use log::debug;
use std::thread::sleep;
use std::time::Duration;

use crate::device::touch::Touch;

/// Integration with xochitl's navigation features
pub struct XochitlIntegration;

impl XochitlIntegration {
    /// Navigate to a specific page by swiping
    pub fn navigate_to_page(touch: &mut Touch, direction: NavigationDirection) -> Result<()> {
        Self::swipe(touch, direction)?;
        sleep(Duration::from_millis(500)); // Legacy, unverified-layout compatibility.
        Ok(())
    }

    /// Gesture only; completion is verified separately on supported layouts.
    pub fn swipe(touch: &mut Touch, direction: NavigationDirection) -> Result<()> {
        match direction {
            NavigationDirection::Next => Self::swipe_left(touch)?,
            NavigationDirection::Previous => Self::swipe_right(touch)?,
        }
        Ok(())
    }

    /// Swipe left (go to next page)
    fn swipe_left(touch: &mut Touch) -> Result<()> {
        debug!("Swiping left to next page");
        Self::horizontal(touch, 700, 100)
    }

    /// Swipe right (go to previous page)
    fn swipe_right(touch: &mut Touch) -> Result<()> {
        debug!("Swiping right to previous page");
        Self::horizontal(touch, 100, 700)
    }

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

/// Direction for page navigation
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationDirection {
    Next,
    Previous,
}

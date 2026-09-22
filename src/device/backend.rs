//! Device side effects shared by production and local execution.
use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use image::DynamicImage;
use std::time::Duration;

use super::{
    keyboard::Keyboard,
    pen::Pen,
    screenshot::Screenshot,
    touch::{Touch, TriggerCorner},
};
use crate::workflow::xochitl_integration::{NavigationDirection, XochitlIntegration};

#[derive(Clone, Default)]
pub struct Frame {
    pub png: Vec<u8>,
    pub details: Vec<String>,
}

impl Frame {
    pub fn base64(&self) -> String {
        STANDARD.encode(&self.png)
    }
}

pub trait DeviceBackend {
    /// Optional editing contract. Unsupported backends retain normal Reader
    /// behavior and cannot arm history. Expected text must be fully persisted.
    fn history_snapshot(
        &mut self,
        _expected: Option<&str>,
    ) -> Result<Option<crate::workflow::history::PageState>> {
        Ok(None)
    }
    fn history_mutate(
        &mut self,
        _command: crate::workflow::history::Command,
        _expected: &str,
    ) -> Result<crate::workflow::history::PageState> {
        anyhow::bail!("Native Q&A history is unavailable")
    }
    fn history_discard(&mut self) {}
    fn wait_for_interactions(
        &mut self,
        timeout: Option<Duration>,
    ) -> Result<Vec<super::interaction::Interaction>> {
        anyhow::ensure!(timeout.is_none(), "Bounded input observation unavailable");
        self.wait_for_trigger()?;
        Ok(vec![super::interaction::Interaction::Reader])
    }
    fn capture(&mut self) -> Result<Frame>;
    fn detail_images(&self) -> Result<Vec<String>>;
    fn wait_for_trigger(&mut self) -> Result<()>;
    fn dismiss_trigger(&mut self) -> Result<()>;
    fn navigate(&mut self, direction: NavigationDirection) -> Result<()>;
    fn render_text(&mut self, text: &str) -> Result<()>;
    fn body_mode(&mut self) -> Result<()>;
    fn line(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<()>;
    fn erase(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<()>;
    fn bitmap(&mut self, bitmap: &[Vec<bool>]) -> Result<()>;
    fn progress(&mut self, message: Option<&str>) -> Result<()>;
    fn status_stroke(&mut self, stroke: crate::workflow::indicator::Stroke) -> Result<()>;
    fn status_clear(&mut self, strokes: &[crate::workflow::indicator::Stroke]) -> Result<()>;
    fn monotonic(&self) -> Duration;
    fn status_suppressed(&mut self) {}
    fn load_header(&self) -> Option<DynamicImage>;
    fn save_header(&mut self, image: &DynamicImage) -> Result<()>;
    fn delay(&mut self, duration: Duration);
}

pub struct RealDevice {
    clock: std::time::Instant,
    #[cfg(target_os = "linux")]
    history: super::native_history::NativeHistory,
    screenshot: Screenshot,
    pen: Pen,
    keyboard: Keyboard,
    touch: Touch,
}

const HEADER_PATH: &str = "/var/cache/reader-buddy/header-pattern.png";

impl RealDevice {
    pub fn new(no_draw: bool, corner: TriggerCorner) -> Result<Self> {
        if let Err(error) = std::fs::create_dir_all("/var/cache/reader-buddy") {
            log::warn!("Failed to create cache directory: {error}");
        }
        Ok(Self {
            clock: std::time::Instant::now(),
            #[cfg(target_os = "linux")]
            history: super::native_history::NativeHistory::new(corner.clone()),
            screenshot: Screenshot::new()?,
            pen: Pen::new(no_draw),
            keyboard: Keyboard::new(no_draw, false),
            touch: Touch::new(no_draw, corner),
        })
    }
}

impl DeviceBackend for RealDevice {
    #[cfg(target_os = "linux")]
    fn history_snapshot(
        &mut self,
        expected: Option<&str>,
    ) -> Result<Option<crate::workflow::history::PageState>> {
        self.history.snapshot(&mut self.keyboard, expected)
    }
    #[cfg(target_os = "linux")]
    fn history_mutate(
        &mut self,
        command: crate::workflow::history::Command,
        expected: &str,
    ) -> Result<crate::workflow::history::PageState> {
        self.history.mutate(&mut self.keyboard, command, expected)
    }
    #[cfg(target_os = "linux")]
    fn history_discard(&mut self) {
        self.history.discard();
    }
    #[cfg(target_os = "linux")]
    fn wait_for_interactions(
        &mut self,
        timeout: Option<Duration>,
    ) -> Result<Vec<super::interaction::Interaction>> {
        self.history
            .wait(&mut self.keyboard, &mut self.touch, timeout)
    }
    fn capture(&mut self) -> Result<Frame> {
        self.screenshot.take_screenshot()?;
        Ok(Frame {
            png: self.screenshot.get_image_data().to_vec(),
            details: Vec::new(),
        })
    }
    fn detail_images(&self) -> Result<Vec<String>> {
        self.screenshot.detail_images_base64()
    }
    fn wait_for_trigger(&mut self) -> Result<()> {
        self.touch.wait_for_trigger()
    }
    fn dismiss_trigger(&mut self) -> Result<()> {
        self.touch.tap_middle_bottom()
    }
    fn navigate(&mut self, direction: NavigationDirection) -> Result<()> {
        #[cfg(target_os = "linux")]
        self.history.other_edit();
        XochitlIntegration::navigate_to_page(&mut self.touch, direction)
    }
    fn render_text(&mut self, text: &str) -> Result<()> {
        #[cfg(target_os = "linux")]
        self.history.note_render(text);
        self.keyboard.string_to_keypresses(text)
    }
    fn body_mode(&mut self) -> Result<()> {
        #[cfg(target_os = "linux")]
        self.history.other_edit();
        self.keyboard.key_cmd_body()
    }
    fn line(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<()> {
        self.pen.draw_path_screen(&[from, to])
    }
    fn erase(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<()> {
        self.pen.erase_rectangle(from, to)
    }
    fn bitmap(&mut self, bitmap: &[Vec<bool>]) -> Result<()> {
        self.pen.draw_bitmap(bitmap)
    }
    fn progress(&mut self, message: Option<&str>) -> Result<()> {
        match message {
            Some(text) => self.keyboard.progress(text),
            None => self.keyboard.progress_end(),
        }
    }
    fn status_stroke(&mut self, stroke: crate::workflow::indicator::Stroke) -> Result<()> {
        log::debug!("Status stroke {stroke:?} at {:?}", self.clock.elapsed());
        self.pen.draw_path_screen(&stroke.points())
    }
    fn status_clear(&mut self, strokes: &[crate::workflow::indicator::Stroke]) -> Result<()> {
        log::debug!("Clearing {} owned status paths", strokes.len());
        for stroke in strokes {
            self.pen.erase_path_screen(&stroke.points())?;
        }
        std::thread::sleep(Duration::from_millis(100));
        self.screenshot.take_screenshot()?;
        let clean = image::load_from_memory(self.screenshot.get_image_data())?;
        anyhow::ensure!(
            crate::workflow::indicator::eligible(&clean),
            "Native status cleanup left marks or the corner changed; further input stopped"
        );
        Ok(())
    }
    fn monotonic(&self) -> Duration {
        self.clock.elapsed()
    }
    fn status_suppressed(&mut self) {
        log::debug!("Status mark suppressed: occupied or unknown corner");
    }
    fn load_header(&self) -> Option<DynamicImage> {
        std::fs::read(HEADER_PATH)
            .ok()
            .and_then(|data| image::load_from_memory(&data).ok())
    }
    fn save_header(&mut self, image: &DynamicImage) -> Result<()> {
        Ok(image.save(HEADER_PATH)?)
    }
    fn delay(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

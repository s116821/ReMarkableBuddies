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
    fn load_header(&self) -> Option<DynamicImage>;
    fn save_header(&mut self, image: &DynamicImage) -> Result<()>;
    fn delay(&mut self, duration: Duration);
}

pub struct RealDevice {
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
            screenshot: Screenshot::new()?,
            pen: Pen::new(no_draw),
            keyboard: Keyboard::new(no_draw, false),
            touch: Touch::new(no_draw, corner),
        })
    }
}

impl DeviceBackend for RealDevice {
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
        XochitlIntegration::navigate_to_page(&mut self.touch, direction)
    }
    fn render_text(&mut self, text: &str) -> Result<()> {
        self.keyboard.string_to_keypresses(text)
    }
    fn body_mode(&mut self) -> Result<()> {
        self.keyboard.key_cmd_body()
    }
    fn line(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<()> {
        self.pen.draw_line_screen(from, to)
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

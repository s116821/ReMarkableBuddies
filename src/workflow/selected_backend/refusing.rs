//! Legacy Workflow calls in selected mode cannot reach the wrapped backend.
use crate::device::backend::{DeviceBackend, Frame, NavigationCompletion};
use crate::workflow::{indicator::Stroke, xochitl_integration::NavigationDirection};
use anyhow::Result;
use image::DynamicImage;
use std::time::Duration;

pub(in crate::workflow) struct RefusingBackend;
impl DeviceBackend for RefusingBackend {
    fn capture(&mut self) -> Result<Frame> {
        anyhow::bail!("Selected acquisition unsupported")
    }
    fn detail_images(&self) -> Result<Vec<String>> {
        anyhow::bail!("Selected acquisition unsupported")
    }
    fn check_request_guard(&mut self) -> Result<()> {
        anyhow::bail!("Selected operation requires context dispatch")
    }
    fn wait_for_trigger(&mut self) -> Result<()> {
        anyhow::bail!("Selected trigger unsupported")
    }
    fn prepare_reader_trigger(&mut self) -> Result<()> {
        anyhow::bail!("Selected trigger unsupported")
    }
    fn navigate(&mut self, _: NavigationDirection) -> Result<NavigationCompletion> {
        anyhow::bail!("Selected navigation requires context dispatch")
    }
    fn render_text(&mut self, _: &str) -> Result<()> {
        anyhow::bail!("Selected text requires context dispatch")
    }
    fn body_mode(&mut self) -> Result<()> {
        anyhow::bail!("Selected body mode requires context dispatch")
    }
    fn line(&mut self, _: (i32, i32), _: (i32, i32)) -> Result<()> {
        anyhow::bail!("Selected line unsupported")
    }
    fn erase(&mut self, _: (i32, i32), _: (i32, i32)) -> Result<()> {
        anyhow::bail!("Selected erase requires context dispatch")
    }
    fn bitmap(&mut self, _: &[Vec<bool>]) -> Result<()> {
        anyhow::bail!("Selected symbol requires context dispatch")
    }
    fn progress(&mut self, _: Option<&str>) -> Result<()> {
        anyhow::bail!("Selected progress requires context dispatch")
    }
    fn status_stroke(&mut self, _: Stroke) -> Result<()> {
        anyhow::bail!("Selected status lease unsupported")
    }
    fn status_clear(&mut self, _: &[Stroke]) -> Result<()> {
        anyhow::bail!("Selected status lease unsupported")
    }
    fn status_style_begin(&mut self) -> Result<bool> {
        anyhow::bail!("Selected status lease unsupported")
    }
    fn status_style_end(&mut self) -> Result<()> {
        anyhow::bail!("Selected status lease unsupported")
    }
    fn monotonic(&self) -> Duration {
        Duration::ZERO
    }
    fn load_header(&self) -> Option<DynamicImage> {
        None
    }
    fn save_header(&mut self, _: &DynamicImage) -> Result<()> {
        anyhow::bail!("Selected header persistence unsupported")
    }
    fn delay(&mut self, _: Duration) {}
}

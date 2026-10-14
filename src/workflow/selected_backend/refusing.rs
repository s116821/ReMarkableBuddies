//! Legacy Workflow calls in selected mode cannot reach the wrapped backend.
use crate::device::backend::{DeviceBackend, Frame, NavigationCompletion};
use crate::workflow::{indicator::Stroke, xochitl_integration::NavigationDirection};
use anyhow::Result;
use image::DynamicImage;
use std::time::Duration;

enum ReadOnlyTransport {
    Unavailable,
    Development(Option<Box<remarkable_open_sdk::development_capture::ReadOnlyDevelopmentCapture>>),
}

pub(in crate::workflow) struct RefusingBackend {
    transport: ReadOnlyTransport,
}
impl RefusingBackend {
    pub(in crate::workflow) fn new() -> Self {
        Self {
            transport: ReadOnlyTransport::Unavailable,
        }
    }
    pub(in crate::workflow) fn with_capture(
        capture: remarkable_open_sdk::development_capture::ReadOnlyDevelopmentCapture,
    ) -> Self {
        Self {
            transport: ReadOnlyTransport::Development(Some(Box::new(capture))),
        }
    }
}
impl DeviceBackend for RefusingBackend {
    fn acquisition_kind(&self) -> crate::device::backend::AcquisitionKind {
        match &self.transport {
            ReadOnlyTransport::Development(_) => {
                crate::device::backend::AcquisitionKind::Development
            }
            ReadOnlyTransport::Unavailable => crate::device::backend::AcquisitionKind::Unsupported,
        }
    }
    fn capture_development(
        &mut self,
    ) -> Result<remarkable_open_sdk::development_capture::ReadOnlyDevelopmentCapture> {
        match &mut self.transport {
            ReadOnlyTransport::Development(capture) => capture
                .take()
                .map(|capture| *capture)
                .ok_or_else(|| anyhow::anyhow!("Owned development capture consumed")),
            ReadOnlyTransport::Unavailable => anyhow::bail!("Development acquisition unsupported"),
        }
    }
    fn capture(&mut self) -> Result<Frame> {
        anyhow::bail!("Selected acquisition unsupported")
    }
    fn detail_images(&self) -> Result<Vec<String>> {
        anyhow::bail!("Selected acquisition unsupported")
    }
    fn check_request_guard(&mut self) -> Result<()> {
        // This check protects only immutable, already-owned transport. It never
        // calls native setup, validates a current device, or grants effect authority.
        anyhow::ensure!(
            !matches!(self.transport, ReadOnlyTransport::Unavailable),
            "Selected operation requires context dispatch"
        );
        Ok(())
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

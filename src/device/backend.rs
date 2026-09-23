//! Device side effects shared by production and local execution.
use anyhow::{Context, Result};
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
    fn navigate(&mut self, direction: NavigationDirection) -> Result<NavigationCompletion>;
    fn render_text(&mut self, text: &str) -> Result<()>;
    fn body_mode(&mut self) -> Result<()>;
    fn line(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<()>;
    fn erase(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<()>;
    fn bitmap(&mut self, bitmap: &[Vec<bool>]) -> Result<()>;
    fn progress(&mut self, message: Option<&str>) -> Result<()>;
    fn status_stroke(&mut self, stroke: crate::workflow::indicator::Stroke) -> Result<()>;
    fn status_clear(&mut self, strokes: &[crate::workflow::indicator::Stroke]) -> Result<()>;
    fn status_style_begin(&mut self) -> Result<bool> {
        Ok(true)
    }
    fn status_style_end(&mut self) -> Result<()> {
        Ok(())
    }
    fn monotonic(&self) -> Duration;
    fn status_suppressed(&mut self) {}
    fn load_header(&self) -> Option<DynamicImage>;
    fn save_header(&mut self, image: &DynamicImage) -> Result<()>;
    fn delay(&mut self, duration: Duration);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationCompletion {
    /// Composite verified-layout readiness; not a native render acknowledgement.
    Settled,
    /// No requested neighbor, or a guarded unchanged source after one attempt.
    NoMovement,
    /// Existing unsupported-layout heuristic; caller still supplies its settling wait.
    Legacy,
}

pub struct RealDevice {
    status_style: Option<super::status_style::Lease>,
    status_journal: Option<super::status_style::Journal>,
    status_style_supported: bool,
    status_wait_cancellation: super::status_style::WaitCancellation,
    #[cfg(target_os = "linux")]
    status_wait_input: Option<super::input_observer::InputObserver>,
    debug_dump: bool,
    clock: std::time::Instant,
    #[cfg(target_os = "linux")]
    history: super::native_history::NativeHistory,
    screenshot: Screenshot,
    status_screenshot: Screenshot,
    pen: Pen,
    keyboard: Keyboard,
    touch: Touch,
}

const HEADER_PATH: &str = "/var/cache/reader-buddy/header-pattern.png";

#[cfg(target_os = "linux")]
struct NativeNavigation<'a> {
    device: &'a mut RealDevice,
    input: Option<super::input_observer::InputObserver>,
}

#[cfg(target_os = "linux")]
impl super::navigation_completion::NavigationIo for NativeNavigation<'_> {
    fn observe(&mut self) -> Result<super::navigation_completion::Frame> {
        use super::native_page;
        use std::path::Path;
        let root = Path::new("/home/root/.local/share/remarkable/xochitl");
        let settings = Path::new("/home/root/.config/remarkable/xochitl.conf");
        let session = native_page::xochitl_session(Path::new("/proc"))?;
        let before = native_page::observed_navigation(root, settings, session.clone())?;
        let image = self.device.status_screenshot.take_image()?.to_luma8();
        anyhow::ensure!(
            native_page::xochitl_session(Path::new("/proc"))? == session,
            "Navigation session changed during capture"
        );
        let after = native_page::observed_navigation(root, settings, session)?;
        Ok(super::navigation_completion::Frame {
            before,
            after,
            image,
        })
    }
    fn swipe(&mut self, direction: NavigationDirection) -> Result<()> {
        XochitlIntegration::swipe(&mut self.device.touch, direction)
    }
    fn now(&self) -> Duration {
        self.device.clock.elapsed()
    }
    fn pace(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
    fn begin_guard(&mut self) -> Result<()> {
        self.input = Some(super::input_observer::InputObserver::new(
            TriggerCorner::LowerLeft,
            None,
        )?);
        Ok(())
    }
    fn guard(&mut self) -> Result<()> {
        let input = self
            .input
            .as_mut()
            .context("Navigation input observer unavailable")?;
        anyhow::ensure!(
            input.poll()?.is_empty() && input.quiescent(),
            "Input cancelled navigation ownership"
        );
        Ok(())
    }
    fn end_guard(&mut self) {
        self.input = None;
    }
}

impl RealDevice {
    /// Read-only production pre-lease readiness, exposed for bounded diagnostic
    /// examples. This does not establish a lease or emit tool/pen input.
    pub fn ready_status_observation(&mut self) -> Result<Option<super::status_style::Observation>> {
        #[cfg(target_os = "linux")]
        {
            use super::{input_observer::InputObserver, status_style::StyleIo};
            // No device mutation occurs in this scope. Observe every input source,
            // including our devices; any activity invalidates the pending baseline.
            let mut input = match InputObserver::new(TriggerCorner::LowerLeft, None) {
                Ok(input) => input,
                Err(_) => {
                    log::debug!("Status readiness input observer unavailable; suppressing status");
                    return Ok(None);
                }
            };
            let started = std::time::Instant::now();
            let debug_dump = self.debug_dump;
            super::status_readiness::wait_ready(
                || self.observe(),
                || {
                    anyhow::ensure!(
                        input.poll()?.is_empty() && input.quiescent(),
                        "Input cancelled status readiness"
                    );
                    Ok(())
                },
                || started.elapsed(),
                std::thread::sleep,
                debug_dump,
            )
        }
        #[cfg(not(target_os = "linux"))]
        Ok(None)
    }

    pub fn new(no_draw: bool, corner: TriggerCorner, debug_dump: bool) -> Result<Self> {
        anyhow::ensure!(
            !std::path::Path::new("/var/cache/reader-buddy/status-style-recovery.json")
                .try_exists()?,
            "Unresolved status style recovery; deliberate recovery is required before Reader input"
        );
        if let Err(error) = std::fs::create_dir_all("/var/cache/reader-buddy") {
            log::warn!("Failed to create cache directory: {error}");
        }
        Ok(Self {
            status_style: None,
            debug_dump,
            status_journal: None,
            status_wait_cancellation: super::status_style::WaitCancellation::default(),
            #[cfg(target_os = "linux")]
            status_wait_input: None,
            status_style_supported: !no_draw
                && std::fs::read_to_string("/etc/os-release").is_ok_and(|release| {
                    super::native_page::verified_contract(super::DeviceModel::detect(), &release)
                }),
            clock: std::time::Instant::now(),
            #[cfg(target_os = "linux")]
            history: super::native_history::NativeHistory::new(corner.clone()),
            screenshot: Screenshot::new()?,
            status_screenshot: Screenshot::new()?,
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
    fn navigate(&mut self, direction: NavigationDirection) -> Result<NavigationCompletion> {
        let _timing = crate::measurement::Span::new("device.navigation");
        #[cfg(target_os = "linux")]
        self.history.other_edit();
        #[cfg(target_os = "linux")]
        if self.status_style_supported {
            return super::navigation_completion::navigate(
                &mut NativeNavigation {
                    device: self,
                    input: None,
                },
                direction,
            );
        }
        XochitlIntegration::navigate_to_page(&mut self.touch, direction)?;
        Ok(NavigationCompletion::Legacy)
    }
    fn render_text(&mut self, text: &str) -> Result<()> {
        let _timing = crate::measurement::Span::new("device.text_input");
        log::debug!(
            "timing_text run={} characters={}",
            crate::measurement::context(),
            text.chars().count()
        );
        #[cfg(target_os = "linux")]
        self.history.note_render(text);
        self.keyboard.string_to_keypresses(text)
    }
    fn body_mode(&mut self) -> Result<()> {
        let _timing = crate::measurement::Span::new("device.body_mode");
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
        let _timing = crate::measurement::Span::new("status.stroke_input");
        log::debug!("Status stroke {stroke:?} at {:?}", self.clock.elapsed());
        self.pen.draw_path_screen(&stroke.points())
    }
    fn status_style_begin(&mut self) -> Result<bool> {
        use super::status_style::{Journal, Lease, Recovery};
        let _timing = crate::measurement::Span::new("status.acquire");
        use std::path::Path;
        const RECORD: &str = "/var/cache/reader-buddy/status-style-recovery.json";
        if self.status_style.is_some() {
            return Ok(true);
        }
        if Path::new(RECORD).try_exists()? {
            match Recovery::read(Path::new(RECORD)) {
                Ok(record) => log::error!("Unresolved status style recovery for document {}; phase {:?}, original slot {}, grid {:?}, Fineliner {:?}; deliberate recovery required", record.identity.document, record.phase, record.original_slot, record.original_grid, record.original_fine.map(|(color,width)| (super::status_style::COLORS[color],width+1))),
                Err(error) => log::error!("Invalid or incomplete status style recovery record: {error}; deliberate recovery required"),
            }
            anyhow::bail!("Unresolved status style recovery; further tablet input stopped");
        }
        if !self.status_style_supported {
            return Ok(false);
        }
        let Some(observed) = self.ready_status_observation()? else {
            log::debug!("Status readiness unavailable; no lease or input");
            return Ok(false);
        };
        let Some(mut lease) = Lease::prepare(observed, self.debug_dump)? else {
            return Ok(false);
        };
        self.status_journal = Some(Journal::create(Path::new(RECORD), &lease.recovery)?);
        let started = std::time::Instant::now();
        if let Err(error) = lease.acquire(self) {
            let restored = lease.restore(self);
            self.status_style = Some(lease);
            restored.context("Status acquisition failed and restoration could not be verified")?;
            self.status_style = None;
            self.status_journal
                .take()
                .context("Missing owned recovery journal")?
                .finish()?;
            log::warn!("Status probe declined after verified UI rollback: {error}");
            return Ok(false);
        }
        log::info!("Status style acquired in {:?}", started.elapsed());
        self.status_style = Some(lease);
        Ok(true)
    }
    fn status_style_end(&mut self) -> Result<()> {
        let _timing = crate::measurement::Span::new("status.restore");
        let Some(mut lease) = self.status_style.take() else {
            return Ok(());
        };
        let started = std::time::Instant::now();
        let restored = if lease.cleanup_pending() {
            lease.finish_cleanup(self)
        } else {
            lease.restore(self)
        };
        if let Err(error) = restored {
            self.status_style = Some(lease);
            return Err(
                error.context("Status preference restoration failed; further input stopped")
            );
        }
        self.status_journal
            .take()
            .context("Missing owned recovery journal")?
            .finish()?;
        log::info!("Status style restored in {:?}", started.elapsed());
        Ok(())
    }
    fn status_clear(&mut self, strokes: &[crate::workflow::indicator::Stroke]) -> Result<()> {
        let _timing = crate::measurement::Span::new("status.cleanup");
        let mut lease = self
            .status_style
            .take()
            .context("Cleanup requires owned style lease")?;
        let started = std::time::Instant::now();
        let restored = lease.prepare_cleanup(self);
        self.status_style = Some(lease);
        restored.context("Restore original tools before status cleanup")?;
        log::info!(
            "Status tools restored before erasure in {:?}",
            started.elapsed()
        );
        log::debug!("Clearing {} owned status paths", strokes.len());
        for stroke in strokes {
            self.pen.erase_path_screen(&stroke.points())?;
        }
        std::thread::sleep(Duration::from_millis(100));
        let clean = self.status_screenshot.take_image()?;
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
        log::debug!("Status mark suppressed: occupied, unsupported, or unknown state");
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

impl super::status_style::StyleIo for RealDevice {
    fn monotonic(&self) -> Duration {
        self.clock.elapsed()
    }
    fn pace(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
    fn begin_wait(&mut self) -> Result<()> {
        self.status_wait_cancellation.check()?;
        #[cfg(target_os = "linux")]
        {
            match super::input_observer::InputObserver::new(TriggerCorner::LowerLeft, None) {
                Ok(input) => {
                    self.status_wait_input = Some(input);
                    Ok(())
                }
                Err(error) => self.status_wait_cancellation.record(Err(error)),
            }
        }
        #[cfg(not(target_os = "linux"))]
        self.status_wait_cancellation.record(Err(anyhow::anyhow!(
            "Native status input observation unavailable"
        )))
    }
    fn check_wait(&mut self) -> Result<()> {
        self.status_wait_cancellation.check()?;
        #[cfg(target_os = "linux")]
        {
            let result = (|| {
                let input = self
                    .status_wait_input
                    .as_mut()
                    .context("Status wait observer missing")?;
                anyhow::ensure!(
                    input.poll()?.is_empty() && input.quiescent(),
                    "Input cancelled status transition"
                );
                Ok(())
            })();
            self.status_wait_cancellation.record(result)
        }
        #[cfg(not(target_os = "linux"))]
        self.status_wait_cancellation.record(Err(anyhow::anyhow!(
            "Native status input observation unavailable"
        )))
    }
    fn end_wait(&mut self) {
        #[cfg(target_os = "linux")]
        {
            self.status_wait_input = None;
        }
    }
    fn checkpoint(&mut self, record: &super::status_style::Recovery) -> Result<()> {
        self.status_journal
            .as_mut()
            .context("Missing owned recovery journal")?
            .append(record)
    }
    fn observe(&mut self) -> Result<super::status_style::Observation> {
        let _timing = crate::measurement::Span::new("status.observe");
        self.status_wait_cancellation.check()?;
        use super::{
            native_page,
            status_style::{Identity, Observation},
        };
        use std::{io::Read, path::Path};
        let root = Path::new("/home/root/.local/share/remarkable/xochitl");
        let settings = Path::new("/home/root/.config/remarkable/xochitl.conf");
        let session = native_page::xochitl_session(Path::new("/proc"))?;
        let owner = native_page::observed_owner(root, settings, session.clone())?;
        let image = self.status_screenshot.take_image()?.to_luma8();
        let mut bytes = Vec::new();
        std::fs::File::open(root.join(format!("{}.content", owner.document)))?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() <= 1024 * 1024,
            "Status document metadata exceeds bound"
        );
        let content: serde_json::Value = serde_json::from_slice(&bytes)?;
        let preferences = serde_json::from_value(content["extraMetadata"].clone())?;
        let after_session = native_page::xochitl_session(Path::new("/proc"))?;
        anyhow::ensure!(
            after_session == session
                && native_page::observed_owner(root, settings, after_session)? == owner,
            "Page changed during status observation"
        );
        Ok(Observation {
            identity: Identity {
                document: owner.document,
                page: owner.page,
                visit: owner.visit,
                session: owner.session,
            },
            preferences,
            image,
        })
    }
    fn press(&mut self, point: (u32, u32)) -> Result<()> {
        let _timing = crate::measurement::Span::new("status.press_input");
        let down = self.touch.touch_start((point.0 as i32, point.1 as i32));
        if down.is_ok() {
            std::thread::sleep(Duration::from_millis(250));
        }
        let up = self.touch.touch_stop();
        down?;
        up?;
        Ok(())
    }
}

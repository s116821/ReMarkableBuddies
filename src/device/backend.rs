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

#[cfg(target_os = "linux")]
struct NativeStatusObservation<'a>(&'a mut RealDevice);

#[cfg(target_os = "linux")]
impl super::capture_recovery::ObservationIo for NativeStatusObservation<'_> {
    type Owner = crate::workflow::history::Owner;
    type Frame = super::status_style::Observation;
    fn now(&self) -> Duration {
        self.0.clock.elapsed()
    }
    fn owner(&mut self) -> Result<Self::Owner> {
        let _timing = crate::measurement::Span::new("status.capture.owner_guard");
        use std::path::Path;
        let session = super::native_page::xochitl_session(Path::new("/proc"))?;
        super::native_page::observed_owner(
            Path::new("/home/root/.local/share/remarkable/xochitl"),
            Path::new("/home/root/.config/remarkable/xochitl.conf"),
            session,
        )
    }
    fn check_input(&mut self) -> Result<()> {
        super::status_style::StyleIo::check_wait(self.0)
    }
    fn capture(&mut self) -> Result<Self::Frame> {
        self.0.observe_status_once()
    }
}

impl RealDevice {
    fn observe_status_once(&mut self) -> Result<super::status_style::Observation> {
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

    /// Diagnostic alias: exercises the same selected-tool path as Reader.
    pub fn current_tool_probe(corner: TriggerCorner, debug_dump: bool) -> Result<Self> {
        Self::new(false, corner, debug_dump)
    }

    fn guard_current_input(&mut self, erasing: bool, points: &[(i32, i32)]) -> Result<()> {
        let _timing = crate::measurement::Span::new("status.path.guard");
        use crate::workflow::indicator::{BOTTOM, LEFT, RIGHT, TOP};
        anyhow::ensure!(
            !points.is_empty()
                && points
                    .iter()
                    .all(|&(x, y)| (LEFT + 8..=RIGHT - 8).contains(&x)
                        && (TOP + 8..=BOTTOM - 8).contains(&y)),
            "Current-tool input exceeds reserved status geometry"
        );
        use super::status_style::StyleIo;
        let lease = self
            .status_style
            .take()
            .context("Current-tool input requires a lease")?;
        let result = (|| {
            // begin_wait reuses an existing observer: pending events from the
            // provider/cadence interval must be checked before it is discarded.
            self.begin_wait()?;
            self.check_wait()?;
            if erasing {
                lease.verify_current_erasure(self)?;
            } else {
                lease.verify_current(self)?;
            }
            self.check_wait()
        })();
        self.status_style = Some(lease);
        self.status_wait_cancellation.record(result)?;
        // Retain descriptors, but authorize a narrowly identified owned source
        // only for the immediate injection/release interval. Other sources keep
        // every pending event; the same physical pen remains unattributable here.
        #[cfg(target_os = "linux")]
        {
            let result = (|| {
                let writer = self.pen.input_identity()?;
                self.status_wait_input
                    .as_mut()
                    .context("Status observer missing before injection")?
                    .begin_owned_pen(writer)
            })();
            self.status_wait_cancellation.record(result)?;
        }
        Ok(())
    }
    fn rearm_current_input(&mut self) -> Result<()> {
        let _timing = crate::measurement::Span::new("status.path.rearm");
        {
            use super::status_style::StyleIo;
            #[cfg(target_os = "linux")]
            if let Some(observer) = self.status_wait_input.as_mut() {
                if observer.has_owned_pen() {
                    let result = observer.finish_owned_pen();
                    self.status_wait_cancellation.record(result)?;
                }
            }
            self.begin_wait()?;
            self.check_wait()?;
        }
        Ok(())
    }
    fn inject_status_path(&mut self, points: &[(i32, i32)], erasing: bool) -> Result<()> {
        self.guard_current_input(erasing, points)?;
        let inject = |device: &mut Self| {
            let _timing = crate::measurement::Span::new("status.path.inject");
            if erasing {
                device.pen.erase_path_screen(points)
            } else {
                device.pen.draw_path_screen(points)
            }
        };
        super::status_style::guarded_injection(
            self,
            |device| &mut device.status_wait_cancellation,
            inject,
            Self::rearm_current_input,
        )
    }
    /// Read-only production pre-lease readiness, exposed for bounded diagnostic
    /// examples. This does not establish a lease or emit tool/pen input.
    pub fn ready_status_observation(&mut self) -> Result<Option<super::status_style::Observation>> {
        #[cfg(target_os = "linux")]
        {
            use super::{input_observer::InputObserver, status_style::StyleIo};
            // No device mutation occurs in this scope. Observe every input source,
            // including our devices; any activity invalidates the pending baseline.
            let mut input = InputObserver::new(TriggerCorner::LowerLeft, None)
                .context("Status readiness input observation failed")?;
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
        self.inject_status_path(&[from, to], false)
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
        let points = stroke.points();
        self.inject_status_path(&points, false)
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
        let prepared = Lease::prepare_current(observed, self.debug_dump)?;
        let Some(mut lease) = prepared else {
            return Ok(false);
        };
        self.status_journal = Some(Journal::create(Path::new(RECORD), &lease.recovery)?);
        let started = std::time::Instant::now();
        if let Err(error) = lease.acquire(self) {
            self.status_style = Some(lease);
            self.status_wait_cancellation.latch();
            return Err(error.context("Status acquisition failed; recovery record retained"));
        }
        log::info!("Status style acquired in {:?}", started.elapsed());
        self.status_style = Some(lease);
        self.rearm_current_input()?;
        Ok(true)
    }
    fn status_style_end(&mut self) -> Result<()> {
        let _timing = crate::measurement::Span::new("status.restore");
        if self.status_style.is_some() {
            self.rearm_current_input()?;
        }
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
        {
            use super::status_style::StyleIo;
            if let Err(error) = self.check_wait() {
                self.status_style = Some(lease);
                return Err(error);
            }
        }
        self.status_journal
            .take()
            .context("Missing owned recovery journal")?
            .finish()?;
        #[cfg(target_os = "linux")]
        {
            self.status_wait_input = None;
        }
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
        let restored = {
            let _timing = crate::measurement::Span::new("status.cleanup.restore_tools");
            lease.prepare_cleanup(self)
        };
        self.status_style = Some(lease);
        restored.context("Restore original tools before status cleanup")?;
        log::info!(
            "Status tools restored before erasure in {:?}",
            started.elapsed()
        );
        log::debug!("Clearing {} owned status paths", strokes.len());
        let _erasure_timing = crate::measurement::Span::new("status.cleanup.erase_and_verify");
        for stroke in strokes {
            let points = stroke.points();
            self.inject_status_path(&points, true)?;
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
            if self.status_wait_input.is_some() {
                return self.check_wait();
            }
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
        let _timing = crate::measurement::Span::new("status.input_guard");
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
        // Preserve pending events across the entire current-tool lease.
    }
    fn checkpoint(&mut self, record: &super::status_style::Recovery) -> Result<()> {
        let _timing = crate::measurement::Span::new("status.checkpoint");
        self.status_journal
            .as_mut()
            .context("Missing owned recovery journal")?
            .append(record)
    }
    fn observe(&mut self) -> Result<super::status_style::Observation> {
        #[cfg(target_os = "linux")]
        if self.status_wait_input.is_some() {
            let result = super::capture_recovery::observe(&mut NativeStatusObservation(self));
            if result.is_err() {
                self.status_wait_cancellation.latch();
            }
            return result;
        }
        // No established observer means no recovery, including early prelease
        // observations. Never create/reset an observer to qualify a retry.
        self.observe_status_once()
    }
    fn press(&mut self, _: (u32, u32)) -> Result<()> {
        anyhow::bail!("Native status leases forbid menu input")
    }
}

use anyhow::Result;
use log::info;

#[cfg(target_os = "linux")]
use std::thread::sleep;

use std::time::Duration;

#[cfg(target_os = "linux")]
use evdev::{Device, EventType as EvdevEventType, InputEvent};

use super::DeviceModel;

#[derive(Default)]
pub(crate) struct HoldTimer {
    start: Option<Duration>,
}

impl HoldTimer {
    pub(crate) fn reset(&mut self) {
        self.start = None;
    }
    pub(crate) fn contact(&mut self, active: bool, now: Duration) {
        if active {
            self.start.get_or_insert(now);
        } else {
            self.reset();
        }
    }
    pub(crate) fn triggered(&self, now: Duration) -> bool {
        self.start
            .is_some_and(|start| now.saturating_sub(start) >= Duration::from_secs(2))
    }
    pub(crate) fn deadline(&self) -> Option<Duration> {
        self.start.map(|start| start + Duration::from_secs(2))
    }
}

#[derive(Debug, Clone)]
pub enum TriggerCorner {
    UpperRight,
    UpperLeft,
    LowerRight,
    LowerLeft,
}

impl TriggerCorner {
    pub(crate) fn contains(&self, x: i32, y: i32) -> bool {
        match self {
            Self::UpperRight => x > 768 - 68 && y < 68,
            Self::UpperLeft => x < 68 && y < 68,
            Self::LowerRight => x > 768 - 68 && y > 1024 - 68,
            Self::LowerLeft => x < 68 && y > 1024 - 68,
        }
    }
    pub fn from_string(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "ur" | "upper-right" => Ok(TriggerCorner::UpperRight),
            "ul" | "upper-left" => Ok(TriggerCorner::UpperLeft),
            "lr" | "lower-right" => Ok(TriggerCorner::LowerRight),
            "ll" | "lower-left" => Ok(TriggerCorner::LowerLeft),
            _ => Err(anyhow::anyhow!(
                "Invalid trigger corner: {}. Use UR, UL, LR, LL, upper-right, upper-left, lower-right, or lower-left",
                s
            )),
        }
    }
}

// Output dimensions remain the same for both devices (only used on Linux)
#[cfg(target_os = "linux")]
const VIRTUAL_WIDTH: u16 = 768;
#[cfg(target_os = "linux")]
const VIRTUAL_HEIGHT: u16 = 1024;

// Event codes (only used on Linux)
#[cfg(target_os = "linux")]
const ABS_MT_SLOT: u16 = 47;
#[cfg(target_os = "linux")]
const ABS_MT_TOUCH_MAJOR: u16 = 48;
#[cfg(target_os = "linux")]
const ABS_MT_TOUCH_MINOR: u16 = 49;
#[cfg(target_os = "linux")]
const ABS_MT_ORIENTATION: u16 = 52;
#[cfg(target_os = "linux")]
const ABS_MT_POSITION_X: u16 = 53;
#[cfg(target_os = "linux")]
const ABS_MT_POSITION_Y: u16 = 54;
#[cfg(target_os = "linux")]
const ABS_MT_TRACKING_ID: u16 = 57;
#[cfg(target_os = "linux")]
const ABS_MT_PRESSURE: u16 = 58;

#[cfg(target_os = "linux")]
pub struct Touch {
    device: Option<Device>,
    #[cfg(test)]
    batch_fixture: Option<Box<dyn FnMut(&[InputEvent]) -> Result<()>>>,
    #[cfg(feature = "development-input-diagnostics")]
    diagnostic: Option<(i32, Box<dyn DiagnosticGestureEmitter>)>,
    device_model: DeviceModel,
    trigger_corner: TriggerCorner,
}

#[cfg(not(target_os = "linux"))]
pub struct Touch {
    _device_model: DeviceModel,
    _trigger_corner: TriggerCorner,
}

/// Development-only emission boundary. Implementations must qualify and perform
/// each batch, including conditional release; this never grants UI authority.
#[cfg(all(target_os = "linux", feature = "development-input-diagnostics"))]
pub trait DiagnosticGestureEmitter {
    fn emit(&mut self, device: &mut Device, events: &[InputEvent]) -> Result<()>;
}

#[cfg(target_os = "linux")]
impl Touch {
    #[cfg(feature = "development-input-diagnostics")]
    pub fn diagnostic_scoped_gesture(
        tracking: i32,
        emitter: Box<dyn DiagnosticGestureEmitter>,
    ) -> Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        anyhow::ensure!(
            (1..=65535).contains(&tracking),
            "Invalid diagnostic contact tag"
        );
        let model = DeviceModel::detect();
        anyhow::ensure!(
            model == DeviceModel::Remarkable2
                && super::native_page::verified_contract(
                    model,
                    &std::fs::read_to_string("/usr/lib/os-release")?
                ),
            "Unqualified scoped gesture device/firmware"
        );
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
            .open("/dev/input/event2")?;
        let device = Device::from_fd(file.into())?;
        Ok(Self {
            device: Some(device),
            #[cfg(test)]
            batch_fixture: None,
            diagnostic: Some((tracking, emitter)),
            device_model: model,
            trigger_corner: TriggerCorner::LowerLeft,
        })
    }

    #[cfg(feature = "development-input-diagnostics")]
    pub fn diagnostic_native_point(&self, point: (i32, i32)) -> (i32, i32) {
        self.virtual_to_input(point)
    }

    fn output_tracking_id(&self) -> i32 {
        #[cfg(feature = "development-input-diagnostics")]
        if let Some((tracking, _)) = &self.diagnostic {
            return *tracking;
        }
        1
    }

    fn has_output(&self) -> bool {
        #[cfg(test)]
        if self.batch_fixture.is_some() {
            return true;
        }
        self.device.is_some()
    }

    fn emit_batch(&mut self, events: &[InputEvent]) -> Result<()> {
        #[cfg(test)]
        if let Some(sink) = &mut self.batch_fixture {
            return sink(events);
        }
        if let Some(device) = &mut self.device {
            #[cfg(feature = "development-input-diagnostics")]
            if let Some((_, emitter)) = &mut self.diagnostic {
                return emitter.emit(device, events);
            }
            device.send_events(events)?;
        }
        Ok(())
    }

    /// Bounded complete-event diagnostic. Never proves UI acknowledgement.
    #[cfg(feature = "development-input-diagnostics")]
    pub fn diagnostic_tap_echo_raw(
        &mut self,
        point: (i32, i32),
        directory: &std::path::Path,
    ) -> Result<()> {
        anyhow::ensure!(
            (1..768).contains(&point.0) && (1..1024).contains(&point.1),
            "Diagnostic point must be strictly inside the virtual screen"
        );
        let model = DeviceModel::detect();
        anyhow::ensure!(
            model == DeviceModel::Remarkable2
                && self.device_model == model
                && super::native_page::verified_contract(
                    model,
                    &std::fs::read_to_string("/etc/os-release")?
                ),
            "Unqualified touch diagnostic firmware/device/transform"
        );
        let mut evidence = super::tap_echo_raw::Evidence::new(directory)?;
        let native = self.native_point(point);
        let mut input = super::input_observer::InputObserver::new(TriggerCorner::LowerLeft, None)?;
        let writer = self.input_identity()?;
        // Attach only after construction: its initial reads are not per-tap evidence.
        input.raw_evidence = Some(evidence);
        let result = (|| {
            input.begin_owned_touch(writer, native)?;
            let down = self.touch_start(point);
            if down.is_ok() {
                sleep(Duration::from_millis(100));
            }
            let release = self.touch_stop();
            let finish = input.finish_owned_touch();
            match (down.and(release), finish) {
                (Err(error), Err(finish)) => {
                    Err(error.context(format!("Touch echo observation also failed: {finish:#}")))
                }
                (Err(error), _) | (_, Err(error)) => Err(error),
                (Ok(()), Ok(())) => Ok(()),
            }
        })();
        evidence = input
            .raw_evidence
            .take()
            .expect("diagnostic evidence retained");
        let persistence = evidence.persist(point, native, &result);
        match (result, persistence) {
            (Err(error), Err(persist)) => {
                Err(error.context(format!("Raw evidence persistence failed: {persist:#}")))
            }
            (Err(error), _) | (_, Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    /// One development contact observed through the production owned-touch watch.
    /// Success is evdev echo evidence, never UI acknowledgement.
    #[cfg(feature = "development-input-diagnostics")]
    pub fn diagnostic_tap_echo(&mut self, point: (i32, i32)) -> Result<()> {
        anyhow::ensure!(
            (1..768).contains(&point.0) && (1..1024).contains(&point.1),
            "Diagnostic point must be strictly inside the virtual screen"
        );
        let model = DeviceModel::detect();
        anyhow::ensure!(
            model == DeviceModel::Remarkable2
                && self.device_model == model
                && super::native_page::verified_contract(
                    model,
                    &std::fs::read_to_string("/etc/os-release")?
                ),
            "Unqualified touch diagnostic firmware/device/transform"
        );
        let writer = self.input_identity()?;
        let mut input = super::input_observer::InputObserver::new(TriggerCorner::LowerLeft, None)?;
        input.begin_owned_touch(writer, self.native_point(point))?;
        let down = self.touch_start(point);
        if down.is_ok() {
            sleep(Duration::from_millis(100));
        }
        // Evaluate both cleanup operations even if down partially failed.
        let release = self.touch_stop();
        let finish = input.finish_owned_touch();
        match (down.and(release), finish) {
            (Err(error), Err(finish)) => {
                Err(error.context(format!("Touch echo observation also failed: {finish:#}")))
            }
            (Err(error), _) | (_, Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    pub(super) fn input_identity(&self) -> Result<super::input_observer::DescriptorIdentity> {
        use std::os::fd::AsFd;
        super::input_observer::descriptor_identity(
            self.device
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Touch input is disabled"))?
                .as_fd(),
        )
    }

    pub(super) fn native_point(&self, point: (i32, i32)) -> (i32, i32) {
        self.virtual_to_input(point)
    }

    pub fn new(no_touch: bool, trigger_corner: TriggerCorner) -> Self {
        let device_model = DeviceModel::detect();
        info!("Touch using device model: {}", device_model.name());

        let device_path = match device_model {
            DeviceModel::Remarkable2 => "/dev/input/event2",
            DeviceModel::RemarkablePaperPro => "/dev/input/event3",
            DeviceModel::Unknown => "/dev/input/event2", // Default to RM2
        };

        let device = if no_touch {
            None
        } else {
            Some(Device::open(device_path).unwrap())
        };

        Self {
            device,
            #[cfg(test)]
            batch_fixture: None,
            #[cfg(feature = "development-input-diagnostics")]
            diagnostic: None,
            device_model,
            trigger_corner,
        }
    }

    pub fn wait_for_trigger(&mut self) -> Result<()> {
        use std::time::Instant;
        let device = self
            .device
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Touch trigger is disabled"))?;
        device.set_nonblocking(true)?;
        let mut position = (0, 0);
        let mut slot = 0;
        let mut touching = false;
        let clock = Instant::now();
        let mut hold = HoldTimer::default();
        info!("Waiting for 2s hold in trigger zone...");
        loop {
            let events: Vec<_> = match self.device.as_mut().unwrap().fetch_events() {
                Ok(events) => events.collect(),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Vec::new(),
                Err(e) => return Err(e.into()),
            };
            for event in events {
                if event.event_type() == EvdevEventType::ABSOLUTE {
                    if event.code() == ABS_MT_SLOT {
                        slot = event.value();
                    }
                    if slot == 0 {
                        match event.code() {
                            ABS_MT_POSITION_X => position.0 = event.value(),
                            ABS_MT_POSITION_Y => position.1 = event.value(),
                            ABS_MT_TRACKING_ID => {
                                touching = event.value() >= 0;
                                hold.reset();
                            }
                            _ => {}
                        }
                    }
                }
                // Evaluate coordinates only after the complete input frame.
                if event.event_type() == EvdevEventType::SYNCHRONIZATION && event.code() == 0 {
                    let (x, y) = self.input_to_virtual(position);
                    if touching && self.is_in_trigger_zone(x, y) {
                        hold.contact(true, clock.elapsed());
                    } else {
                        hold.reset();
                    }
                }
            }
            if hold.triggered(clock.elapsed()) {
                info!("Trigger activated after 2s hold!");
                return Ok(());
            }
            sleep(Duration::from_millis(10));
        }
    }
    pub fn touch_start(&mut self, xy: (i32, i32)) -> Result<()> {
        let (x, y) = self.virtual_to_input(xy);
        if self.has_output() {
            let tracking = self.output_tracking_id();
            self.emit_batch(&[
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_SLOT, 0),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_TRACKING_ID, tracking),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_POSITION_X, x),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_POSITION_Y, y),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_PRESSURE, 100),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_TOUCH_MAJOR, 17),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_TOUCH_MINOR, 17),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_ORIENTATION, 4),
                InputEvent::new(EvdevEventType::SYNCHRONIZATION.0, 0, 0), // SYN_REPORT
            ])?;
            sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    pub fn touch_stop(&mut self) -> Result<()> {
        if self.has_output() {
            self.emit_batch(&[
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_SLOT, 0),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_TRACKING_ID, -1),
                InputEvent::new(EvdevEventType::SYNCHRONIZATION.0, 0, 0), // SYN_REPORT
            ])?;
            sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    pub fn goto_xy(&mut self, xy: (i32, i32)) -> Result<()> {
        let (x, y) = self.virtual_to_input(xy);
        if self.has_output() {
            let tracking = self.output_tracking_id();
            self.emit_batch(&[
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_SLOT, 0),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_TRACKING_ID, tracking),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_POSITION_X, x),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_POSITION_Y, y),
                InputEvent::new(EvdevEventType::SYNCHRONIZATION.0, 0, 0), // SYN_REPORT
            ])?;
        }
        Ok(())
    }

    fn is_in_trigger_zone(&self, x: i32, y: i32) -> bool {
        self.trigger_corner.contains(x, y)
    }

    fn screen_width(&self) -> u32 {
        match self.device_model {
            DeviceModel::Remarkable2 => 1404,
            DeviceModel::RemarkablePaperPro => 2065,
            DeviceModel::Unknown => 1404, // Default to RM2
        }
    }

    fn screen_height(&self) -> u32 {
        match self.device_model {
            DeviceModel::Remarkable2 => 1872,
            DeviceModel::RemarkablePaperPro => 2833,
            DeviceModel::Unknown => 1872, // Default to RM2
        }
    }

    fn virtual_to_input(&self, (x, y): (i32, i32)) -> (i32, i32) {
        // Swap and normalize the coordinates
        let x_normalized = x as f32 / VIRTUAL_WIDTH as f32;
        let y_normalized = y as f32 / VIRTUAL_HEIGHT as f32;

        match self.device_model {
            DeviceModel::RemarkablePaperPro => {
                let x_input = (x_normalized * self.screen_width() as f32) as i32;
                let y_input = (y_normalized * self.screen_height() as f32) as i32;
                (x_input, y_input)
            }
            _ => {
                // RM2 coordinate transformation
                let x_input = (x_normalized * self.screen_width() as f32) as i32;
                let y_input = ((1.0 - y_normalized) * self.screen_height() as f32) as i32;
                (x_input, y_input)
            }
        }
    }

    fn input_to_virtual(&self, (x, y): (i32, i32)) -> (i32, i32) {
        // Swap and normalize the coordinates
        let x_normalized = x as f32 / self.screen_width() as f32;
        let y_normalized = y as f32 / self.screen_height() as f32;

        match self.device_model {
            DeviceModel::RemarkablePaperPro => {
                let x_input = (x_normalized * VIRTUAL_WIDTH as f32) as i32;
                let y_input = (y_normalized * VIRTUAL_HEIGHT as f32) as i32;
                (x_input, y_input)
            }
            _ => {
                // RM2 coordinate transformation
                let x_input = (x_normalized * VIRTUAL_WIDTH as f32) as i32;
                let y_input = ((1.0 - y_normalized) * VIRTUAL_HEIGHT as f32) as i32;
                (x_input, y_input)
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
impl Touch {
    pub fn new(_no_touch: bool, trigger_corner: TriggerCorner) -> Self {
        let device_model = DeviceModel::detect();
        info!("Touch using device model: {}", device_model.name());

        Self {
            _device_model: device_model,
            _trigger_corner: trigger_corner,
        }
    }

    pub fn wait_for_trigger(&mut self) -> Result<()> {
        Ok(())
    }

    pub fn touch_start(&mut self, _xy: (i32, i32)) -> Result<()> {
        Ok(())
    }

    pub fn touch_stop(&mut self) -> Result<()> {
        Ok(())
    }

    pub fn goto_xy(&mut self, _xy: (i32, i32)) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod hold_tests {
    use super::{HoldTimer, TriggerCorner};
    use std::time::Duration;

    #[cfg(target_os = "linux")]
    #[test]
    fn proposed_diagnostic_point_uses_exact_existing_f32_mapping() {
        let touch = super::Touch {
            device: None,
            #[cfg(test)]
            batch_fixture: None,
            #[cfg(feature = "development-input-diagnostics")]
            diagnostic: None,
            device_model: super::DeviceModel::Remarkable2,
            trigger_corner: TriggerCorner::LowerLeft,
        };
        assert_eq!(touch.native_point((110, 280)), (201, 1360));
    }

    #[cfg(all(target_os = "linux", feature = "development-input-diagnostics"))]
    #[test]
    fn unchanged_swipe_executes_every_tagged_batch_and_failed_start_stop_guard() {
        use crate::workflow::xochitl_integration::{NavigationDirection, XochitlIntegration};
        use std::{cell::RefCell, rc::Rc};
        for direction in [NavigationDirection::Next, NavigationDirection::Previous] {
            let batches = Rc::new(RefCell::new(Vec::new()));
            let sink = batches.clone();
            let mut touch = super::Touch {
                device: None,
                batch_fixture: Some(Box::new(move |events| {
                    sink.borrow_mut().push(
                        events
                            .iter()
                            .map(|e| [e.event_type().0 as i32, e.code() as i32, e.value()])
                            .collect::<Vec<_>>(),
                    );
                    Ok(())
                })),
                diagnostic: Some((731, Box::new(UnusedEmitter))),
                device_model: super::DeviceModel::Remarkable2,
                trigger_corner: TriggerCorner::LowerLeft,
            };
            XochitlIntegration::swipe(&mut touch, direction).unwrap();
            let observed = batches.borrow();
            assert_eq!(observed.len(), 17);
            let (a, b) = if direction == NavigationDirection::Next {
                (700i32, 100i32)
            } else {
                (100, 700)
            };
            for (step, events) in observed[..16].iter().enumerate() {
                let offset = (((b - a).abs() as f32) * (step as f32 / 15.0)) as i32;
                let (x, y) = touch.virtual_to_input((a + (b - a).signum() * offset, 512));
                assert_eq!(events[0], [3, 47, 0]);
                assert_eq!(events[1], [3, 57, 731]);
                assert_eq!(events[2], [3, 53, x]);
                assert_eq!(events[3], [3, 54, y]);
                assert_eq!(y, 936);
                assert_eq!(events.last(), Some(&[0, 0, 0]));
                assert_eq!(events.len(), if step == 0 { 9 } else { 5 });
            }
            assert_eq!(observed[16], vec![[3, 47, 0], [3, 57, -1], [0, 0, 0]]);
        }
        let calls = Rc::new(RefCell::new(Vec::new()));
        let sink = calls.clone();
        let mut touch = super::Touch {
            device: None,
            batch_fixture: Some(Box::new(move |events| {
                sink.borrow_mut().push(events.len());
                anyhow::bail!("Unstarted guard refuses every actual write")
            })),
            diagnostic: Some((731, Box::new(UnusedEmitter))),
            device_model: super::DeviceModel::Remarkable2,
            trigger_corner: TriggerCorner::LowerLeft,
        };
        assert!(XochitlIntegration::swipe(&mut touch, NavigationDirection::Next).is_err());
        assert_eq!(*calls.borrow(), vec![9, 3]); // Guard still called on caller's conditional stop.
    }
    #[cfg(all(target_os = "linux", feature = "development-input-diagnostics"))]
    struct UnusedEmitter;
    #[cfg(all(target_os = "linux", feature = "development-input-diagnostics"))]
    impl super::DiagnosticGestureEmitter for UnusedEmitter {
        fn emit(&mut self, _: &mut evdev::Device, _: &[evdev::InputEvent]) -> anyhow::Result<()> {
            panic!("Test fixture never opens or writes an input device")
        }
    }
    #[test]
    fn stationary_contact_triggers_at_threshold_not_before() {
        let mut hold = HoldTimer::default();
        hold.contact(true, Duration::from_secs(1));
        assert!(!hold.triggered(Duration::from_millis(2999)));
        assert!(hold.triggered(Duration::from_secs(3)));
    }

    #[test]
    fn leaving_zone_release_and_new_tracking_id_reset_hold() {
        let mut hold = HoldTimer::default();
        hold.contact(true, Duration::ZERO);
        hold.contact(false, Duration::from_millis(1999));
        assert!(!hold.triggered(Duration::from_secs(5)));
        hold.contact(true, Duration::from_secs(6));
        assert!(!hold.triggered(Duration::from_secs(7)));
        assert!(hold.triggered(Duration::from_secs(8)));
        hold.reset();
        assert!(!hold.triggered(Duration::from_secs(10)));
    }

    #[test]
    fn repeated_position_frames_do_not_restart_timer() {
        let mut hold = HoldTimer::default();
        hold.contact(true, Duration::ZERO);
        hold.contact(true, Duration::from_secs(1));
        assert!(hold.triggered(Duration::from_secs(2)));
        for value in ["LL", "upper-right", "ul", "LOWER-RIGHT"] {
            assert!(TriggerCorner::from_string(value).is_ok());
        }
        assert!(TriggerCorner::from_string("middle").is_err());
    }
}

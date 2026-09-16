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
    device_model: DeviceModel,
    trigger_corner: TriggerCorner,
}

#[cfg(not(target_os = "linux"))]
pub struct Touch {
    _device_model: DeviceModel,
    _trigger_corner: TriggerCorner,
}

#[cfg(target_os = "linux")]
impl Touch {
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
        if let Some(device) = &mut self.device {
            device.send_events(&[
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_SLOT, 0),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_TRACKING_ID, 1),
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
        if let Some(device) = &mut self.device {
            device.send_events(&[
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
        if let Some(device) = &mut self.device {
            device.send_events(&[
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_SLOT, 0),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_TRACKING_ID, 1),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_POSITION_X, x),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, ABS_MT_POSITION_Y, y),
                InputEvent::new(EvdevEventType::SYNCHRONIZATION.0, 0, 0), // SYN_REPORT
            ])?;
        }
        Ok(())
    }

    pub fn tap_middle_bottom(&mut self) -> Result<()> {
        self.touch_start((384, 1023))?; // middle bottom
        sleep(Duration::from_millis(100));
        self.touch_stop()?;
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

    pub fn tap_middle_bottom(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod hold_tests {
    use super::{HoldTimer, TriggerCorner};
    use std::time::Duration;

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

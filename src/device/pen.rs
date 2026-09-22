use anyhow::Result;
use log::info;

#[cfg(target_os = "linux")]
use std::thread::sleep;

#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
use evdev::{Device, EventType as EvdevEventType, InputEvent};

use super::DeviceModel;

// Output dimensions remain the same for both devices (only used on Linux)
#[cfg(target_os = "linux")]
const VIRTUAL_WIDTH: u32 = 768;
#[cfg(target_os = "linux")]
const VIRTUAL_HEIGHT: u32 = 1024;

#[cfg(target_os = "linux")]
pub struct Pen {
    device: Option<Device>,
    device_model: DeviceModel,
}

#[cfg(not(target_os = "linux"))]
pub struct Pen {
    _device_model: DeviceModel,
}

#[cfg(target_os = "linux")]
impl Pen {
    pub fn new(no_draw: bool) -> Self {
        let device_model = DeviceModel::detect();
        info!("Pen using device model: {}", device_model.name());

        let pen_input_device = match device_model {
            DeviceModel::Remarkable2 => "/dev/input/event1",
            DeviceModel::RemarkablePaperPro => "/dev/input/event2",
            DeviceModel::Unknown => "/dev/input/event1", // Default to RM2
        };

        let device = if no_draw {
            None
        } else {
            Some(Device::open(pen_input_device).unwrap())
        };

        Self {
            device,
            device_model,
        }
    }

    pub fn draw_line_screen(&mut self, p1: (i32, i32), p2: (i32, i32)) -> Result<()> {
        self.draw_line(self.virtual_to_input(p1), self.virtual_to_input(p2))
    }

    pub fn draw_path_screen(&mut self, points: &[(i32, i32)]) -> Result<()> {
        if let Some(&first) = points.first() {
            let result = (|| -> Result<()> {
                self.pen_up()?;
                self.pen_down_at(self.virtual_to_input(first))?;
                self.follow_path_screen(points)
            })();
            let release = self.pen_up();
            result?;
            release?;
        }
        Ok(())
    }

    pub fn erase_path_screen(&mut self, points: &[(i32, i32)]) -> Result<()> {
        if let Some(&first) = points.first() {
            let result = (|| -> Result<()> {
                self.pen_up()?;
                self.eraser_up()?;
                self.eraser_down_at(first)?;
                self.follow_path_screen(points)
            })();
            let release = self.eraser_up();
            result?;
            release?;
        }
        Ok(())
    }

    fn follow_path_screen(&mut self, points: &[(i32, i32)]) -> Result<()> {
        for pair in points.windows(2) {
            let previous = self.virtual_to_input(pair[0]);
            let next = self.virtual_to_input(pair[1]);
            let length = ((next.0 - previous.0) as f64).hypot((next.1 - previous.1) as f64);
            let steps = (length / 5.0).ceil().max(1.0) as i32;
            for step in 1..=steps {
                self.goto_xy((
                    previous.0 + (next.0 - previous.0) * step / steps,
                    previous.1 + (next.1 - previous.1) * step / steps,
                ))?;
            }
        }
        Ok(())
    }

    pub fn draw_line(&mut self, (x1, y1): (i32, i32), (x2, y2): (i32, i32)) -> Result<()> {
        let length = ((x2 as f32 - x1 as f32).powf(2.0) + (y2 as f32 - y1 as f32).powf(2.0)).sqrt();
        // 5.0 is the maximum distance between points
        let steps = ((length / 5.0).ceil() as i32).max(1);

        self.pen_up()?;
        self.pen_down_at((x1, y1))?;

        for i in 0..=steps {
            let x = x1 + (((x2 - x1) as i64 * i as i64) / steps as i64) as i32;
            let y = y1 + (((y2 - y1) as i64 * i as i64) / steps as i64) as i32;
            self.goto_xy((x, y))?;
        }

        self.pen_up()?;

        Ok(())
    }

    pub fn draw_bitmap(&mut self, bitmap: &[Vec<bool>]) -> Result<()> {
        let mut is_pen_down = false;
        for (y, row) in bitmap.iter().enumerate() {
            for (x, &pixel) in row.iter().enumerate() {
                if pixel {
                    if !is_pen_down {
                        self.goto_xy_virtual((x as i32, y as i32))?;
                        self.pen_down()?;
                        is_pen_down = true;
                        sleep(Duration::from_millis(1));
                    }
                    self.goto_xy_virtual((x as i32, y as i32))?;
                    self.goto_xy_virtual((x as i32 + 1, y as i32))?;
                } else if is_pen_down {
                    self.pen_up()?;
                    is_pen_down = false;
                    sleep(Duration::from_millis(1));
                }
            }
            self.pen_up()?;
            is_pen_down = false;
            sleep(Duration::from_millis(5));
        }
        Ok(())
    }

    pub fn draw_rectangle(
        &mut self,
        top_left: (i32, i32),
        bottom_right: (i32, i32),
        fill: bool,
    ) -> Result<()> {
        let (x1, y1) = top_left;
        let (x2, y2) = bottom_right;

        if fill {
            // Draw horizontal lines to fill the rectangle
            for y in y1..=y2 {
                self.draw_line_screen((x1, y), (x2, y))?;
            }
        } else {
            // Draw outline only
            self.draw_line_screen((x1, y1), (x2, y1))?; // Top
            self.draw_line_screen((x2, y1), (x2, y2))?; // Right
            self.draw_line_screen((x2, y2), (x1, y2))?; // Bottom
            self.draw_line_screen((x1, y2), (x1, y1))?; // Left
        }

        Ok(())
    }

    // Hover-before-contact follows Ghostwriter; see THIRD_PARTY_NOTICES.md.
    pub fn pen_down_at(&mut self, (x, y): (i32, i32)) -> Result<()> {
        if let Some(device) = &mut self.device {
            device.send_events(&[
                InputEvent::new(EvdevEventType::ABSOLUTE.0, 0, x),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, 1, y),
                InputEvent::new(EvdevEventType::KEY.0, 320, 1),
                InputEvent::new(EvdevEventType::KEY.0, 330, 0),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, 24, 0),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, 25, 100),
                InputEvent::new(EvdevEventType::SYNCHRONIZATION.0, 0, 0),
            ])?;
            sleep(Duration::from_millis(10));
        }
        self.pen_down()
    }
    pub fn pen_down(&mut self) -> Result<()> {
        if let Some(device) = &mut self.device {
            device.send_events(&[
                InputEvent::new(EvdevEventType::KEY.0, 320, 1), // BTN_TOOL_PEN
                InputEvent::new(EvdevEventType::KEY.0, 330, 1), // BTN_TOUCH
                InputEvent::new(EvdevEventType::ABSOLUTE.0, 24, 2630), // ABS_PRESSURE (max pressure)
                InputEvent::new(EvdevEventType::ABSOLUTE.0, 25, 0),    // ABS_DISTANCE
                InputEvent::new(EvdevEventType::SYNCHRONIZATION.0, 0, 0), // SYN_REPORT
            ])?;
            sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    pub fn pen_up(&mut self) -> Result<()> {
        if let Some(device) = &mut self.device {
            device.send_events(&[
                InputEvent::new(EvdevEventType::ABSOLUTE.0, 24, 0), // ABS_PRESSURE
                InputEvent::new(EvdevEventType::ABSOLUTE.0, 25, 100), // ABS_DISTANCE
                InputEvent::new(EvdevEventType::KEY.0, 330, 0),     // BTN_TOUCH
                InputEvent::new(EvdevEventType::KEY.0, 320, 0),     // BTN_TOOL_PEN
                InputEvent::new(EvdevEventType::SYNCHRONIZATION.0, 0, 0), // SYN_REPORT
            ])?;
            sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    /// Establish rubber proximity before contact, just like the pen hover frame.
    fn eraser_down_at(&mut self, point: (i32, i32)) -> Result<()> {
        let (x, y) = self.virtual_to_input(point);
        if let Some(device) = &mut self.device {
            device.send_events(&[
                InputEvent::new(EvdevEventType::ABSOLUTE.0, 0, x),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, 1, y),
                InputEvent::new(EvdevEventType::KEY.0, 320, 0),
                InputEvent::new(EvdevEventType::KEY.0, 321, 1),
                InputEvent::new(EvdevEventType::KEY.0, 330, 0),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, 24, 0),
                InputEvent::new(EvdevEventType::ABSOLUTE.0, 25, 100),
                InputEvent::new(EvdevEventType::SYNCHRONIZATION.0, 0, 0),
            ])?;
            sleep(Duration::from_millis(10));
        }
        self.eraser_down()?;
        sleep(Duration::from_millis(10));
        Ok(())
    }

    /// Activate eraser tool (simulates flipping the stylus to eraser end)
    pub fn eraser_down(&mut self) -> Result<()> {
        if let Some(device) = &mut self.device {
            device.send_events(&[
                InputEvent::new(EvdevEventType::KEY.0, 321, 1), // BTN_TOOL_RUBBER (eraser)
                InputEvent::new(EvdevEventType::KEY.0, 330, 1), // BTN_TOUCH
                InputEvent::new(EvdevEventType::ABSOLUTE.0, 24, 2630), // ABS_PRESSURE (max pressure)
                InputEvent::new(EvdevEventType::ABSOLUTE.0, 25, 0),    // ABS_DISTANCE
                InputEvent::new(EvdevEventType::SYNCHRONIZATION.0, 0, 0), // SYN_REPORT
            ])?;
        }
        Ok(())
    }

    /// Deactivate eraser tool
    pub fn eraser_up(&mut self) -> Result<()> {
        if let Some(device) = &mut self.device {
            device.send_events(&[
                InputEvent::new(EvdevEventType::ABSOLUTE.0, 24, 0), // ABS_PRESSURE
                InputEvent::new(EvdevEventType::ABSOLUTE.0, 25, 100), // ABS_DISTANCE
                InputEvent::new(EvdevEventType::KEY.0, 330, 0),     // BTN_TOUCH
                InputEvent::new(EvdevEventType::KEY.0, 321, 0),     // BTN_TOOL_RUBBER
                InputEvent::new(EvdevEventType::SYNCHRONIZATION.0, 0, 0), // SYN_REPORT
            ])?;
        }
        Ok(())
    }

    /// Erase content in a rectangular region by using the eraser tool
    pub fn erase_rectangle(
        &mut self,
        top_left: (i32, i32),
        bottom_right: (i32, i32),
    ) -> Result<()> {
        info!(
            "Erasing rectangle from ({}, {}) to ({}, {})",
            top_left.0, top_left.1, bottom_right.0, bottom_right.1
        );

        let (x1, y1) = top_left;
        let (x2, y2) = bottom_right;

        // Erase by filling the rectangle with eraser strokes
        let result = (|| -> Result<()> {
            for y in y1..=y2 {
                self.eraser_up()?;
                self.goto_xy_virtual((x1, y))?;
                self.eraser_down()?;
                self.goto_xy_virtual((x2, y))?;
            }
            Ok(())
        })();
        let release = self.eraser_up();
        result?;
        release
    }

    pub fn goto_xy_virtual(&mut self, point: (i32, i32)) -> Result<()> {
        self.goto_xy(self.virtual_to_input(point))
    }

    pub fn goto_xy(&mut self, (x, y): (i32, i32)) -> Result<()> {
        if let Some(device) = &mut self.device {
            device.send_events(&[
                InputEvent::new(EvdevEventType::ABSOLUTE.0, 0, x), // ABS_X
                InputEvent::new(EvdevEventType::ABSOLUTE.0, 1, y), // ABS_Y
                InputEvent::new(EvdevEventType::SYNCHRONIZATION.0, 0, 0), // SYN_REPORT
            ])?;
            sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    pub fn max_x_value(&self) -> i32 {
        match self.device_model {
            DeviceModel::Remarkable2 => 15725,
            DeviceModel::RemarkablePaperPro => 11180,
            DeviceModel::Unknown => 15725, // Default to RM2
        }
    }

    pub fn max_y_value(&self) -> i32 {
        match self.device_model {
            DeviceModel::Remarkable2 => 20966,
            DeviceModel::RemarkablePaperPro => 15340,
            DeviceModel::Unknown => 20966, // Default to RM2
        }
    }

    fn virtual_to_input(&self, (x, y): (i32, i32)) -> (i32, i32) {
        // Swap and normalize the coordinates
        let x_normalized = x as f32 / VIRTUAL_WIDTH as f32;
        let y_normalized = y as f32 / VIRTUAL_HEIGHT as f32;

        match self.device_model {
            DeviceModel::RemarkablePaperPro => {
                let x_input = (x_normalized * self.max_x_value() as f32) as i32;
                let y_input = (y_normalized * self.max_y_value() as f32) as i32;
                (x_input, y_input)
            }
            _ => {
                let x_input = ((1.0 - y_normalized) * self.max_y_value() as f32) as i32;
                let y_input = (x_normalized * self.max_x_value() as f32) as i32;
                (x_input, y_input)
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
impl Pen {
    pub fn new(_no_draw: bool) -> Self {
        let device_model = DeviceModel::detect();
        info!("Pen using device model: {}", device_model.name());

        Self {
            _device_model: device_model,
        }
    }

    pub fn draw_line_screen(&mut self, _p1: (i32, i32), _p2: (i32, i32)) -> Result<()> {
        Ok(())
    }

    pub fn draw_path_screen(&mut self, _points: &[(i32, i32)]) -> Result<()> {
        Ok(())
    }

    pub fn erase_path_screen(&mut self, _points: &[(i32, i32)]) -> Result<()> {
        Ok(())
    }

    pub fn draw_line(&mut self, _p1: (i32, i32), _p2: (i32, i32)) -> Result<()> {
        Ok(())
    }

    pub fn draw_bitmap(&mut self, _bitmap: &[Vec<bool>]) -> Result<()> {
        Ok(())
    }

    pub fn draw_rectangle(
        &mut self,
        _top_left: (i32, i32),
        _bottom_right: (i32, i32),
        _fill: bool,
    ) -> Result<()> {
        Ok(())
    }

    pub fn erase_rectangle(
        &mut self,
        _top_left: (i32, i32),
        _bottom_right: (i32, i32),
    ) -> Result<()> {
        Ok(())
    }
}

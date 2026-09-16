use super::{raster, scenario::*};
use crate::{
    device::{
        backend::{DeviceBackend, Frame},
        touch::HoldTimer,
    },
    workflow::xochitl_integration::NavigationDirection,
    TriggerCorner,
};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use image::{DynamicImage, Rgba, RgbaImage};
use serde::Serialize;
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    path::Path,
    rc::Rc,
    time::Duration,
};

pub type Shared = Rc<RefCell<State>>;

#[derive(Clone)]
pub struct Page {
    pub background: RgbaImage,
    pub text: String,
    pub lines: Vec<((i32, i32), (i32, i32))>,
    pub indicator_visible: bool,
}
impl Page {
    pub fn image(&self) -> RgbaImage {
        let mut image = self.background.clone();
        raster::text(&mut image, &self.text);
        if self.indicator_visible {
            for pair in crate::workflow::indicator::circle_points().windows(2) {
                imageproc::drawing::draw_line_segment_mut(
                    &mut image,
                    (pair[0].0 as f32, pair[0].1 as f32),
                    (pair[1].0 as f32, pair[1].1 as f32),
                    Rgba([0, 0, 0, 255]),
                );
            }
        }
        for &(from, to) in &self.lines {
            imageproc::drawing::draw_line_segment_mut(
                &mut image,
                (from.0 as f32, from.1 as f32),
                (to.0 as f32, to.1 as f32),
                Rgba([0, 0, 0, 255]),
            );
        }
        image
    }
    pub fn x_count(&self) -> usize {
        self.lines
            .windows(2)
            .filter(|pair| {
                use crate::workflow::indicator::{BOTTOM, LEFT, RIGHT, TOP};
                pair[0] == ((LEFT, TOP), (RIGHT, BOTTOM))
                    && pair[1] == ((RIGHT, TOP), (LEFT, BOTTOM))
            })
            .count()
    }
}

#[derive(Clone, Serialize)]
pub struct Event {
    pub at_ms: u64,
    pub page: usize,
    pub action: String,
    pub detail: String,
}

pub struct State {
    pub pages: Vec<Page>,
    pub initial: Vec<RgbaImage>,
    pub active: usize,
    pub clock: u64,
    pub events: Vec<Event>,
    pub counts: BTreeMap<Operation, usize>,
    pub model_calls: usize,
    pub faults: Vec<Fault>,
    pub used_faults: usize,
    pub header: Option<DynamicImage>,
    pub gestures: VecDeque<Vec<GestureFrame>>,
    pub corner: TriggerCorner,
    pub last: Frame,
}

impl State {
    pub fn new(scenario: &Scenario, root: &Path) -> Result<Self> {
        let mut pages = Vec::new();
        for spec in &scenario.pages {
            let mut background = if let Some(path) = &spec.image {
                image::open(root.join(path))
                    .with_context(|| format!("Load page {}", path.display()))?
                    .to_rgba8()
            } else {
                RgbaImage::from_pixel(768, 1024, Rgba([255, 255, 255, 255]))
            };
            anyhow::ensure!(
                background.dimensions() == (768, 1024),
                "Page images must be 768x1024"
            );
            if let Some(path) = &spec.strokes {
                let strokes: Vec<Vec<[i32; 2]>> =
                    serde_json::from_slice(&std::fs::read(root.join(path))?)?;
                for stroke in strokes {
                    for pair in stroke.windows(2) {
                        anyhow::ensure!(
                            pair.iter()
                                .all(|p| (0..768).contains(&p[0]) && (0..1024).contains(&p[1])),
                            "Stroke outside page"
                        );
                        imageproc::drawing::draw_line_segment_mut(
                            &mut background,
                            (pair[0][0] as f32, pair[0][1] as f32),
                            (pair[1][0] as f32, pair[1][1] as f32),
                            Rgba([0, 0, 0, 255]),
                        );
                    }
                }
            }
            pages.push(Page {
                background,
                text: spec.text.clone(),
                lines: Vec::new(),
                indicator_visible: false,
            });
        }
        let initial = pages.iter().map(Page::image).collect();
        let header = scenario
            .header_image
            .as_ref()
            .map(|p| image::open(root.join(p)))
            .transpose()?;
        Ok(Self {
            pages,
            initial,
            active: scenario.active_page,
            clock: 0,
            events: Vec::new(),
            counts: BTreeMap::new(),
            model_calls: 0,
            faults: scenario.faults.clone(),
            used_faults: 0,
            header,
            gestures: scenario.gestures.clone().into(),
            corner: TriggerCorner::from_string(&scenario.trigger_corner)?,
            last: Frame::default(),
        })
    }
    pub fn event(&mut self, action: &str, detail: impl Into<String>) {
        self.events.push(Event {
            at_ms: self.clock,
            page: self.active,
            action: action.into(),
            detail: detail.into(),
        });
    }
    fn operation(&mut self, operation: Operation) -> Result<Option<Effect>> {
        if matches!(
            operation,
            Operation::Capture
                | Operation::Next
                | Operation::Previous
                | Operation::Text
                | Operation::Body
                | Operation::HeaderSave
                | Operation::Line
        ) {
            anyhow::ensure!(
                !self.pages[self.active].indicator_visible,
                "Temporary circle present before {operation:?}"
            );
        }
        let call = self.counts.entry(operation).or_default();
        *call += 1;
        let effect = self
            .faults
            .iter()
            .find(|f| f.operation == operation && f.call == *call)
            .map(|f| f.effect);
        if effect.is_some() {
            self.used_faults += 1;
        }
        self.event(
            &format!("{operation:?}").to_lowercase(),
            format!("{effect:?}"),
        );
        if effect == Some(Effect::Error) {
            bail!("Injected {operation:?} error");
        }
        Ok(effect)
    }
}

pub struct SimDevice(pub Shared);

fn png(image: &DynamicImage) -> Result<Vec<u8>> {
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

impl DeviceBackend for SimDevice {
    fn capture(&mut self) -> Result<Frame> {
        let mut state = self.0.borrow_mut();
        let effect = state.operation(Operation::Capture)?;
        if effect == Some(Effect::Stale) {
            anyhow::ensure!(
                !state.last.png.is_empty(),
                "No previous capture for stale fault"
            );
            return Ok(state.last.clone());
        }
        if effect == Some(Effect::Corrupt) {
            return Ok(Frame {
                png: vec![0],
                details: Vec::new(),
            });
        }
        let image = DynamicImage::ImageRgba8(state.pages[state.active].image());
        let h = image.height();
        let th = h * 2 / 5;
        let details = [0, (h - th) / 2, h - th]
            .iter()
            .map(|&y| Ok(STANDARD.encode(png(&image.crop_imm(0, y, image.width(), th))?)))
            .collect::<Result<Vec<_>>>()?;
        state.last = Frame {
            png: png(&image)?,
            details,
        };
        Ok(state.last.clone())
    }
    fn detail_images(&self) -> Result<Vec<String>> {
        Ok(self.0.borrow().last.details.clone())
    }
    fn wait_for_trigger(&mut self) -> Result<()> {
        let mut state = self.0.borrow_mut();
        state.operation(Operation::Trigger)?;
        let frames = state
            .gestures
            .pop_front()
            .context("Simulator gesture sequence exhausted")?;
        let base = state.clock;
        let mut hold = HoldTimer::default();
        for frame in frames {
            let now = Duration::from_millis(frame.at_ms);
            if hold.triggered(now) {
                state.clock = base + hold.deadline().unwrap().as_millis() as u64;
                state.event("hold_triggered", "two-second hold");
                return Ok(());
            }
            state.clock = base + frame.at_ms;
            state.event(
                "gesture_frame",
                format!("contact={} x={} y={}", frame.contact, frame.x, frame.y),
            );
            hold.contact(
                frame.contact && state.corner.contains(frame.x, frame.y),
                now,
            );
        }
        if let Some(deadline) = hold.deadline() {
            state.clock = base + deadline.as_millis() as u64;
            state.event("hold_triggered", "stationary timeout without new events");
            return Ok(());
        }
        bail!("Simulator gesture sequence ended without a valid hold")
    }
    fn dismiss_trigger(&mut self) -> Result<()> {
        let mut state = self.0.borrow_mut();
        state.event("dismiss_trigger", "middle-bottom tap");
        state.clock += 100;
        Ok(())
    }
    fn navigate(&mut self, direction: NavigationDirection) -> Result<()> {
        let mut state = self.0.borrow_mut();
        let operation = match direction {
            NavigationDirection::Next => Operation::Next,
            NavigationDirection::Previous => Operation::Previous,
        };
        let effect = state.operation(operation)?;
        if effect != Some(Effect::NoMove) {
            state.active = match direction {
                NavigationDirection::Next => (state.active + 1).min(state.pages.len() - 1),
                NavigationDirection::Previous => state.active.saturating_sub(1),
            };
        }
        state.clock += 700; // 50 ms contact + 15*10 ms motion + 500 ms transition.
        state.event("navigation_settled", "");
        Ok(())
    }
    fn render_text(&mut self, text: &str) -> Result<()> {
        let mut state = self.0.borrow_mut();
        state.operation(Operation::Text)?;
        let page = state.active;
        state.pages[page].text.push_str(text);
        Ok(())
    }
    fn body_mode(&mut self) -> Result<()> {
        self.0.borrow_mut().operation(Operation::Body)?;
        Ok(())
    }
    fn line(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<()> {
        let mut state = self.0.borrow_mut();
        state.operation(Operation::Line)?;
        let page = state.active;
        state.pages[page].lines.push((from, to));
        Ok(())
    }
    fn erase(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<()> {
        let mut state = self.0.borrow_mut();
        state.event("erase", format!("{from:?} {to:?}"));
        let page = state.active;
        let mut image = state.pages[page].image();
        for y in from.1.max(0)..to.1.min(1024) {
            for x in from.0.max(0)..to.0.min(768) {
                image.put_pixel(x as u32, y as u32, Rgba([255, 255, 255, 255]));
            }
        }
        state.pages[page] = Page {
            background: image,
            text: String::new(),
            lines: Vec::new(),
            indicator_visible: false,
        };
        Ok(())
    }
    fn bitmap(&mut self, bitmap: &[Vec<bool>]) -> Result<()> {
        let mut state = self.0.borrow_mut();
        state.event("bitmap", "");
        let page = state.active;
        for (y, row) in bitmap.iter().take(1024).enumerate() {
            for (x, pixel) in row.iter().take(768).enumerate() {
                if *pixel {
                    state.pages[page].background.put_pixel(
                        x as u32,
                        y as u32,
                        Rgba([0, 0, 0, 255]),
                    );
                }
            }
        }
        Ok(())
    }
    fn progress(&mut self, message: Option<&str>) -> Result<()> {
        self.0.borrow_mut().event("progress", message.unwrap_or(""));
        Ok(())
    }
    fn status_circle(&mut self) -> Result<()> {
        let mut state = self.0.borrow_mut();
        let page = state.active;
        // A failed native draw may already have emitted part of a stroke.
        state.pages[page].indicator_visible = true;
        state.operation(Operation::StatusCircle)?;
        Ok(())
    }
    fn status_clear(&mut self) -> Result<()> {
        let mut state = self.0.borrow_mut();
        state.operation(Operation::StatusClear)?;
        let page = state.active;
        state.pages[page].indicator_visible = false;
        Ok(())
    }
    fn status_suppressed(&mut self) {
        self.0
            .borrow_mut()
            .event("status_suppressed", "occupied or unknown region");
    }
    fn load_header(&self) -> Option<DynamicImage> {
        self.0.borrow().header.clone()
    }
    fn save_header(&mut self, image: &DynamicImage) -> Result<()> {
        let mut state = self.0.borrow_mut();
        state.operation(Operation::HeaderSave)?;
        state.header = Some(image.clone());
        Ok(())
    }
    fn delay(&mut self, duration: Duration) {
        let mut state = self.0.borrow_mut();
        state.event("delay", duration.as_millis().to_string());
        state.clock += duration.as_millis() as u64;
    }
}

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
    pub indicator_paths: Vec<(crate::workflow::indicator::Stroke, u8)>,
}
impl Page {
    pub fn image(&self) -> RgbaImage {
        let mut image = self.background.clone();
        raster::text(&mut image, &self.text);
        for (stroke, passes) in &self.indicator_paths {
            let shade = 180u8.saturating_sub(passes.saturating_mul(40));
            for pair in stroke.points().windows(2) {
                imageproc::drawing::draw_line_segment_mut(
                    &mut image,
                    (pair[0].0 as f32, pair[0].1 as f32),
                    (pair[1].0 as f32, pair[1].1 as f32),
                    Rgba([shade, shade, shade, 255]),
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
    pub fn failure_codes(&self) -> Vec<String> {
        use crate::workflow::indicator::Failure;
        let codes = [
            Failure::Selection,
            Failure::Transcription,
            Failure::Provider,
            Failure::NoSuccessor,
            Failure::InvalidSuccessor,
            Failure::Device,
        ];
        self.lines
            .windows(3)
            .filter_map(|lines| {
                use crate::workflow::indicator::{BOTTOM, LEFT, RIGHT, TOP, X_INSET};
                let (l, t, r, b) = (
                    LEFT + X_INSET,
                    TOP + X_INSET,
                    RIGHT - X_INSET,
                    BOTTOM - X_INSET,
                );
                if lines[0] != ((l, t), (r, b)) || lines[1] != ((r, t), (l, b)) {
                    return None;
                }
                codes
                    .iter()
                    .find(|code| code.segment() == lines[2])
                    .map(|code| format!("{code:?}"))
            })
            .collect()
    }
    pub fn x_count(&self) -> usize {
        self.lines
            .windows(2)
            .filter(|pair| {
                use crate::workflow::indicator::{BOTTOM, LEFT, RIGHT, TOP, X_INSET};
                let (left, top, right, bottom) = (
                    LEFT + X_INSET,
                    TOP + X_INSET,
                    RIGHT - X_INSET,
                    BOTTOM - X_INSET,
                );
                pair[0] == ((left, top), (right, bottom))
                    && pair[1] == ((right, top), (left, bottom))
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
    #[cfg(test)]
    pub idle_events: VecDeque<Vec<crate::device::interaction::Interaction>>,
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
    pub visit: u64,
    pub session: u64,
    pub persisted: Option<crate::workflow::history::PageState>,
    pub deletion: Option<(usize, String, String)>,
}

impl State {
    fn history_page(&self) -> Result<crate::workflow::history::PageState> {
        self.history_page_at(self.active)
    }
    fn history_page_at(&self, index: usize) -> Result<crate::workflow::history::PageState> {
        use crate::{
            device::native_text::{Character, NativeText, Paragraph},
            workflow::history::{Owner, PageState},
        };
        let page = &self.pages[index];
        Ok(PageState {
            owner: Owner {
                document: "simulated-document".into(),
                page: index.to_string(),
                visit: self.visit.to_string(),
                session: self.session.to_string(),
            },
            content: NativeText {
                paragraphs: page
                    .text
                    .split('\n')
                    .map(|line| Paragraph {
                        style: 1,
                        characters: line
                            .chars()
                            .map(|value| Character {
                                value,
                                bold: false,
                                italic: false,
                            })
                            .collect(),
                    })
                    .collect(),
                root_layout: vec![1],
                scene_records: vec![serde_json::to_vec(&page.lines)?],
            },
            seal: serde_json::to_vec(&(&page.text, &page.lines, page.indicator_visible))?,
            supported: !page.indicator_visible,
        })
    }

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
                indicator_paths: Vec::new(),
            });
        }
        let initial = pages.iter().map(Page::image).collect();
        let header = scenario
            .header_image
            .as_ref()
            .map(|p| image::open(root.join(p)))
            .transpose()?;
        Ok(Self {
            #[cfg(test)]
            idle_events: VecDeque::new(),
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
            visit: 0,
            session: 0,
            persisted: None,
            deletion: None,
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
    #[cfg(test)]
    fn wait_for_interactions(
        &mut self,
        _timeout: Option<Duration>,
    ) -> Result<Vec<crate::device::interaction::Interaction>> {
        self.0
            .borrow_mut()
            .idle_events
            .pop_front()
            .context("Idle test has no remaining input")
    }
    fn history_snapshot(
        &mut self,
        expected: Option<&str>,
    ) -> Result<Option<crate::workflow::history::PageState>> {
        let mut state = self.0.borrow_mut();
        let effect = state.operation(Operation::HistorySnapshot)?;
        if effect == Some(Effect::Unavailable) {
            state.event(
                "history_identity_unavailable",
                "native current-document identity absent despite visible document",
            );
            return Ok(None);
        }
        if effect == Some(Effect::WrongPage) {
            anyhow::ensure!(state.pages.len() > 1, "Wrong-page fault needs another page");
            state.event(
                "history_wrong_page",
                "stable last-opened metadata refers to another document/page",
            );
            return Ok(Some(
                state.history_page_at((state.active + 1) % state.pages.len())?,
            ));
        }
        if effect == Some(Effect::Stale) {
            state.event(
                "history_persistence_timeout",
                "old snapshot remains stable; no ownership",
            );
            return Ok(state.persisted.clone());
        }
        if effect == Some(Effect::Lag) {
            state.event(
                "history_persistence_lag",
                "visible text precedes persisted text; wait for expected content",
            );
            // Native RM2 3.28 measurements were about 10.6-10.8 seconds.
            state.clock += 11_000;
        }
        let current = state.history_page()?;
        anyhow::ensure!(
            expected.is_none_or(|text| current.content.text() == text),
            "Simulated persisted text did not reach expected content"
        );
        state.persisted = Some(current.clone());
        Ok(Some(current))
    }

    fn history_discard(&mut self) {
        self.0.borrow_mut().deletion = None;
    }

    fn history_mutate(
        &mut self,
        command: crate::workflow::history::Command,
        _expected: &str,
    ) -> Result<crate::workflow::history::PageState> {
        use crate::workflow::history::Command;
        let mut state = self.0.borrow_mut();
        let effect = state.operation(Operation::HistoryMutation)?;
        let active = state.active;
        let applied = state.pages[active].text.clone();
        let next = match command {
            Command::DeleteSuffix {
                characters,
                paragraphs,
            } => {
                let length = applied.chars().count();
                anyhow::ensure!(characters <= length, "Simulated selection exceeds text");
                let suffix: String = applied.chars().skip(length - characters).collect();
                anyhow::ensure!(
                    suffix.bytes().filter(|value| *value == b'\n').count() == paragraphs,
                    "Simulated paragraph range differs from owned suffix"
                );
                let count = if effect == Some(Effect::Partial) {
                    characters / 2
                } else {
                    characters
                };
                let removed: String = applied.chars().take(length - count).collect();
                state.deletion = Some((active, applied, removed.clone()));
                removed
            }
            Command::RestoreDeletion | Command::RepeatDeletion => {
                let (page, applied, removed) = state
                    .deletion
                    .as_ref()
                    .context("Missing owned simulated deletion")?;
                anyhow::ensure!(
                    *page == active,
                    "Simulated native entry belongs to another page"
                );
                if command == Command::RestoreDeletion {
                    applied.clone()
                } else {
                    removed.clone()
                }
            }
        };
        state.pages[active].text = next;
        if effect == Some(Effect::Partial) {
            bail!("Injected partial history mutation");
        }
        if effect == Some(Effect::Corrupt) {
            state.pages[active].text.push_str("[unexpected]");
        }
        let result = state.history_page()?;
        state.persisted = Some(result.clone());
        Ok(result)
    }

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
            state.visit += 1;
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
            indicator_paths: Vec::new(),
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
    fn status_stroke(&mut self, stroke: crate::workflow::indicator::Stroke) -> Result<()> {
        let mut state = self.0.borrow_mut();
        let page = state.active;
        // A failed native draw may already have emitted part of a stroke.
        state.pages[page].indicator_visible = true;
        if let Some((_, count)) = state.pages[page]
            .indicator_paths
            .iter_mut()
            .find(|(s, _)| *s == stroke)
        {
            *count = count.saturating_add(1);
        } else {
            state.pages[page].indicator_paths.push((stroke, 1));
        }
        state.event("status_path", format!("{stroke:?}"));
        state.operation(Operation::StatusStroke)?;
        Ok(())
    }
    fn status_clear(&mut self, strokes: &[crate::workflow::indicator::Stroke]) -> Result<()> {
        let mut state = self.0.borrow_mut();
        state.operation(Operation::StatusClear)?;
        let page = state.active;
        state.pages[page]
            .indicator_paths
            .retain(|(stroke, _)| !strokes.contains(stroke));
        state.pages[page].indicator_visible = !state.pages[page].indicator_paths.is_empty();
        Ok(())
    }
    fn monotonic(&self) -> Duration {
        Duration::from_millis(self.0.borrow().clock)
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

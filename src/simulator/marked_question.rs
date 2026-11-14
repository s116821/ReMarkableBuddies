use super::{device::Shared, exchange_history::ModeledPage, State};
use crate::{
    conversation::{
        capture_facts::HistoricalCapture, marked_question::modeled_source,
        reader_plan::checked_region,
    },
    device::backend::{MarkedEraseSubmission, MarkedSourceBinding, PreparedMarkedErase},
    storage::Uuid,
    workflow::exchange_history::UserPresentationTarget,
};
use anyhow::{ensure, Result};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use remarkable_open_sdk::{
    capture::{
        synthetic, Affine, CaptureLimits, CaptureRequest, CapturedBatch, ImageRole, ResizeFilter,
        SourceRegion,
    },
    mock::MockPlatform,
    OperationId, Platform,
};
use std::{io::Cursor, rc::Rc, time::Duration};

const FRAME: [u32; 2] = [768, 1024];
const MAPPING: [f64; 6] = [1.0 / 768.0, 0., 0., 1.0 / 1024.0, 0., 0.];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Stroke {
    pub(super) pixels: Vec<(u32, u32)>,
    pub(super) color: Rgba<u8>,
}

#[derive(Clone, Copy)]
pub(super) enum EraseFault {
    Partial,
    LostReply,
    RenewedVisitBeforeAccept,
}

pub(super) struct MarkedQuestion {
    pub(super) document: Uuid,
    pub(super) source_page: Uuid,
    pub(super) target: UserPresentationTarget,
    pub(super) raster: Rc<RgbaImage>,
    pub(super) strokes: Vec<Stroke>,
    pub(super) viewport: String,
    pub(super) page: ModeledPage,
    pub(super) has_destination: bool,
    pub(super) has_source_binding: bool,
    pub(super) erase_fault: Option<EraseFault>,
    pub(super) erase_calls: usize,
    pub(super) accepted_operation: Option<Uuid>,
    pub(super) capture: Option<MarkedSourceBinding>,
    pub(super) captured_strokes: Vec<Stroke>,
    pub(super) before_erase: Option<Box<dyn FnOnce()>>,
}

fn sdk_id(id: Uuid) -> remarkable_open_sdk::Uuid {
    remarkable_open_sdk::Uuid::parse(&id.to_string()).expect("valid modeled UUID")
}

impl MarkedQuestion {
    pub(super) fn remember_capture(&mut self, binding: MarkedSourceBinding) {
        self.captured_strokes = self.strokes.clone();
        self.capture = Some(binding);
    }
    pub(super) fn source_image(&self) -> RgbaImage {
        let mut image = self.raster.as_ref().clone();
        for stroke in &self.strokes {
            for &(x, y) in &stroke.pixels {
                image.put_pixel(x, y, stroke.color);
            }
        }
        image
    }

    pub(super) fn capture(
        &self,
        session: u64,
        visit: u64,
    ) -> Result<(CapturedBatch, MarkedSourceBinding)> {
        let [width, height] = FRAME;
        ensure!(
            self.raster.dimensions() == (width, height)
                && self.strokes.iter().all(|stroke| !stroke.pixels.is_empty()
                    && stroke.pixels.iter().all(|&(x, y)| x < width && y < height)),
            "invalid modeled source layer"
        );
        let owner = MockPlatform::new(
            sdk_id(Uuid::new_v4()),
            sdk_id(self.document),
            vec![sdk_id(self.source_page), sdk_id(self.target.page)],
            sdk_id(self.source_page),
        )
        .observe_page()
        .map_err(|error| anyhow::anyhow!("modeled SDK observation: {error:?}"))?;
        // The synthetic SDK owner models the initial visit only. A later visit
        // cannot use this capture path to replace its original visit or session.
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(self.source_image()).write_to(&mut png, ImageFormat::Png)?;
        let png = png.into_inner();
        let acquisition = synthetic::Acquisition {
            before: owner.clone(),
            after: owner.clone(),
            rendered_owner: owner.clone(),
            render_generation: 1,
            buffer_generation: 1,
            input_changed: false,
            interval: [Duration::ZERO, Duration::from_millis(1)],
            viewport_revision: self.viewport.clone(),
            conversion_procedure: "modeled-rgba8".into(),
            procedure_revision: "modeled-source-viewport".into(),
            native_png: png.clone(),
            native_transform: Affine::new(MAPPING)?,
            valid_source_region: SourceRegion {
                x: 0.,
                y: 0.,
                width: f64::from(width),
                height: f64::from(height),
            },
            target: None,
        };
        let request = CaptureRequest::new(
            OperationId(sdk_id(Uuid::new_v4())),
            owner,
            Duration::from_secs(1),
            false,
        );
        let batch = synthetic::validate_batch(
            &request,
            acquisition,
            vec![synthetic::ImageInput {
                role: ImageRole::Overview,
                crop: remarkable_open_sdk::capture::PixelRect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                },
                output_dimensions: FRAME,
                filter: ResizeFilter::Nearest,
                png,
            }],
            CaptureLimits::default(),
        )?;
        let facts = HistoricalCapture::from_batch(&batch)?;
        ensure!(
            facts.source.session.value() == session && facts.source.visit.value() == visit,
            "SDK capture does not match the active simulated visit"
        );
        let overview = &facts.images[0];
        let binding = MarkedSourceBinding {
            source: modeled_source(&facts),
            overview_sha256: overview.sha256.clone(),
            dimensions: overview.dimensions,
            viewport_revision: facts.viewport_revision,
            overview_to_source: overview.affine_bits.map(|n| f64::from_bits(n.value())),
        };
        Ok((batch, binding))
    }

    fn current(&self, state: &State, binding: &MarkedSourceBinding) -> bool {
        !state.request_lost
            && state.active == 0
            && state.request_pin == Some((state.active, state.visit, state.session))
            && self.capture.as_ref() == Some(binding)
            && self.document == binding.source.document
            && self.source_page == binding.source.page
            && binding.source.session.parse::<u64>() == Ok(state.session)
            && binding.source.visit.parse::<u64>() == Ok(state.visit)
            && self.viewport == binding.viewport_revision
            && binding.dimensions == FRAME
            && binding.overview_to_source == MAPPING
    }

    fn after_erase(&self, region: [i32; 4]) -> Option<Vec<Stroke>> {
        let region = checked_region(region).ok()?;
        // The modeled footprint is the half-open rectangle in the captured pixel grid.
        let inside = |&(x, y): &(u32, u32)| {
            (region.x as u32..(region.x + region.width) as u32).contains(&x)
                && (region.y as u32..(region.y + region.height) as u32).contains(&y)
        };
        let mut preserved = Vec::new();
        for stroke in &self.strokes {
            if stroke.pixels.iter().any(inside) {
                if !stroke.pixels.iter().all(inside) {
                    return None;
                }
            } else {
                preserved.push(stroke.clone());
            }
        }
        (preserved.len() < self.strokes.len()).then_some(preserved)
    }
}

pub(super) fn prepare_effect<'a>(
    shared: Shared,
    binding: &MarkedSourceBinding,
    operation: Uuid,
    region: [i32; 4],
) -> Option<PreparedMarkedErase<'a>> {
    let before_erase = shared
        .borrow_mut()
        .marked_question
        .as_mut()?
        .before_erase
        .take();
    if let Some(before_erase) = before_erase {
        before_erase();
    }
    let (before, after, raster) = {
        let state = shared.borrow();
        let model = state.marked_question.as_ref()?;
        if !model.current(&state, binding)
            || model.accepted_operation.is_some()
            || model.strokes != model.captured_strokes
        {
            return None;
        }
        (
            model.strokes.clone(),
            model.after_erase(region)?,
            model.raster.clone(),
        )
    };
    let binding = binding.clone();
    let expected = after.clone();
    Some(PreparedMarkedErase(Box::new(move || {
        let mut state = shared.borrow_mut();
        if state.marked_question.as_ref().is_some_and(|model| {
            matches!(
                model.erase_fault,
                Some(EraseFault::RenewedVisitBeforeAccept)
            )
        }) {
            state.visit += 1;
            state.request_pin = Some((state.active, state.visit, state.session));
        }
        let Some(model) = state.marked_question.as_ref() else {
            return MarkedEraseSubmission::Refused;
        };
        if !model.current(&state, &binding)
            || model.accepted_operation.is_some()
            || model.strokes != before
            || !Rc::ptr_eq(&model.raster, &raster)
        {
            return MarkedEraseSubmission::Refused;
        }
        let model = state
            .marked_question
            .as_mut()
            .expect("accepted modeled owner");
        model.accepted_operation = Some(operation);
        model.erase_calls += 1;
        if matches!(model.erase_fault, Some(EraseFault::Partial)) {
            if let Some(stroke) = before.iter().find(|stroke| !after.contains(stroke)) {
                let index = model
                    .strokes
                    .iter()
                    .position(|s| s == stroke)
                    .expect("original stroke");
                model.strokes.remove(index);
            }
            return MarkedEraseSubmission::Uncertain;
        }
        model.strokes = after;
        if matches!(model.erase_fault, Some(EraseFault::LostReply)) {
            return MarkedEraseSubmission::Uncertain;
        }
        drop(state);
        MarkedEraseSubmission::Accepted {
            operation,
            readback: Box::new(move || {
                let state = shared.borrow();
                state.marked_question.as_ref().is_some_and(|model| {
                    model.current(&state, &binding)
                        && model.accepted_operation == Some(operation)
                        && model.strokes == expected
                        && Rc::ptr_eq(&model.raster, &raster)
                })
            }),
        }
    })))
}

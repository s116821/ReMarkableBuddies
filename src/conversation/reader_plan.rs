//! Immutable bounded arguments only. A plan is never effect authority.
use crate::analysis::BoundingBox;
use crate::storage::{MAX_ITEMS, MAX_RECORD};
use crate::workflow::erase_plan::smart_erase_plan;
use anyhow::{ensure, Result};
use serde::Serialize;

pub type ReaderEraseRectangle = ((i32, i32), (i32, i32));

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum ReaderHandoff {
    NextPage,
    PreviousPage,
    BodyMode,
    Text(String),
    Symbol { x: i32, y: i32, text: String },
    Erase { bounds: [i32; 4] },
    SmartErase(SmartEraseInput),
    Progress(Option<String>),
}

/// Exact input and derived ordered calls. Callers cannot substitute coordinates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SmartEraseInput {
    bounds: [i32; 4],
    screenshot: Vec<u8>,
    rectangles: Vec<ReaderEraseRectangle>,
}

fn checked_region(bounds: [i32; 4]) -> Result<BoundingBox> {
    let [x, y, width, height] = bounds;
    ensure!(
        x >= 0 && y >= 0 && width > 0 && height > 0,
        "invalid Reader erase region"
    );
    let right = x
        .checked_add(width)
        .ok_or_else(|| anyhow::anyhow!("Reader erase coordinate overflow"))?;
    let bottom = y
        .checked_add(height)
        .ok_or_else(|| anyhow::anyhow!("Reader erase coordinate overflow"))?;
    ensure!(
        right <= 768 && bottom <= 1024,
        "Reader erase region outside virtual frame"
    );
    Ok(BoundingBox {
        x,
        y,
        width,
        height,
    })
}

impl SmartEraseInput {
    pub fn new(bounds: [i32; 4], screenshot: Vec<u8>) -> Result<Self> {
        let region = checked_region(bounds)?;
        ensure!(
            screenshot.len() <= MAX_RECORD,
            "Reader erase screenshot exceeds bound"
        );
        let format = image::guess_format(&screenshot)?;
        ensure!(
            matches!(format, image::ImageFormat::Png | image::ImageFormat::Jpeg),
            "unsupported Reader erase image"
        );
        let dimensions = image::ImageReader::with_format(std::io::Cursor::new(&screenshot), format)
            .into_dimensions()?;
        ensure!(
            dimensions == (768, 1024),
            "Reader erase image is not the exact virtual frame"
        );
        let image = image::load_from_memory_with_format(&screenshot, format)?.to_luma8();
        let rectangles = smart_erase_plan(&region, &image).rectangles;
        ensure!(
            rectangles.len() <= MAX_ITEMS,
            "Reader erase lower-call count exceeds bound"
        );
        let input = Self {
            bounds,
            screenshot,
            rectangles,
        };
        ensure!(
            serde_json::to_vec(&input)?.len() <= MAX_RECORD,
            "Reader erase arguments exceed bound"
        );
        Ok(input)
    }
    pub fn bounds(&self) -> [i32; 4] {
        self.bounds
    }
    pub fn screenshot(&self) -> &[u8] {
        &self.screenshot
    }
    pub fn rectangles(&self) -> &[ReaderEraseRectangle] {
        &self.rectangles
    }
}

/// No setters or deserializer; the admitted context consumes this frozen value.
#[derive(Debug, Serialize)]
pub struct ReaderPlan {
    steps: Vec<ReaderHandoff>,
}
impl ReaderPlan {
    pub fn new(steps: Vec<ReaderHandoff>) -> Result<Self> {
        ensure!(
            !steps.is_empty() && steps.len() <= MAX_ITEMS,
            "Reader plan step count outside bound"
        );
        let mut lower_calls = 0usize;
        for step in &steps {
            if let ReaderHandoff::Symbol { x, y, .. } = step {
                ensure!(
                    (0..768).contains(x) && (0..1024).contains(y),
                    "Reader symbol outside virtual frame"
                );
            }
            if let ReaderHandoff::Erase { bounds } = step {
                checked_region(*bounds)?;
            }
            lower_calls = lower_calls
                .checked_add(match step {
                    ReaderHandoff::SmartErase(input) => input.rectangles.len(),
                    _ => 1,
                })
                .ok_or_else(|| anyhow::anyhow!("Reader plan lower-call overflow"))?;
        }
        ensure!(
            lower_calls <= MAX_ITEMS,
            "Reader plan lower-call count exceeds bound"
        );
        let plan = Self { steps };
        ensure!(
            serde_json::to_vec(&plan)?.len() <= MAX_RECORD,
            "Reader plan arguments exceed bound"
        );
        Ok(plan)
    }
    pub fn steps(&self) -> &[ReaderHandoff] {
        &self.steps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, GrayImage, Luma};
    fn png(image: GrayImage) -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        DynamicImage::ImageLuma8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }
    #[test]
    fn selected_plan_freezes_exact_input_and_preserves_repeated_lower_ordinals() {
        let mut image = GrayImage::from_pixel(768, 1024, Luma([255]));
        image.put_pixel(5, 2, Luma([0]));
        image.put_pixel(5, 3, Luma([0]));
        let bytes = png(image);
        let input = SmartEraseInput::new([4, 1, 4, 5], bytes.clone()).unwrap();
        assert_eq!(input.screenshot(), bytes);
        assert_eq!(input.bounds(), [4, 1, 4, 5]);
        assert_eq!(
            input
                .rectangles()
                .iter()
                .map(|(from, _)| from.1)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 1, 2, 3, 4, 5]
        );
        let plan = ReaderPlan::new(vec![
            ReaderHandoff::SmartErase(input),
            ReaderHandoff::Text("exact draft".into()),
        ])
        .unwrap();
        assert_eq!(plan.steps().len(), 2);
    }
    #[test]
    fn selected_plan_refuses_unbounded_or_invalid_arguments_before_publication() {
        assert!(ReaderPlan::new(vec![]).is_err());
        assert!(ReaderPlan::new(vec![ReaderHandoff::BodyMode; MAX_ITEMS + 1]).is_err());
        assert!(ReaderPlan::new(vec![ReaderHandoff::Text("x".repeat(MAX_RECORD))]).is_err());
        for bounds in [
            [i32::MAX, 0, 1, 1],
            [-1, 0, 1, 1],
            [0, 0, 0, 1],
            [767, 0, 2, 1],
        ] {
            assert!(ReaderPlan::new(vec![ReaderHandoff::Erase { bounds }]).is_err());
        }
        assert!(SmartEraseInput::new([0, 0, 1, 1], b"not an image".to_vec()).is_err());
        assert!(SmartEraseInput::new([0, 0, 1, 1], png(GrayImage::new(1, 1))).is_err());
        assert!(SmartEraseInput::new([0, 0, 768, 1024], png(GrayImage::new(768, 1024))).is_err());
    }
}

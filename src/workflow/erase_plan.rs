//! Exact ordered smart-erase geometry, shared by planning and submission.
use crate::analysis::BoundingBox;
use image::GrayImage;

pub(super) struct SmartErasePlan {
    pub ink_rows: Vec<i32>,
    pub rectangles: Vec<((i32, i32), (i32, i32))>,
}

pub(super) fn smart_erase_plan(region: &BoundingBox, image: &GrayImage) -> SmartErasePlan {
    const INK_THRESHOLD: u8 = 200;
    const MARGIN: i32 = 2;
    let mut ink_rows = Vec::new();
    for y in region.y..(region.y + region.height).min(1024) {
        if y < 0 || y >= image.height() as i32 {
            continue;
        }
        for x in region.x..(region.x + region.width).min(768) {
            if x >= 0
                && x < image.width() as i32
                && image.get_pixel(x as u32, y as u32)[0] < INK_THRESHOLD
            {
                ink_rows.push(y);
                break;
            }
        }
    }
    let mut rectangles = Vec::new();
    for &y in &ink_rows {
        let start = (y - MARGIN).max(region.y).max(0);
        let end = (y + MARGIN + 1).min(region.y + region.height).min(1024);
        for row in start..end {
            rectangles.push((
                (region.x, row),
                ((region.x + region.width).min(768), row + 1),
            ));
        }
    }
    SmartErasePlan {
        ink_rows,
        rectangles,
    }
}

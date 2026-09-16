//! One reserved status area, with conservative clearance for native erasing.
use image::DynamicImage;

pub const LEFT: i32 = 698;
pub const TOP: i32 = 934;
pub const SIZE: i32 = 50;
pub const RIGHT: i32 = LEFT + SIZE - 1;
pub const BOTTOM: i32 = TOP + SIZE - 1;
// Native pen strokes extend beyond their centerline endpoints.
pub const X_INSET: i32 = 3;

pub fn eligible(image: &DynamicImage) -> bool {
    if image.width() != 768 || image.height() != 1024 {
        return false;
    }
    let gray = image.to_luma8();
    (TOP - 12..=BOTTOM + 12)
        .all(|y| (LEFT - 12..=RIGHT + 12).all(|x| gray.get_pixel(x as u32, y as u32)[0] >= 248))
}

pub fn circle_points() -> Vec<(i32, i32)> {
    (0..=32)
        .map(|step| {
            let angle = step as f64 * std::f64::consts::TAU / 32.0;
            (
                LEFT + 24 + (20.0 * angle.cos()).round() as i32,
                TOP + 24 + (20.0 * angle.sin()).round() as i32,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma};

    #[test]
    fn status_geometry_is_bounded_and_clearance_protects_neighboring_ink() {
        let mut image = GrayImage::from_pixel(768, 1024, Luma([255]));
        assert!(eligible(&DynamicImage::ImageLuma8(image.clone())));
        image.put_pixel((LEFT - 8) as u32, TOP as u32, Luma([0]));
        assert!(!eligible(&DynamicImage::ImageLuma8(image)));
        assert!(!eligible(&DynamicImage::new_luma8(20, 20)));
        let points = circle_points();
        assert_eq!(points.first(), points.last());
        assert!(points
            .iter()
            .all(|&(x, y)| (LEFT..=RIGHT).contains(&x) && (TOP..=BOTTOM).contains(&y)));
        assert_eq!(RIGHT - LEFT + 1, 50);
        assert_eq!(BOTTOM - TOP + 1, 50);
    }
}

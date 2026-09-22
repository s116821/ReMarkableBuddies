//! One reserved status area, with conservative clearance for native erasing.
use image::DynamicImage;
use std::time::Duration;

pub const CADENCE: Duration = Duration::from_millis(333);
pub type Point = (i32, i32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Stage {
    #[default]
    Preparing,
    AnswerPending,
    AnswerReady,
}

impl Stage {
    pub fn next(self) -> Self {
        match self {
            Self::Preparing => Self::AnswerPending,
            _ => Self::AnswerReady,
        }
    }

    pub fn vertices(self) -> [Point; 3] {
        let (width, top, bottom) = match self {
            Self::Preparing => (15, 9, 38),
            Self::AnswerPending => (10, 16, 35),
            Self::AnswerReady => (5, 23, 32),
        };
        [
            (LEFT + 24, TOP + top),
            (LEFT + 24 + width, TOP + bottom),
            (LEFT + 24 - width, TOP + bottom),
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stroke {
    Edge(Stage, u8),
    Auxiliary(Stage),
}

impl Stroke {
    pub fn points(self) -> Vec<Point> {
        match self {
            Self::Edge(stage, edge) => {
                let vertices = stage.vertices();
                vec![
                    vertices[edge as usize % 3],
                    vertices[(edge as usize + 1) % 3],
                ]
            }
            Self::Auxiliary(stage) => {
                let [a, b, c] = stage.vertices();
                let distance = |p: Point, q: Point| ((p.0 - q.0) as f64).hypot((p.1 - q.1) as f64);
                let (sa, sb, sc) = (distance(b, c), distance(a, c), distance(a, b));
                let perimeter = sa + sb + sc;
                let cx = (sa * a.0 as f64 + sb * b.0 as f64 + sc * c.0 as f64) / perimeter;
                let cy = (sa * a.1 as f64 + sb * b.1 as f64 + sc * c.1 as f64) / perimeter;
                let radius = ((b.0 - c.0) * (b.1 - a.1)).abs() as f64 / perimeter;
                (0..=24)
                    .map(|i| {
                        let angle = i as f64 * std::f64::consts::TAU / 24.0;
                        (
                            (cx + radius * angle.cos()).round() as i32,
                            (cy + radius * angle.sin()).round() as i32,
                        )
                    })
                    .collect()
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    Selection,
    Transcription,
    Provider,
    NoSuccessor,
    InvalidSuccessor,
    Device,
}

impl Failure {
    pub fn segment(self) -> (Point, Point) {
        let (l, t, r, b, cx, cy) = (
            LEFT + 12,
            TOP + 12,
            LEFT + 37,
            TOP + 37,
            LEFT + 24,
            TOP + 24,
        );
        match self {
            Self::Selection => ((l, t), (r, t)),
            Self::Transcription => ((r, t), (r, b)),
            Self::Provider => ((l, b), (r, b)),
            Self::NoSuccessor => ((l, t), (l, b)),
            Self::InvalidSuccessor => ((l, cy), (r, cy)),
            Self::Device => ((cx, t), (cx, b)),
        }
    }
}

/// Next regular deadline after a stroke, skipping missed intervals.
pub fn next_deadline(start: Duration, finished: Duration) -> Duration {
    let intervals = finished.saturating_sub(start).as_nanos() / CADENCE.as_nanos() + 1;
    start + CADENCE * intervals.min(u32::MAX as u128) as u32
}

pub const LEFT: i32 = 698;
pub const TOP: i32 = 934;
pub const SIZE: i32 = 50;
pub const RIGHT: i32 = LEFT + SIZE - 1;
pub const BOTTOM: i32 = TOP + SIZE - 1;
// Native pen strokes extend beyond their centerline endpoints.
pub const X_INSET: i32 = 10;

pub fn eligible(image: &DynamicImage) -> bool {
    if image.width() != 768 || image.height() != 1024 {
        return false;
    }
    let gray = image.to_luma8();
    (TOP - 12..=BOTTOM + 12)
        .all(|y| (LEFT - 12..=RIGHT + 12).all(|x| gray.get_pixel(x as u32, y as u32)[0] >= 248))
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
        let points = Stroke::Auxiliary(Stage::AnswerReady).points();
        assert_eq!(points.first(), points.last());
        assert!(points
            .iter()
            .all(|&(x, y)| (LEFT..=RIGHT).contains(&x) && (TOP..=BOTTOM).contains(&y)));
        assert_eq!(RIGHT - LEFT + 1, 50);
        assert_eq!(BOTTOM - TOP + 1, 50);
    }

    #[test]
    fn nested_triangles_and_rounded_incircles_fit_safe_region() {
        let mut previous_width = i32::MAX;
        for stage in [Stage::Preparing, Stage::AnswerPending, Stage::AnswerReady] {
            let [a, b, c] = stage.vertices();
            assert!(b.0 - c.0 < previous_width);
            previous_width = b.0 - c.0;
            for edge in 0..3 {
                assert!(Stroke::Edge(stage, edge)
                    .points()
                    .iter()
                    .all(|&(x, y)| (LEFT + 8..=RIGHT - 8).contains(&x)
                        && (TOP + 8..=BOTTOM - 8).contains(&y)));
            }
            let circle = Stroke::Auxiliary(stage).points();
            assert_eq!(circle.first(), circle.last());
            // Each side has a raster point within a pixel of tangency; every
            // circle point stays inside the triangle within rounding tolerance.
            for (p, q) in [(a, b), (b, c), (c, a)] {
                let distances: Vec<f64> = circle
                    .iter()
                    .map(|&(x, y)| {
                        ((q.0 - p.0) * (y - p.1) - (q.1 - p.1) * (x - p.0)) as f64
                            / ((q.0 - p.0) as f64).hypot((q.1 - p.1) as f64)
                    })
                    .collect();
                assert!(distances.iter().all(|d| *d >= -0.75));
                assert!(distances.iter().any(|d| d.abs() <= 0.75));
            }
        }
    }

    #[test]
    fn failure_segments_are_six_distinct_half_box_lines() {
        let segments: Vec<_> = [
            Failure::Selection,
            Failure::Transcription,
            Failure::Provider,
            Failure::NoSuccessor,
            Failure::InvalidSuccessor,
            Failure::Device,
        ]
        .into_iter()
        .map(Failure::segment)
        .collect();
        for (index, &(a, b)) in segments.iter().enumerate() {
            assert!(!segments[..index].contains(&(a, b)));
            assert!(a.0 == b.0 || a.1 == b.1);
            assert_eq!((a.0 - b.0).abs() + (a.1 - b.1).abs(), 25);
            for (x, y) in [a, b] {
                assert!((LEFT + 12..=LEFT + 37).contains(&x));
                assert!((TOP + 12..=TOP + 37).contains(&y));
            }
        }
    }

    #[test]
    fn deadlines_account_for_stroke_time_and_skip_overruns() {
        for (start, finish, want) in [(0, 100, 333), (333, 500, 666), (0, 333, 666), (0, 800, 999)]
        {
            assert_eq!(
                next_deadline(Duration::from_millis(start), Duration::from_millis(finish)),
                Duration::from_millis(want)
            );
        }
    }
}

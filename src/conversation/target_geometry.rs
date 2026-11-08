//! Exact historical-original-mask-v1. Geometry is correspondence, never a native lease.
use anyhow::{ensure, Context, Result};
use image::{
    codecs::png::{CompressionType, FilterType, PngEncoder},
    ImageEncoder,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io::Cursor};

pub const SCALE: u32 = 1_000_000;
pub const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PIXELS: u64 = 8_000_000;
pub const RECIPE: &str = "historical-original-mask-v1:image0.25.10/png0.18.1:rgb8-fast-nofilter";
pub type Point = [u32; 2];
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Geometry {
    pub components: Vec<Vec<Point>>,
}
fn cross(a: [i128; 2], b: [i128; 2], c: [i128; 2]) -> Result<i128> {
    (b[0] - a[0])
        .checked_mul(c[1] - a[1])
        .and_then(|left| {
            (b[1] - a[1])
                .checked_mul(c[0] - a[0])
                .and_then(|right| left.checked_sub(right))
        })
        .context("target geometry arithmetic overflow")
}
fn point(p: Point) -> [i128; 2] {
    [i128::from(p[0]), i128::from(p[1])]
}
fn on_segment(a: [i128; 2], b: [i128; 2], p: [i128; 2]) -> Result<bool> {
    Ok(cross(a, b, p)? == 0 && (0..2).all(|i| p[i] >= a[i].min(b[i]) && p[i] <= a[i].max(b[i])))
}
fn intersect(a: Point, b: Point, c: Point, d: Point) -> Result<bool> {
    let (a, b, c, d) = (point(a), point(b), point(c), point(d));
    let (ab_c, ab_d, cd_a, cd_b) = (
        cross(a, b, c)?,
        cross(a, b, d)?,
        cross(c, d, a)?,
        cross(c, d, b)?,
    );
    Ok((ab_c.signum() != ab_d.signum()
        && cd_a.signum() != cd_b.signum()
        && ab_c != 0
        && ab_d != 0
        && cd_a != 0
        && cd_b != 0)
        || on_segment(a, b, c)?
        || on_segment(a, b, d)?
        || on_segment(c, d, a)?
        || on_segment(c, d, b)?)
}
impl Geometry {
    pub fn canonicalize(mut self) -> Result<Self> {
        ensure!(
            !self.components.is_empty() && self.components.len() <= 8,
            "target component bound"
        );
        let total = self.components.iter().try_fold(0usize, |n, p| {
            n.checked_add(p.len()).context("target vertex overflow")
        })?;
        ensure!(total <= 512, "target vertex bound");
        for polygon in &mut self.components {
            ensure!((3..=64).contains(&polygon.len()), "target polygon size");
            ensure!(
                polygon.iter().all(|p| p[0] <= SCALE && p[1] <= SCALE),
                "target coordinate outside frame"
            );
            ensure!(
                polygon.iter().collect::<BTreeSet<_>>().len() == polygon.len(),
                "repeated target vertex"
            );
            let mut area = 0i128;
            for i in 0..polygon.len() {
                let a = point(polygon[i]);
                let b = point(polygon[(i + 1) % polygon.len()]);
                area = area
                    .checked_add(cross([0, 0], a, b)?)
                    .context("target area overflow")?;
                // Adjacent collinear reversal/overlap is not a simple polygon.
                let c = point(polygon[(i + 2) % polygon.len()]);
                ensure!(
                    !(cross(a, b, c)? == 0 && (on_segment(a, b, c)? || on_segment(b, c, a)?)),
                    "overlapping target edges"
                );
                for j in (i + 1)..polygon.len() {
                    if j == i + 1 || (i == 0 && j == polygon.len() - 1) {
                        continue;
                    }
                    ensure!(
                        !intersect(
                            polygon[i],
                            polygon[(i + 1) % polygon.len()],
                            polygon[j],
                            polygon[(j + 1) % polygon.len()]
                        )?,
                        "self-intersecting target polygon"
                    );
                }
            }
            ensure!(area != 0, "zero target area");
            if area < 0 {
                polygon.reverse();
            }
            let start = polygon
                .iter()
                .enumerate()
                .min_by_key(|(_, p)| **p)
                .unwrap()
                .0;
            polygon.rotate_left(start);
        }
        self.components.sort();
        ensure!(
            self.components.windows(2).all(|w| w[0] != w[1]),
            "duplicate target component"
        );
        Ok(self)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.clone().canonicalize()? == *self,
            "target geometry is not canonical"
        );
        Ok(())
    }
    fn includes(&self, x: u32, y: u32, width: u32, height: u32) -> Result<bool> {
        let p = [
            i128::from(2 * u64::from(x) + 1) * i128::from(SCALE),
            i128::from(2 * u64::from(y) + 1) * i128::from(SCALE),
        ];
        for polygon in &self.components {
            let mut inside = false;
            for i in 0..polygon.len() {
                let scaled = |v: Point| {
                    [
                        2 * i128::from(v[0]) * i128::from(width),
                        2 * i128::from(v[1]) * i128::from(height),
                    ]
                };
                let a = scaled(polygon[i]);
                let b = scaled(polygon[(i + 1) % polygon.len()]);
                if on_segment(a, b, p)? {
                    return Ok(true);
                }
                if (a[1] > p[1]) != (b[1] > p[1]) {
                    let side = cross(a, b, p)?;
                    if (side > 0) == (b[1] > a[1]) {
                        inside = !inside;
                    }
                }
            }
            if inside {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub fn mask_png(
        &self,
        original: &[u8],
        dimensions: [u32; 2],
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Vec<u8>> {
        self.validate()?;
        progress()?;
        let [width, height] = dimensions;
        ensure!(
            width > 0
                && height > 0
                && u64::from(width)
                    .checked_mul(u64::from(height))
                    .is_some_and(|n| n <= MAX_PIXELS),
            "target original pixel bound"
        );
        ensure!(
            !original.is_empty() && original.len() <= MAX_PNG_BYTES,
            "target original encoded bound"
        );
        let actual =
            image::ImageReader::with_format(Cursor::new(original), image::ImageFormat::Png)
                .into_dimensions()?;
        ensure!(actual == (width, height), "target original frame differs");
        let mut reader =
            image::ImageReader::with_format(Cursor::new(original), image::ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(width);
        limits.max_image_height = Some(height);
        limits.max_alloc = Some(MAX_PIXELS * 16);
        reader.limits(limits);
        let source = reader.decode()?.into_rgba8();
        let capacity = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|n| n.checked_mul(3))
            .context("target canvas overflow")?;
        let mut rgb = Vec::with_capacity(usize::try_from(capacity)?);
        for (offset, pixel) in source.pixels().enumerate() {
            if offset % 4096 == 0 {
                progress()?;
            }
            let x = u32::try_from(offset % width as usize)?;
            let y = u32::try_from(offset / width as usize)?;
            if self.includes(x, y, width, height)? {
                let alpha = u32::from(pixel[3]);
                for channel in &pixel.0[..3] {
                    rgb.push(
                        ((u32::from(*channel) * alpha + 255 * (255 - alpha) + 127) / 255) as u8,
                    );
                }
            } else {
                rgb.extend_from_slice(&[255, 255, 255]);
            }
        }
        progress()?;
        let mut output = Vec::new();
        PngEncoder::new_with_quality(&mut output, CompressionType::Fast, FilterType::NoFilter)
            .write_image(&rgb, width, height, image::ExtendedColorType::Rgb8)?;
        ensure!(output.len() <= MAX_PNG_BYTES, "target mask encoded bound");
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rectangle() -> Geometry {
        Geometry {
            components: vec![vec![[0, 0], [500_000, 0], [500_000, SCALE], [0, SCALE]]],
        }
        .canonicalize()
        .unwrap()
    }
    #[test]
    fn canonical_orientation_and_origin() {
        let a = rectangle();
        let mut b = a.clone();
        b.components[0].reverse();
        b.components[0].rotate_left(2);
        assert_eq!(b.canonicalize().unwrap(), a);
    }
    #[test]
    fn invalid_topology_refuses() {
        for points in [
            vec![[0, 0], [SCALE, SCALE], [0, SCALE], [SCALE, 0]],
            vec![[0, 0], [1, 1], [2, 2]],
            vec![[0, 0], [0, 0], [1, 1]],
            vec![[0, 0], [SCALE + 1, 0], [0, SCALE]],
        ] {
            assert!(Geometry {
                components: vec![points]
            }
            .canonicalize()
            .is_err());
        }
    }
    #[test]
    fn center_boundary_and_union_are_exact() {
        let g = rectangle();
        assert!(g.includes(0, 0, 2, 2).unwrap());
        assert!(!g.includes(1, 0, 2, 2).unwrap());
        assert!(g.includes(0, 0, 1, 1).unwrap());
        let mut union = g.clone();
        union.components.push(vec![
            [500_000, 0],
            [SCALE, 0],
            [SCALE, SCALE],
            [500_000, SCALE],
        ]);
        let union = union.canonicalize().unwrap();
        assert!(union.includes(1, 0, 2, 2).unwrap());
    }
    #[test]
    fn mask_preserves_canvas_and_alpha_and_is_deterministic() {
        let source =
            image::RgbaImage::from_raw(2, 1, vec![0, 20, 40, 128, 10, 30, 50, 255]).unwrap();
        let mut original = Vec::new();
        PngEncoder::new(&mut original)
            .write_image(source.as_raw(), 2, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        let g = rectangle();
        let a = g.mask_png(&original, [2, 1], &mut || Ok(())).unwrap();
        let b = g.mask_png(&original, [2, 1], &mut || Ok(())).unwrap();
        assert_eq!(a, b);
        let decoded = image::load_from_memory(&a).unwrap().into_rgb8();
        assert_eq!(decoded.dimensions(), (2, 1));
        assert_eq!(decoded.as_raw(), &[127, 137, 147, 255, 255, 255]);
        assert!(g.mask_png(&original, [1, 2], &mut || Ok(())).is_err());
        assert!(g
            .mask_png(&original, [2, 1], &mut || anyhow::bail!("cancel"))
            .is_err());
    }
}

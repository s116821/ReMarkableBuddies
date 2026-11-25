//! Admit and render the exact stock paper assets from the saved-source owner.
use super::super::static_png;
use anyhow::{bail, ensure, Context, Result};
use image::{codecs::png::PngDecoder, ImageDecoder, Limits};
use remarkable_rm::{
    templates::{decode_png, decode_svg},
    BackgroundImage, DEFAULT_HEIGHT, DEFAULT_WIDTH,
};
use roxmltree::Node;
use std::io::Cursor;

const MAX_TEMPLATE_PNG: usize = 1024 * 1024;
const MAX_TEMPLATE_SVG: usize = 32 * 1024;
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";

pub(super) fn background(
    svg: Option<&[u8]>,
    png: Option<&[u8]>,
    height: u32,
) -> Result<Option<BackgroundImage>> {
    let background = match (svg, png) {
        (Some(svg), Some(png)) => {
            let svg = svg_background(svg)?;
            let png = png_background(png)?;
            ensure!(
                match (&svg, &png) {
                    (None, None) => true,
                    (Some(svg), Some(png)) => svg
                        .rgba
                        .chunks_exact(4)
                        .zip(png.rgba.chunks_exact(4))
                        .all(|(left, right)| over_white(left) == over_white(right)),
                    _ => false,
                },
                "saved paper SVG and PNG variants disagree"
            );
            svg
        }
        (Some(svg), None) => svg_background(svg)?,
        (None, Some(png)) => png_background(png)?,
        (None, None) => bail!("saved page stock template unavailable"),
    };
    ensure!(
        background.is_none() || height == DEFAULT_HEIGHT,
        "patterned saved paper cannot extend beyond its native canvas"
    );
    Ok(background)
}

fn png_background(png: &[u8]) -> Result<Option<BackgroundImage>> {
    ensure!(
        png.len() <= MAX_TEMPLATE_PNG,
        "saved paper PNG exceeds budget"
    );
    static_png(png)?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(DEFAULT_WIDTH);
    limits.max_image_height = Some(DEFAULT_HEIGHT);
    limits.max_alloc = Some(u64::from(DEFAULT_WIDTH) * u64::from(DEFAULT_HEIGHT) * 8);
    let decoder = PngDecoder::with_limits(Cursor::new(png), limits)?;
    ensure!(
        matches!(
            decoder.dimensions(),
            (1, 1) | (DEFAULT_WIDTH, DEFAULT_HEIGHT)
        ),
        "unsupported saved paper PNG geometry"
    );
    drop(decoder);
    patterned(decode_png(png, DEFAULT_WIDTH, DEFAULT_HEIGHT)?)
}

fn svg_background(svg: &[u8]) -> Result<Option<BackgroundImage>> {
    ensure!(
        svg.len() <= MAX_TEMPLATE_SVG,
        "saved paper SVG exceeds budget"
    );
    let xml = roxmltree::Document::parse_with_options(
        std::str::from_utf8(svg)?,
        roxmltree::ParsingOptions {
            nodes_limit: 2048,
            ..roxmltree::ParsingOptions::default()
        },
    )?;
    let root = xml.root_element();
    ensure!(
        root.tag_name().namespace() == Some(SVG_NAMESPACE)
            && root.tag_name().name() == "svg"
            && attributes(root, &["width", "height", "viewBox", "version"])
            && root
                .attribute("viewBox")
                .is_none_or(|v| v == "0 0 1404 1872")
            && root.attribute("width").is_none_or(|v| v == "1404")
            && root.attribute("height").is_none_or(|v| v == "1872"),
        "unsupported saved paper SVG"
    );
    let native_geometry = root.attribute("viewBox").is_some()
        || (root.attribute("width").is_some() && root.attribute("height").is_some());
    let mut all_white = true;
    for shape in root.children().filter(|node| !ignorable(*node)) {
        ensure!(
            shape.is_element()
                && shape.tag_name().namespace() == Some(SVG_NAMESPACE)
                && shape.children().all(ignorable),
            "unsupported saved paper SVG content"
        );
        let colour = check_shape(shape)?;
        all_white &= colour == "white"
            || colour.eq_ignore_ascii_case("#fff")
            || colour.eq_ignore_ascii_case("#ffffff");
    }
    if all_white {
        return Ok(None);
    }
    ensure!(
        native_geometry,
        "patterned saved paper lacks native geometry"
    );
    let background = patterned(decode_svg(svg, DEFAULT_WIDTH, DEFAULT_HEIGHT)?)?;
    ensure!(
        background.is_some(),
        "patterned saved paper rendered without marks"
    );
    Ok(background)
}

fn check_shape<'a>(shape: Node<'a, '_>) -> Result<&'a str> {
    let width = DEFAULT_WIDTH as f32;
    let height = DEFAULT_HEIGHT as f32;
    let colour = match shape.tag_name().name() {
        "rect" => {
            ensure!(
                attributes(shape, &["x", "y", "width", "height", "fill"]),
                "unsupported saved paper rectangle"
            );
            let x = position(shape.attribute("x").unwrap_or("0"), width)?;
            let y = position(shape.attribute("y").unwrap_or("0"), height)?;
            ensure!(
                extent(shape.attribute("width"), width)? <= width - x
                    && extent(shape.attribute("height"), height)? <= height - y,
                "saved paper rectangle exceeds native geometry"
            );
            shape.attribute("fill")
        }
        "circle" => {
            ensure!(
                attributes(shape, &["cx", "cy", "r", "fill"]),
                "unsupported saved paper circle"
            );
            let x = position(
                shape.attribute("cx").context("paper circle x missing")?,
                width,
            )?;
            let y = position(
                shape.attribute("cy").context("paper circle y missing")?,
                height,
            )?;
            let radius = position(
                shape
                    .attribute("r")
                    .context("paper circle radius missing")?,
                width,
            )?;
            ensure!(
                radius > 0.0 && radius <= x.min(width - x).min(y).min(height - y),
                "saved paper circle exceeds native geometry"
            );
            shape.attribute("fill")
        }
        "line" => {
            ensure!(
                attributes(shape, &["x1", "x2", "y1", "y2", "stroke", "stroke-width"]),
                "unsupported saved paper line"
            );
            for attribute in ["x1", "x2"] {
                position(
                    shape.attribute(attribute).context("paper line x missing")?,
                    width,
                )?;
            }
            for attribute in ["y1", "y2"] {
                position(
                    shape.attribute(attribute).context("paper line y missing")?,
                    height,
                )?;
            }
            ensure!(
                position(shape.attribute("stroke-width").unwrap_or("1"), 32.0)? > 0.0,
                "invalid saved paper line width"
            );
            shape.attribute("stroke")
        }
        _ => bail!("unsupported saved paper SVG shape"),
    }
    .context("saved paper shape colour missing")?;
    ensure!(
        matches!(colour, "white" | "black" | "gray" | "grey")
            || colour.strip_prefix('#').is_some_and(|hex| {
                matches!(hex.len(), 3 | 6) && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
            }),
        "unsupported saved paper colour"
    );
    Ok(colour)
}

fn attributes(node: Node<'_, '_>, names: &[&str]) -> bool {
    node.attributes()
        .all(|attribute| attribute.namespace().is_none() && names.contains(&attribute.name()))
}

fn ignorable(node: Node<'_, '_>) -> bool {
    node.is_comment() || (node.is_text() && node.text().is_some_and(|text| text.trim().is_empty()))
}

fn position(value: &str, limit: f32) -> Result<f32> {
    let number: f32 = value.parse().context("invalid saved paper coordinate")?;
    ensure!(
        (0.0..=limit).contains(&number),
        "saved paper geometry exceeds bounds"
    );
    Ok(number)
}

fn extent(value: Option<&str>, limit: f32) -> Result<f32> {
    match value {
        Some("100%") => Ok(limit),
        Some(value) => position(value, limit),
        None => Ok(0.0),
    }
}

fn over_white(pixel: &[u8]) -> [u16; 3] {
    let clear = u16::from(u8::MAX - pixel[3]);
    std::array::from_fn(|i| u16::from(pixel[i]) + clear)
}

fn patterned(image: BackgroundImage) -> Result<Option<BackgroundImage>> {
    let is_white = |pixel: &[u8]| over_white(pixel) == [255; 3];
    ensure!(
        image.rgba.chunks_exact(4).any(is_white),
        "saved stock paper lacks a white base"
    );
    Ok(if image.rgba.chunks_exact(4).all(is_white) {
        None
    } else {
        Some(image)
    })
}

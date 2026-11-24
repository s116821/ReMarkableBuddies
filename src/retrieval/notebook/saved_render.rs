//! Bounded rendering for saved RM2 notebook pages admitted by the source owner.
use super::{static_png, Options, OrderedNotebookImages};
use crate::storage::Uuid;
use anyhow::{bail, ensure, Context, Result};
use image::{codecs::png::PngDecoder, ImageDecoder, Limits};
use remarkable_rm::{
    blocks::iter_blocks,
    primitives::Reader,
    scene::{
        items::{read_scene_item, ItemType},
        line::read_line,
        tree::{read_scene_tree_block, read_tree_node_block},
    },
    BlockType, CrdtId, Line, Pen, PenColor, RenderOptions, DEFAULT_HEIGHT, DEFAULT_WIDTH,
};
use std::{collections::BTreeSet, io::Cursor};

const MAX_SCENE_BYTES: usize = 256 * 1024;
const MAX_BLOCKS: usize = 512;
const MAX_POINTS: usize = 8_192;
const MAX_HEIGHT: u32 = DEFAULT_HEIGHT * 3 / 2;
// Each accepted asset fits the same page's bounded, sequential decode.
const MAX_TEMPLATE_PNG: usize = 1024 * 1024;
const MAX_TEMPLATE_SVG: usize = 32 * 1024;
const MAX_PNG: usize = 2 * 1024 * 1024;
const EDGE_MARGIN: f32 = 24.0;
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";

/// Borrowed bytes from the same pinned read used for the saved-source revision.
pub(crate) struct SavedPage<'a> {
    pub id: Uuid,
    pub scene: &'a [u8],
    pub template_svg: Option<&'a [u8]>,
    pub template_png: Option<&'a [u8]>,
}

#[derive(Default)]
pub(crate) struct NotebookRaster {
    originals: Vec<(Uuid, Vec<u8>)>,
    pixels: u64,
}

impl NotebookRaster {
    pub(crate) fn push(
        &mut self,
        page: SavedPage<'_>,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        let options = Options::default();
        ensure!(
            self.originals.len() < options.max_pages
                && !page.id.is_nil()
                && self.originals.iter().all(|(id, _)| *id != page.id),
            "unsupported saved notebook page count or identity"
        );
        progress()?;
        let height = checked_scene_extent(page.scene)?;
        let pixels = self.pixels + u64::from(DEFAULT_WIDTH) * u64::from(height);
        ensure!(
            pixels <= options.max_total_pixels,
            "saved notebook raster pixel budget exceeded"
        );
        verify_white_template(&page)?;
        progress()?;

        let scene =
            remarkable_rm::parse_page(page.scene).context("saved scene assembly refused")?;
        let png = remarkable_rm::render_page(
            &scene,
            RenderOptions {
                width: DEFAULT_WIDTH,
                height,
            },
        )
        .context("saved scene rasterization refused")?;
        ensure!(png.len() <= MAX_PNG, "saved scene PNG exceeds budget");
        progress()?;
        self.originals.push((page.id, png));
        self.pixels = pixels;
        Ok(())
    }

    pub(crate) fn images<T>(
        &self,
        document: Uuid,
        order: &[Uuid],
        buddy_pages: &BTreeSet<Uuid>,
        use_images: impl FnOnce(&OrderedNotebookImages<'_>) -> Result<T>,
    ) -> Result<T> {
        let originals: Vec<_> = self
            .originals
            .iter()
            .map(|(page, png)| (*page, png.as_slice()))
            .collect();
        use_images(&OrderedNotebookImages {
            document,
            order,
            originals: &originals,
            buddy_pages,
        })
    }
}

fn verify_white_template(page: &SavedPage<'_>) -> Result<()> {
    ensure!(
        page.template_svg.is_some() || page.template_png.is_some(),
        "saved page stock white template unavailable"
    );
    if let Some(svg) = page.template_svg {
        verify_white_svg(svg)?;
    }
    if let Some(png) = page.template_png {
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
            !decoder.is_apng()?
                && matches!(
                    decoder.dimensions(),
                    (1, 1) | (DEFAULT_WIDTH, DEFAULT_HEIGHT)
                ),
            "unsupported saved paper PNG geometry"
        );
        let mut pixels = vec![0; usize::try_from(decoder.total_bytes())?];
        decoder.read_image(&mut pixels)?;
        ensure!(
            pixels.iter().all(|&channel| channel == 255),
            "saved paper PNG is not uniform opaque white"
        );
    }
    Ok(())
}

fn verify_white_svg(svg: &[u8]) -> Result<()> {
    ensure!(
        svg.len() <= MAX_TEMPLATE_SVG,
        "saved paper SVG exceeds budget"
    );
    let xml = roxmltree::Document::parse_with_options(
        std::str::from_utf8(svg)?,
        roxmltree::ParsingOptions {
            nodes_limit: 16,
            ..roxmltree::ParsingOptions::default()
        },
    )?;
    let root = xml.root_element();
    ensure!(
        root.tag_name().namespace() == Some(SVG_NAMESPACE)
            && root.tag_name().name() == "svg"
            && root.attributes().all(|attr| attr.namespace().is_none()
                && matches!(attr.name(), "width" | "height" | "viewBox" | "version"))
            && root
                .attribute("viewBox")
                .is_none_or(|v| v == "0 0 1404 1872")
            && root.attribute("width").is_none_or(|v| v == "1404")
            && root.attribute("height").is_none_or(|v| v == "1872"),
        "unsupported saved paper SVG"
    );
    for child in root.children().filter(|node| !node.is_comment()) {
        if child.is_text() && child.text().is_some_and(|s| s.trim().is_empty()) {
            continue;
        }
        ensure!(
            child.is_element()
                && child.tag_name().namespace() == Some(SVG_NAMESPACE)
                && child.tag_name().name() == "rect"
                && child
                    .children()
                    .all(|node| node.is_text() && node.text().is_some_and(|s| s.trim().is_empty()))
                && child.attributes().all(|attr| attr.namespace().is_none()
                    && match attr.name() {
                        "x" | "y" => attr.value() == "0",
                        "width" => matches!(attr.value(), "1404" | "100%"),
                        "height" => matches!(attr.value(), "1872" | "100%"),
                        "fill" => matches!(attr.value(), "#fff" | "#ffffff" | "white"),
                        _ => false,
                    })
                && child.attribute("fill").is_some(),
            "saved paper SVG can paint nonwhite content"
        );
    }
    Ok(())
}

/// Check the records the upstream assembler can drop, plus the ink geometry
/// its rasterizer would otherwise clip. Only complete flat scenes qualify.
fn checked_scene_extent(bytes: &[u8]) -> Result<u32> {
    ensure!(
        bytes.len() <= MAX_SCENE_BYTES,
        "saved scene exceeds byte budget"
    );
    let mut trees = BTreeSet::new();
    let mut nodes = BTreeSet::new();
    let mut layers = BTreeSet::new();
    let mut items = BTreeSet::new();
    let mut line_parents = BTreeSet::new();
    let mut points = 0;
    let mut has_size = false;
    let mut bottom = DEFAULT_HEIGHT as f32;

    for (index, block) in iter_blocks(bytes)?.enumerate() {
        ensure!(index < MAX_BLOCKS, "saved scene exceeds block budget");
        let block = block?;
        let version = block.header.current_version;
        let kind = block.header.kind().context("unknown saved scene record")?;
        let mut body = block.body;
        match (kind, version) {
            (BlockType::MigrationInfo | BlockType::AuthorIds | BlockType::PageInfo, 1) => continue,
            (BlockType::SceneTree, 1) => {
                let tree = read_scene_tree_block(&mut body)?;
                ensure!(
                    tree.parent_id == CrdtId::ROOT && trees.insert(tree.node_id),
                    "nested or ambiguous saved scene tree"
                );
            }
            (BlockType::TreeNode, 1 | 2) => {
                let node = read_tree_node_block(&mut body)?;
                ensure!(
                    node.anchor.is_none() && node.visible.value && nodes.insert(node.node_id),
                    "unsupported saved scene layer"
                );
            }
            (BlockType::SceneGroupItem, 1) => {
                let group = read_scene_item(&mut body, ItemType::Group, |value| {
                    let id = value.read_id(2)?;
                    strict_value_end(value)?;
                    Ok(id)
                })?;
                ensure!(
                    group.parent_id == CrdtId::ROOT
                        && group.deleted_length == 0
                        && items.insert(group.item_id)
                        && group.value.is_some_and(|id| {
                            id != CrdtId::ROOT && id != CrdtId::SENTINEL && layers.insert(id)
                        }),
                    "unsupported saved scene group"
                );
            }
            (BlockType::SceneLineItem, 2) => {
                let stroke = read_scene_item(&mut body, ItemType::Line, |value| {
                    let line = read_line(value, version)?;
                    strict_value_end(value)?;
                    Ok(line)
                })?;
                ensure!(
                    stroke.deleted_length == 0 && items.insert(stroke.item_id),
                    "ambiguous saved stroke"
                );
                let line = stroke.value.context("missing saved stroke")?;
                line_parents.insert(stroke.parent_id);
                points += line.points.len();
                ensure!(points <= MAX_POINTS, "saved scene exceeds point budget");
                bottom = bottom.max(ink_bottom(&line)?);
            }
            (BlockType::SceneInfo, 1) => {
                ensure!(!has_size, "ambiguous saved scene geometry");
                body.read_lww_id(1)?;
                ensure!(
                    body.read_lww_bool(2)?.value && body.read_lww_bool(3)?.value,
                    "saved paper or scene is hidden"
                );
                let mut size = body.read_subblock(5)?;
                ensure!(
                    size.read_u32()? == DEFAULT_WIDTH
                        && size.read_u32()? == DEFAULT_HEIGHT
                        && size.is_eof(),
                    "unsupported saved scene paper size"
                );
                has_size = true;
            }
            _ => bail!("saved scene record or version is outside the supported profile"),
        }
        ensure!(body.is_eof(), "uninterpreted saved scene data");
    }
    nodes.remove(&CrdtId::ROOT);
    ensure!(
        has_size
            && !layers.is_empty()
            && layers == trees
            && layers == nodes
            && line_parents.is_subset(&layers),
        "saved scene layer accounting is incomplete"
    );
    Ok(bottom.ceil() as u32)
}

fn strict_value_end(value: &Reader<'_>) -> Result<(), remarkable_rm::ParseError> {
    if value.is_eof() {
        Ok(())
    } else {
        Err(remarkable_rm::ParseError::InvalidBlock(
            "uninterpreted saved drawable data",
        ))
    }
}

fn ink_bottom(line: &Line) -> Result<f32> {
    let first = line.points.first().context("saved stroke has no points")?;
    ensure!(
        line.tool == Pen::FinelinerV2
            && line.color == PenColor::Black
            && line.color_rgba.is_none_or(|color| color == [0, 0, 0, 255])
            && (0.5..=12.0).contains(&line.thickness_scale)
            && line.starting_length == 0.0
            && line.move_id.is_none()
            && line
                .points
                .iter()
                .any(|point| (point.x, point.y) != (first.x, first.y)),
        "saved stroke is outside the supported ink profile"
    );
    let half_width = DEFAULT_WIDTH as f32 / 2.0;
    let mut bottom = 0.0_f32;
    for point in &line.points {
        ensure!(
            point.x.is_finite()
                && point.y.is_finite()
                && point.x.abs() <= half_width - EDGE_MARGIN
                && (EDGE_MARGIN..=MAX_HEIGHT as f32 - EDGE_MARGIN).contains(&point.y),
            "saved ink exceeds the supported full canvas"
        );
        bottom = bottom.max(point.y + EDGE_MARGIN);
    }
    Ok(bottom)
}

#[cfg(test)]
mod tests;

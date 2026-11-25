//! Bounded rendering for saved RM2 notebook pages admitted by the source owner.
use super::{OrderedNotebookImages, PatternedPaper, MAX_ORDERED_NOTEBOOK_PAGES};
use crate::storage::Uuid;
use anyhow::{bail, ensure, Context, Result};
use remarkable_rm::{
    blocks::iter_blocks,
    primitives::Reader,
    scene::{
        items::{read_scene_item, ItemEnvelope, ItemType},
        line::read_line,
        tree::{read_scene_tree_block, read_tree_node_block},
    },
    BackgroundImage, BlockType, CrdtId, Layer, Line, Page, Pen, PenColor, RenderOptions,
    DEFAULT_HEIGHT, DEFAULT_WIDTH,
};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
};

const MAX_SCENE_BYTES: usize = 256 * 1024;
const MAX_BLOCKS: usize = 512;
const MAX_POINTS: usize = 8_192;
const MAX_HEIGHT: u32 = DEFAULT_HEIGHT * 3 / 2;
const MAX_PNG: usize = 2 * 1024 * 1024;
const MAX_RETAINED_PNG_BYTES: usize = 8 * 1024 * 1024;
const EDGE_MARGIN: f32 = 24.0;
mod paper;

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
    patterned_paper: BTreeMap<Uuid, PatternedPaper>,
    retained_png_bytes: usize,
}

impl NotebookRaster {
    pub(crate) fn push(
        &mut self,
        page: SavedPage<'_>,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        ensure!(
            self.originals.len() < MAX_ORDERED_NOTEBOOK_PAGES
                && !page.id.is_nil()
                && self.originals.iter().all(|(id, _)| *id != page.id),
            "unsupported saved notebook page count or identity"
        );
        progress()?;
        let (scene, height) = saved_scene(page.scene)?;
        let patterned = paper::is_patterned(page.template_svg, page.template_png, height)?;
        progress()?;

        // Keep white strokes visible while leaving all template pixels out of OCR.
        let needs_contrast = patterned
            && scene
                .layers
                .iter()
                .flat_map(|layer| &layer.lines)
                .any(|line| match line.color_rgba {
                    Some([red, green, blue, alpha]) => [red, green, blue] == [255; 3] && alpha != 0,
                    None => line.color == PenColor::White,
                });
        let matte = needs_contrast.then(|| BackgroundImage {
            width: 1,
            height: 1,
            rgba: vec![224, 224, 224, 255],
        });
        let png = remarkable_rm::render_page_with_background(
            &scene,
            RenderOptions {
                width: DEFAULT_WIDTH,
                height,
            },
            matte.as_ref(),
        )
        .context("saved scene rasterization refused")?;
        ensure!(
            png.len() <= MAX_PNG && self.retained_png_bytes + png.len() <= MAX_RETAINED_PNG_BYTES,
            "saved notebook PNG budget exceeded"
        );
        progress()?;
        self.retained_png_bytes += png.len();
        if patterned {
            let empty = scene.text.is_none()
                && scene
                    .layers
                    .iter()
                    .all(|layer| layer.lines.is_empty() && layer.glyphs.is_empty());
            self.patterned_paper.insert(
                page.id,
                if empty {
                    PatternedPaper::PaperOnly
                } else {
                    PatternedPaper::Ink
                },
            );
        }
        self.originals.push((page.id, png));
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
            patterned_paper: &self.patterned_paper,
        })
    }
}

/// Assemble the flat, fully accounted scene once, in the tablet's paint order.
fn saved_scene(bytes: &[u8]) -> Result<(Page, u32)> {
    ensure!(
        bytes.len() <= MAX_SCENE_BYTES,
        "saved scene exceeds byte budget"
    );
    let mut trees = BTreeSet::new();
    let mut nodes = BTreeMap::new();
    let mut layers = BTreeSet::new();
    let mut item_ids = BTreeSet::new();
    let mut groups = Vec::new();
    let mut lines: BTreeMap<_, Vec<_>> = BTreeMap::new();
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
                    tree.parent_id == CrdtId::ROOT && trees.insert(tree.tree_id),
                    "nested or ambiguous saved scene tree"
                );
            }
            (BlockType::TreeNode, 1 | 2) => {
                let node = read_tree_node_block(&mut body)?;
                ensure!(
                    node.anchor.is_none()
                        && node.visible.value
                        && nodes.insert(node.node_id, node.label.value).is_none(),
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
                        && item_ids.insert(group.item_id)
                        && group.value.is_some_and(|id| {
                            id != CrdtId::ROOT && id != CrdtId::SENTINEL && layers.insert(id)
                        }),
                    "unsupported saved scene group"
                );
                groups.push(group);
            }
            (BlockType::SceneLineItem, 2) => {
                let stroke = read_scene_item(&mut body, ItemType::Line, |value| {
                    let line = read_line(value, version)?;
                    strict_value_end(value)?;
                    Ok(line)
                })?;
                ensure!(
                    stroke.deleted_length == 0 && item_ids.insert(stroke.item_id),
                    "ambiguous saved stroke"
                );
                let line = stroke.value.as_ref().context("missing saved stroke")?;
                points += line.points.len();
                ensure!(points <= MAX_POINTS, "saved scene exceeds point budget");
                bottom = bottom.max(ink_bottom(line)?);
                lines.entry(stroke.parent_id).or_default().push(stroke);
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
                if !body.is_eof() {
                    check_portrait_layout(&mut body)?;
                }
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
            && nodes.keys().eq(layers.iter())
            && lines.keys().all(|id| layers.contains(id))
            && !item_ids.contains(&CrdtId::SENTINEL)
            && !item_ids.contains(&CrdtId::ROOT),
        "saved scene layer accounting is incomplete"
    );
    let mut ordered_layers = Vec::with_capacity(layers.len());
    for id in paint_order(groups)? {
        ordered_layers.push(Layer {
            node_id: id,
            name: nodes.remove(&id).context("missing saved layer")?,
            visible: true,
            lines: paint_order(lines.remove(&id).unwrap_or_default())?,
            glyphs: Vec::new(),
        });
    }
    Ok((
        Page {
            layers: ordered_layers,
            text: None,
            paper_size: Some((DEFAULT_WIDTH, DEFAULT_HEIGHT)),
        },
        bottom.ceil() as u32,
    ))
}

// Stock RM2 pages can include a zero rectangle, two copies of the paper size
// and an enabled flag after SceneInfo's integer paper size. Validate the
// captured layout values independently of their last-writer timestamps.
fn check_portrait_layout(body: &mut Reader<'_>) -> Result<()> {
    fn stored_value<'a>(body: &mut Reader<'a>, index: u32) -> Result<Reader<'a>> {
        let mut field = body.read_subblock(index)?;
        field.read_id(1)?;
        let value = field.read_subblock(2)?;
        ensure!(field.is_eof(), "uninterpreted saved layout field");
        Ok(value)
    }

    let mut rectangle = stored_value(body, 6)?;
    for _ in 0..4 {
        ensure!(
            rectangle.read_f64()? == 0.0,
            "unsupported saved layout rectangle"
        );
    }
    ensure!(rectangle.is_eof(), "uninterpreted saved layout rectangle");
    for mut size in [body.read_subblock(7)?, stored_value(body, 8)?] {
        ensure!(
            size.read_f64()? == f64::from(DEFAULT_WIDTH)
                && size.read_f64()? == f64::from(DEFAULT_HEIGHT)
                && size.is_eof(),
            "unsupported saved layout dimensions"
        );
    }
    let mut flag = stored_value(body, 9)?;
    ensure!(
        flag.read_u8()? == 1 && flag.is_eof(),
        "unsupported saved layout flag"
    );
    Ok(())
}

// Sibling links determine paint order. For simultaneous inserts, rmscene's
// tablet rule is higher author first, then lower sequence number. The pinned
// renderer's generic assembler breaks drawing ties in ascending author order.
fn paint_order<T>(mut items: Vec<ItemEnvelope<T>>) -> Result<Vec<T>> {
    let indices: BTreeMap<_, _> = items
        .iter()
        .enumerate()
        .map(|(i, item)| (item.item_id, i))
        .collect();
    let priority = |id: CrdtId, index| (Reverse(id.author), id.seq, index);
    let mut pending = vec![0_usize; items.len()];
    let mut next = vec![Vec::new(); items.len()];
    for (i, item) in items.iter().enumerate() {
        for (other, is_left) in [(item.left_id, true), (item.right_id, false)] {
            if other == CrdtId::SENTINEL {
                continue;
            }
            let &j = indices
                .get(&other)
                .context("missing saved paint neighbour")?;
            let (before, after) = if is_left { (j, i) } else { (i, j) };
            next[before].push(after);
            pending[after] += 1;
        }
    }
    let mut ready: BTreeSet<_> = (0..items.len())
        .filter(|&i| pending[i] == 0)
        .map(|i| priority(items[i].item_id, i))
        .collect();
    let mut values = Vec::with_capacity(items.len());
    while let Some((_, _, i)) = ready.pop_first() {
        values.push(items[i].value.take().context("missing saved paint item")?);
        for &j in &next[i] {
            pending[j] -= 1;
            if pending[j] == 0 {
                ready.insert(priority(items[j].item_id, j));
            }
        }
    }
    ensure!(values.len() == items.len(), "cyclic saved paint order");
    Ok(values)
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
    ensure!(
        (0.5..=12.0).contains(&line.thickness_scale)
            && line.starting_length == 0.0
            && line.move_id.is_none()
            && !line.points.is_empty(),
        "saved stroke is outside the supported ink profile"
    );
    // Bound the painted extent before allocating the canvas. The renderer
    // applies the recorded pressure, speed, tilt, colour and blending.
    let widest = f32::from(line.points.iter().map(|p| p.width).max().unwrap_or(0)) / 4.0;
    let scale = line.thickness_scale as f32;
    let diameter = match line.tool {
        Pen::FinelinerV1 | Pen::FinelinerV2 => 1.8 * scale,
        Pen::BallpointV1 | Pen::BallpointV2 => widest + 1.5,
        Pen::PencilV1 | Pen::PencilV2 => 10.0 * scale,
        Pen::MechanicalPencilV1 | Pen::MechanicalPencilV2 => scale * scale,
        Pen::PaintbrushV1 | Pen::PaintbrushV2 | Pen::Calligraphy => 2.0 * widest,
        Pen::MarkerV1 | Pen::MarkerV2 | Pen::HighlighterV1 | Pen::HighlighterV2 | Pen::Shader => {
            widest
        }
        Pen::Eraser | Pen::EraserAreaSelect => bail!("saved scene contains an unapplied eraser"),
    };
    let radius = if matches!(line.tool, Pen::HighlighterV1 | Pen::HighlighterV2) {
        diameter * std::f32::consts::FRAC_1_SQRT_2 // square cap at any angle
    } else {
        diameter * 0.5
    };
    let margin = (radius + 1.0).ceil().max(EDGE_MARGIN);
    let half_width = DEFAULT_WIDTH as f32 / 2.0;
    let mut bottom = 0.0_f32;
    for point in &line.points {
        ensure!(
            point.x.is_finite()
                && point.y.is_finite()
                && point.x.abs() <= half_width - margin
                && (margin..=MAX_HEIGHT as f32 - margin).contains(&point.y),
            "saved ink exceeds the supported full canvas"
        );
        bottom = bottom.max(point.y + margin);
    }
    Ok(bottom)
}

#[cfg(test)]
mod tests;

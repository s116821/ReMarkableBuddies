use super::*;

const HEADER: &[u8] = b"reMarkable .lines file, version=6          ";
const WHITE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1404 1872"><rect width="100%" height="100%" fill="#fff"/></svg>"##;
const NONWHITE: &[u8] =
    br##"<svg xmlns="http://www.w3.org/2000/svg"><rect width="100%" height="100%" fill="#000"/></svg>"##;

fn record(scene: &mut Vec<u8>, kind: u8, version: u8, payload: &[u8]) {
    scene.extend((payload.len() as u32).to_le_bytes());
    scene.extend([0, version, version, kind]);
    scene.extend(payload);
}

fn subblock(target: &mut Vec<u8>, tag: u8, payload: &[u8]) {
    target.push(tag);
    target.extend((payload.len() as u32).to_le_bytes());
    target.extend(payload);
}

fn scene(stroke: Option<(f32, u32)>) -> Vec<u8> {
    scene_with_samples(stroke.map(|(y, tool)| (y, tool, &[-20.0, 20.0][..])))
}

fn scene_with_samples(stroke: Option<(f32, u32, &[f32])>) -> Vec<u8> {
    let mut scene = HEADER.to_vec();
    let mut info = Vec::new();
    subblock(&mut info, 0x1c, &[0x1f, 0, 0, 0x2f, 0, 2]);
    subblock(&mut info, 0x2c, &[0x1f, 0, 0, 0x21, 1]);
    subblock(&mut info, 0x3c, &[0x1f, 0, 0, 0x21, 1]);
    let size = [DEFAULT_WIDTH.to_le_bytes(), DEFAULT_HEIGHT.to_le_bytes()].concat();
    subblock(&mut info, 0x5c, &size);
    record(&mut scene, 13, 1, &info);

    append_layer(&mut scene, 2, [0, 3], [0, 0]);

    if let Some((y, tool, samples)) = stroke {
        TestStroke {
            tool,
            ..Default::default()
        }
        .append(&mut scene, y, samples);
    }
    scene
}

fn append_layer(scene: &mut Vec<u8>, layer_id: u8, group_id: [u8; 2], left: [u8; 2]) {
    let mut tree = vec![0x1f, 0, layer_id, 0x2f, 0, layer_id, 0x31, 0];
    subblock(&mut tree, 0x4c, &[0x1f, 0, 1]);
    record(scene, 1, 1, &tree);
    let mut layer = vec![0x1f, 0, layer_id];
    subblock(&mut layer, 0x2c, &[0x1f, 0, 0, 0x2c, 2, 0, 0, 0, 0, 1]);
    subblock(&mut layer, 0x3c, &[0x1f, 0, 0, 0x21, 1]);
    record(scene, 2, 1, &layer);

    let mut group = vec![0x1f, 0, 1, 0x2f];
    group.extend(group_id);
    group.push(0x3f);
    group.extend(left);
    group.extend([0x4f, 0, 0, 0x54, 0, 0, 0, 0]);
    subblock(&mut group, 0x6c, &[2, 0x2f, 0, layer_id]);
    record(scene, 4, 1, &group);
}

#[derive(Clone, Copy)]
struct TestStroke {
    // The fixture's sequence numbers all fit in a single varuint byte.
    id: [u8; 2],
    left: [u8; 2],
    layer: u8,
    tool: u32,
    color: remarkable_rm::PenColor,
    width: u16,
}

impl Default for TestStroke {
    fn default() -> Self {
        Self {
            id: [0, 4],
            left: [0, 0],
            layer: 2,
            tool: Pen::FinelinerV2 as u32,
            color: remarkable_rm::PenColor::Black,
            width: 8,
        }
    }
}

impl TestStroke {
    fn append(self, scene: &mut Vec<u8>, y: f32, samples: &[f32]) {
        let mut ink = vec![3, 0x14];
        ink.extend(self.tool.to_le_bytes());
        ink.push(0x24);
        ink.extend((self.color as u32).to_le_bytes());
        ink.push(0x38);
        ink.extend(2.0_f64.to_le_bytes());
        ink.extend([0x44, 0, 0, 0, 0]);
        let mut points = Vec::new();
        for &x in samples {
            points.extend(x.to_le_bytes());
            points.extend(y.to_le_bytes());
            points.extend([0, 0]);
            points.extend(self.width.to_le_bytes());
            points.extend([0, 255]);
        }
        subblock(&mut ink, 0x5c, &points);
        ink.extend([0x6f, 0, 5]);
        let mut envelope = vec![0x1f, 0, self.layer, 0x2f];
        envelope.extend(self.id);
        envelope.push(0x3f);
        envelope.extend(self.left);
        envelope.extend([0x4f, 0, 0, 0x54, 0, 0, 0, 0]);
        subblock(&mut envelope, 0x6c, &ink);
        record(scene, 5, 2, &envelope);
    }
}

fn saved_page(id: Uuid, bytes: &[u8]) -> SavedPage<'_> {
    SavedPage {
        id,
        scene: bytes,
        template_svg: Some(WHITE),
        template_png: None,
    }
}

#[test]
fn saved_render_keeps_blank_page_and_full_extent_in_native_order() -> Result<()> {
    let document = Uuid::new_v4();
    let originals = [(); 4].map(|_| Uuid::new_v4());
    let buddy = Uuid::new_v4();
    let last = Uuid::new_v4();
    let last_y = MAX_HEIGHT - 24;
    let mut raster = NotebookRaster::default();
    for id in originals {
        raster.push(saved_page(id, &scene(None)), &mut || Ok(()))?;
    }
    raster.push(
        saved_page(last, &scene(Some((last_y as f32, Pen::FinelinerV2 as u32)))),
        &mut || Ok(()),
    )?;

    let order = [
        originals[0],
        buddy,
        originals[1],
        originals[2],
        originals[3],
        last,
    ];
    let expected = [originals.as_slice(), &[last]].concat();
    raster.images(document, &order, &BTreeSet::from([buddy]), |images| {
        assert_eq!(
            images
                .originals
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            expected
        );
        let blank = image::load_from_memory(images.originals[0].1)?.to_luma8();
        let extended = image::load_from_memory(images.originals[4].1)?.to_luma8();
        assert_eq!(blank.dimensions(), (DEFAULT_WIDTH, DEFAULT_HEIGHT));
        assert!(blank.as_raw().iter().all(|&channel| channel == 255));
        assert_eq!(extended.dimensions(), (DEFAULT_WIDTH, last_y + 24));
        assert!(extended.get_pixel(DEFAULT_WIDTH / 2, last_y)[0] < 240);
        assert_eq!(
            extended.get_pixel(DEFAULT_WIDTH / 2, DEFAULT_HEIGHT - 1)[0],
            255
        );
        Ok(())
    })
}

#[test]
fn saved_render_preserves_single_and_repeated_punctuation_dots() -> Result<()> {
    let mut rasters = Vec::new();
    for samples in [&[0.0][..], &[0.0, 0.0, 0.0]] {
        let scene = scene_with_samples(Some((340.0, Pen::FinelinerV2 as u32, samples)));
        let mut raster = NotebookRaster::default();
        raster.push(saved_page(Uuid::new_v4(), &scene), &mut || Ok(()))?;
        let image = image::load_from_memory(&raster.originals[0].1)?.to_luma8();
        assert!(image.get_pixel(DEFAULT_WIDTH / 2, 340)[0] < 240);
        assert_eq!(image.get_pixel(DEFAULT_WIDTH / 2 + 6, 340)[0], 255);
        rasters.push(image);
    }
    assert_eq!(rasters[0], rasters[1]);
    Ok(())
}

#[test]
fn saved_render_refuses_partial_interpretation() -> Result<()> {
    let id = Uuid::new_v4();
    let mut typed = scene(None);
    record(&mut typed, 7, 1, &[]);
    let mut unknown = scene(None);
    record(&mut unknown, 0xF0, 1, &[]);
    let erased = scene(Some((400.0, Pen::Eraser as u32)));
    let cropped = scene(Some((MAX_HEIGHT as f32, Pen::FinelinerV2 as u32)));
    let mut raster = NotebookRaster::default();
    for bytes in [HEADER, &typed, &unknown, &erased, &cropped] {
        assert!(raster.push(saved_page(id, bytes), &mut || Ok(())).is_err());
        assert!(raster.originals.is_empty());
    }
    let blank = scene(None);
    let mut page = saved_page(id, &blank);
    page.template_svg = Some(NONWHITE);
    assert!(raster.push(page, &mut || Ok(())).is_err());
    let mut page = saved_page(id, &blank);
    page.template_svg = None;
    assert!(raster.push(page, &mut || Ok(())).is_err());
    assert!(raster.originals.is_empty());
    Ok(())
}

const STOCK_PENS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/notebook-ink/pens-small.rm"
));

#[test]
fn published_rm2_page_preserves_nine_pens_in_the_notebook_input() -> Result<()> {
    let page = Uuid::new_v4();
    let mut raster = NotebookRaster::default();
    raster.push(saved_page(page, STOCK_PENS), &mut || Ok(()))?;
    raster.images(Uuid::new_v4(), &[page], &BTreeSet::new(), |images| {
        assert_eq!(images.originals.len(), 1);
        assert_eq!(images.originals[0].0, page);
        let image = image::load_from_memory(images.originals[0].1)?.to_rgb8();
        assert_eq!(image.dimensions(), (DEFAULT_WIDTH, DEFAULT_HEIGHT));
        let samples = [
            ("fineliner", 241, 235, [0, 0, 0]),
            ("highlighter", 180, 330, [255, 237, 117]),
            ("ballpoint", 320, 435, [75, 120, 210]),
            ("pencil", 352, 550, [208, 67, 20]),
            ("mechanical pencil", 343, 680, [166, 166, 166]),
            ("calligraphy", 293, 860, [0, 0, 0]),
            ("marker", 210, 1098, [0, 0, 0]),
            ("shader", 205, 1240, [199, 199, 198]),
            ("paintbrush", 396, 1480, [204, 51, 0]),
        ];
        for (pen, x, y, rgb) in samples {
            let visible = (y - 2..=y + 2).any(|row| {
                (x - 2..=x + 2).any(|col| {
                    image
                        .get_pixel(col, row)
                        .0
                        .iter()
                        .zip(rgb)
                        .all(|(&actual, expected)| actual.abs_diff(expected) <= 12)
                })
            });
            assert!(visible, "missing {pen} near ({x}, {y})");
        }
        assert_eq!(image.get_pixel(1250, 1700).0, [255, 255, 255]);
        Ok(())
    })
}

#[test]
fn overlapping_inks_follow_sibling_links_before_concurrent_author_order() -> Result<()> {
    use remarkable_rm::PenColor;
    let red = TestStroke {
        id: [1, 4],
        color: PenColor::Red,
        ..Default::default()
    };
    let blue = TestStroke {
        id: [2, 4],
        color: PenColor::Blue,
        ..Default::default()
    };
    let after_red = TestStroke {
        left: red.id,
        ..blue
    };
    for (ordered, red_on_top) in [
        ([red, blue], true),
        ([blue, red], true),
        ([after_red, red], false),
    ] {
        let mut scene = scene(None);
        for stroke in ordered {
            stroke.append(&mut scene, 500.0, &[-30.0, 30.0]);
        }
        let mut raster = NotebookRaster::default();
        raster.push(saved_page(Uuid::new_v4(), &scene), &mut || Ok(()))?;
        let image = image::load_from_memory(&raster.originals[0].1)?.to_rgb8();
        let [r, _, b] = image.get_pixel(DEFAULT_WIDTH / 2, 500).0;
        assert!(if red_on_top { r > b } else { b > r });
    }
    for invalid in [
        [
            TestStroke {
                left: [99, 1],
                ..red
            },
            blue,
        ],
        [
            TestStroke {
                left: blue.id,
                ..red
            },
            after_red,
        ],
    ] {
        let mut scene = scene(None);
        for stroke in invalid {
            stroke.append(&mut scene, 500.0, &[-30.0, 30.0]);
        }
        assert!(saved_scene(&scene).is_err());
    }
    Ok(())
}

#[test]
fn overlapping_layers_follow_the_root_group_order() -> Result<()> {
    use remarkable_rm::PenColor;
    let red = TestStroke {
        id: [1, 4],
        color: PenColor::Red,
        ..Default::default()
    };
    let blue = TestStroke {
        id: [2, 4],
        layer: 7,
        color: PenColor::Blue,
        ..Default::default()
    };
    // With independent groups the higher author draws first. Linking blue's
    // group after red's reverses the visible colour at the same crossing.
    for (left, red_on_top) in [([0, 0], true), ([0, 3], false)] {
        let mut scene = scene(None);
        append_layer(&mut scene, blue.layer, [1, 3], left);
        blue.append(&mut scene, 500.0, &[-30.0, 30.0]);
        red.append(&mut scene, 500.0, &[-30.0, 30.0]);
        let mut raster = NotebookRaster::default();
        raster.push(saved_page(Uuid::new_v4(), &scene), &mut || Ok(()))?;
        let image = image::load_from_memory(&raster.originals[0].1)?.to_rgb8();
        let [r, _, b] = image.get_pixel(DEFAULT_WIDTH / 2, 500).0;
        assert!(if red_on_top { r > b } else { b > r });
    }
    Ok(())
}

#[test]
fn broad_highlighter_is_included_to_its_edge_or_refused_before_capture() -> Result<()> {
    let marker = TestStroke {
        tool: Pen::HighlighterV2 as u32,
        color: remarkable_rm::PenColor::Yellow,
        width: 320,
        ..Default::default()
    };
    let y = DEFAULT_HEIGHT + 120;
    let mut extended = scene(None);
    marker.append(&mut extended, y as f32, &[-90.0, 90.0]);
    let mut raster = NotebookRaster::default();
    raster.push(saved_page(Uuid::new_v4(), &extended), &mut || Ok(()))?;
    let painted = image::load_from_memory(&raster.originals[0].1)?.to_rgb8();
    assert!(painted.height() >= y + 58);
    assert!(painted.get_pixel(DEFAULT_WIDTH / 2, y + 36)[2] < 240);
    let mut clipped = scene(None);
    marker.append(&mut clipped, (MAX_HEIGHT - 45) as f32, &[-90.0, 90.0]);
    assert!(saved_scene(&clipped).is_err());
    Ok(())
}

#[test]
fn nonstandard_portrait_layout_remains_unrenderable() {
    let mut scene = STOCK_PENS.to_vec();
    let mut extent = vec![0x7c, 16, 0, 0, 0];
    extent.extend(f64::from(DEFAULT_WIDTH).to_le_bytes());
    extent.extend(f64::from(DEFAULT_HEIGHT).to_le_bytes());
    let positions: Vec<_> = scene
        .windows(extent.len())
        .enumerate()
        .filter_map(|(i, field)| (field == extent).then_some(i))
        .collect();
    assert_eq!(positions.len(), 1);
    let width = positions[0] + 5;
    scene[width..width + 8].copy_from_slice(&f64::from(DEFAULT_WIDTH - 1).to_le_bytes());
    assert!(saved_scene(&scene).is_err());
}

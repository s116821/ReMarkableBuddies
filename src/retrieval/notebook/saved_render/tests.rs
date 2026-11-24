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
    let mut scene = HEADER.to_vec();
    let mut info = Vec::new();
    subblock(&mut info, 0x1c, &[0x1f, 0, 0, 0x2f, 0, 2]);
    subblock(&mut info, 0x2c, &[0x1f, 0, 0, 0x21, 1]);
    subblock(&mut info, 0x3c, &[0x1f, 0, 0, 0x21, 1]);
    let size = [DEFAULT_WIDTH.to_le_bytes(), DEFAULT_HEIGHT.to_le_bytes()].concat();
    subblock(&mut info, 0x5c, &size);
    record(&mut scene, 13, 1, &info);

    let mut tree = vec![0x1f, 0, 2, 0x2f, 0, 2, 0x31, 0];
    subblock(&mut tree, 0x4c, &[0x1f, 0, 1]);
    record(&mut scene, 1, 1, &tree);
    let mut layer = vec![0x1f, 0, 2];
    subblock(&mut layer, 0x2c, &[0x1f, 0, 0, 0x2c, 2, 0, 0, 0, 0, 1]);
    subblock(&mut layer, 0x3c, &[0x1f, 0, 0, 0x21, 1]);
    record(&mut scene, 2, 1, &layer);

    let group = [
        0x1f, 0, 1, 0x2f, 0, 3, 0x3f, 0, 0, 0x4f, 0, 0, 0x54, 0, 0, 0, 0,
    ];
    let mut group = group.to_vec();
    subblock(&mut group, 0x6c, &[2, 0x2f, 0, 2]);
    record(&mut scene, 4, 1, &group);

    if let Some((y, tool)) = stroke {
        let mut ink = vec![3, 0x14];
        ink.extend(tool.to_le_bytes());
        ink.extend([0x24, 0, 0, 0, 0, 0x38]);
        ink.extend(2.0_f64.to_le_bytes());
        ink.extend([0x44, 0, 0, 0, 0]);
        let mut points = Vec::new();
        for x in [-20_f32, 20_f32] {
            points.extend(x.to_le_bytes());
            points.extend(y.to_le_bytes());
            points.extend([0, 0, 8, 0, 0, 255]);
        }
        subblock(&mut ink, 0x5c, &points);
        ink.extend([0x6f, 0, 5]);
        let mut envelope = vec![
            0x1f, 0, 2, 0x2f, 0, 4, 0x3f, 0, 0, 0x4f, 0, 0, 0x54, 0, 0, 0, 0,
        ];
        subblock(&mut envelope, 0x6c, &ink);
        record(&mut scene, 5, 2, &envelope);
    }
    scene
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
    let first = Uuid::new_v4();
    let buddy = Uuid::new_v4();
    let last = Uuid::new_v4();
    let last_y = DEFAULT_HEIGHT + 200;
    let mut raster = NotebookRaster::default();
    raster.push(saved_page(first, &scene(None)), &mut || Ok(()))?;
    raster.push(
        saved_page(last, &scene(Some((last_y as f32, Pen::FinelinerV2 as u32)))),
        &mut || Ok(()),
    )?;

    raster.images(
        document,
        &[first, buddy, last],
        &BTreeSet::from([buddy]),
        |images| {
            assert_eq!(
                images
                    .originals
                    .iter()
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>(),
                [first, last]
            );
            let blank = image::load_from_memory(images.originals[0].1)?.to_luma8();
            let extended = image::load_from_memory(images.originals[1].1)?.to_luma8();
            assert_eq!(blank.dimensions(), (DEFAULT_WIDTH, DEFAULT_HEIGHT));
            assert!(blank.as_raw().iter().all(|&channel| channel == 255));
            assert_eq!(extended.dimensions(), (DEFAULT_WIDTH, last_y + 24));
            assert!(extended.get_pixel(DEFAULT_WIDTH / 2, last_y)[0] < 240);
            assert_eq!(
                extended.get_pixel(DEFAULT_WIDTH / 2, DEFAULT_HEIGHT - 1)[0],
                255
            );
            Ok(())
        },
    )
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

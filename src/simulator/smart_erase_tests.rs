//! Geometry controls through the actual Workflow and recording simulator.
use super::*;
use crate::analysis::BoundingBox;
use image::{DynamicImage, GrayImage, Luma};

#[test]
fn actual_smart_erase_preserves_overlapping_order_bounds_and_blank_noop() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/simulator/scenarios");
    let scenario: Scenario =
        serde_json::from_slice(&std::fs::read(root.join("blank-answer.json")).unwrap()).unwrap();
    for (region, ink, rows) in [
        (
            BoundingBox {
                x: 4,
                y: 1,
                width: 4,
                height: 5,
            },
            vec![(5, 2), (5, 3), (12, 2)],
            vec![1, 2, 3, 4, 1, 2, 3, 4, 5],
        ),
        (
            BoundingBox {
                x: 766,
                y: 1020,
                width: 5,
                height: 10,
            },
            vec![(767, 1022), (767, 1023)],
            vec![1020, 1021, 1022, 1023, 1021, 1022, 1023],
        ),
        (
            BoundingBox {
                x: 4,
                y: 1,
                width: 4,
                height: 5,
            },
            vec![(12, 2)],
            vec![],
        ),
    ] {
        let state = Rc::new(RefCell::new(State::new(&scenario, &root).unwrap()));
        let mut workflow = Workflow::with_device(Box::new(SimDevice(state.clone())), false);
        let mut image = GrayImage::from_pixel(768, 1024, Luma([255]));
        for (x, y) in ink {
            image.put_pixel(x, y, Luma([0]));
        }
        let mut bytes = std::io::Cursor::new(Vec::new());
        DynamicImage::ImageLuma8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        workflow
            .erase_region_smart(&region, bytes.get_ref())
            .unwrap();
        let actual = state
            .borrow()
            .events
            .iter()
            .filter(|event| event.action == "erase")
            .map(|event| event.detail.clone())
            .collect::<Vec<_>>();
        let expected = rows
            .into_iter()
            .map(|y| {
                format!(
                    "{:?} {:?}",
                    (region.x, y),
                    ((region.x + region.width).min(768), y + 1)
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }
}

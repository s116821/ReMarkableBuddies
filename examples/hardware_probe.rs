//! Explicit, offline hardware actions for a disposable test document.
#[cfg(target_os = "linux")]
use anyhow::{bail, Result};
#[cfg(target_os = "linux")]
use remarkable_reader_buddy::{Keyboard, Pen, Screenshot, Touch, TriggerCorner};
#[cfg(target_os = "linux")]
use std::{thread::sleep, time::Duration};

#[cfg(target_os = "linux")]
fn main() -> Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("tap") | Some("press") => {
            let x = args
                .get(2)
                .ok_or_else(|| anyhow::anyhow!("x required"))?
                .parse()?;
            let y = args
                .get(3)
                .ok_or_else(|| anyhow::anyhow!("y required"))?
                .parse()?;
            let mut touch = Touch::new(false, TriggerCorner::LowerLeft);
            touch.touch_start((x, y))?;
            sleep(Duration::from_millis(if args[1] == "press" {
                2000
            } else {
                100
            }));
            touch.touch_stop()?;
        }
        Some("strokes") => {
            let path = args
                .get(2)
                .ok_or_else(|| anyhow::anyhow!("stroke JSON path required"))?;
            let strokes: Vec<Vec<(i32, i32)>> = serde_json::from_slice(&std::fs::read(path)?)?;
            let mut pen = Pen::new(false);
            for stroke in strokes {
                for segment in stroke.windows(2) {
                    pen.draw_line_screen(segment[0], segment[1])?;
                }
            }
        }
        Some("line") => Pen::new(false).draw_line_screen((250, 400), (500, 500))?,
        Some("question") => {
            let mut pen = Pen::new(false);
            let strokes: Vec<Vec<(i32, i32)>> = vec![
                vec![
                    (280, 387),
                    (282, 382),
                    (287, 380),
                    (294, 380),
                    (299, 384),
                    (300, 389),
                    (298, 394),
                    (280, 415),
                    (302, 415),
                ],
                vec![(315, 397), (335, 397)],
                vec![(325, 387), (325, 407)],
                vec![
                    (350, 387),
                    (352, 382),
                    (357, 380),
                    (364, 380),
                    (369, 384),
                    (370, 389),
                    (368, 394),
                    (350, 415),
                    (372, 415),
                ],
                vec![(385, 390), (405, 390)],
                vec![(385, 403), (405, 403)],
                vec![
                    (425, 385),
                    (430, 380),
                    (445, 380),
                    (450, 385),
                    (450, 393),
                    (438, 400),
                    (438, 405),
                ],
                vec![(438, 413), (439, 415)],
                vec![(260, 355), (475, 355), (475, 440), (260, 440), (260, 355)],
            ];
            for stroke in strokes {
                for segment in stroke.windows(2) {
                    pen.draw_line_screen(
                        (
                            (segment[0].0 - 260) * 2 + 150,
                            (segment[0].1 - 355) * 2 + 300,
                        ),
                        (
                            (segment[1].0 - 260) * 2 + 150,
                            (segment[1].1 - 355) * 2 + 300,
                        ),
                    )?;
                }
            }
        }
        Some("outline") => {
            let mut pen = Pen::new(false);
            for (a, b) in [
                ((100, 350), (650, 350)),
                ((650, 350), (650, 600)),
                ((650, 600), (100, 600)),
                ((100, 600), (100, 350)),
            ] {
                pen.draw_line_screen(a, b)?;
            }
        }
        Some("stationary-hold") => {
            let mut touch = Touch::new(false, TriggerCorner::LowerLeft);
            touch.touch_start((30, 970))?;
            sleep(Duration::from_secs(3));
            touch.touch_stop()?;
        }
        Some("hold") => {
            let mut touch = Touch::new(false, TriggerCorner::LowerLeft);
            touch.touch_start((30, 970))?;
            for _ in 0..35 {
                touch.goto_xy((30, 970))?;
                sleep(Duration::from_millis(100));
            }
            touch.touch_stop()?;
        }
        Some("erase") => Pen::new(false).erase_rectangle((240, 390), (510, 510))?,
        Some("next") | Some("previous") => {
            use remarkable_reader_buddy::workflow::xochitl_integration::{
                NavigationDirection, XochitlIntegration,
            };
            let mut touch = Touch::new(false, TriggerCorner::LowerLeft);
            let direction = if args[1] == "next" {
                NavigationDirection::Next
            } else {
                NavigationDirection::Previous
            };
            XochitlIntegration::navigate_to_page(&mut touch, direction)?;
        }
        Some("key-layout") => {
            use evdev::KeyCode as K;
            let mut keyboard = Keyboard::new(false, true);
            sleep(Duration::from_secs(1));
            for (label, key, shift, alt) in [
                (" A:", K::KEY_EQUAL, false, true),
                (" B:", K::KEY_MINUS, false, true),
                (" C:", K::KEY_0, true, false),
                (" D:", K::KEY_EQUAL, true, true),
                (" E:", K::KEY_MINUS, true, true),
                (" F:", K::KEY_KPPLUS, true, false),
            ] {
                keyboard.string_to_keypresses(label)?;
                if alt {
                    keyboard.key_down(K::KEY_LEFTALT)?;
                }
                if shift {
                    keyboard.key_down(K::KEY_LEFTSHIFT)?;
                }
                sleep(Duration::from_millis(30));
                keyboard.key_down(key)?;
                sleep(Duration::from_millis(30));
                keyboard.key_up(key)?;
                if shift {
                    keyboard.key_up(K::KEY_LEFTSHIFT)?;
                }
                if alt {
                    keyboard.key_up(K::KEY_LEFTALT)?;
                }
            }
            sleep(Duration::from_secs(2));
        }
        Some("clear-text") => {
            use evdev::KeyCode as K;
            let mut keyboard = Keyboard::new(false, true);
            sleep(Duration::from_secs(1));
            keyboard.key_down(K::KEY_LEFTCTRL)?;
            keyboard.string_to_keypresses("a")?;
            keyboard.key_up(K::KEY_LEFTCTRL)?;
            keyboard.string_to_keypresses("\x08")?;
            sleep(Duration::from_secs(2));
        }
        Some("text") => {
            let mut keyboard = Keyboard::new(false, true);
            sleep(Duration::from_secs(1));
            keyboard.string_to_keypresses(
                args.get(2)
                    .map(String::as_str)
                    .unwrap_or("Reader Buddy offline keyboard test"),
            )?;
        }
        _ => bail!("Usage: hardware_probe tap X Y | line | text"),
    }
    sleep(Duration::from_secs(2));
    let mut screenshot = Screenshot::new()?;
    screenshot.take_screenshot()?;
    screenshot.save_image("/tmp/reader-buddy-probe.png")?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("hardware_probe requires Linux tablet input devices")
}

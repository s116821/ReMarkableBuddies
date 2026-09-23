//! Explicit, offline hardware actions for a disposable test document.
#[cfg(target_os = "linux")]
use anyhow::{bail, Result};
#[cfg(target_os = "linux")]
use remarkable_reader_buddy::{Keyboard, Pen, Screenshot, Touch, TriggerCorner};
#[cfg(target_os = "linux")]
use std::{thread::sleep, time::Duration};

/// Diagnostic only: select a bounded suffix without deleting it, or press one key.
#[cfg(target_os = "linux")]
fn history_keys(action: &str, count: usize) -> Result<()> {
    use evdev::{uinput::VirtualDevice, AttributeSet, EventType, InputEvent, KeyCode as K};
    anyhow::ensure!(
        count <= 8000,
        "diagnostic selection exceeds 8000 characters"
    );
    let mut keys = AttributeSet::<K>::new();
    // Advertise a full keyboard so udev/xochitl recognize editing input.
    for code in 1..=57 {
        keys.insert(K::new(code));
    }
    for key in [
        K::KEY_LEFTCTRL,
        K::KEY_LEFTSHIFT,
        K::KEY_END,
        K::KEY_LEFT,
        K::KEY_DOWN,
        K::KEY_UP,
        K::KEY_RIGHT,
        K::KEY_BACKSPACE,
        K::KEY_Z,
        K::KEY_Y,
    ] {
        keys.insert(key);
    }
    let mut device = VirtualDevice::builder()?
        .name("Reader Buddy history diagnostic")
        .with_keys(&keys)?
        .build()?;
    sleep(Duration::from_secs(1));
    let mut emit = |key: K, value: i32| -> Result<()> {
        device.emit(&[InputEvent::new(EventType::KEY.0, key.code(), value)])?;
        sleep(Duration::from_millis(50));
        Ok(())
    };
    let result = (|| -> Result<()> {
        match action {
            "select-paragraphs" => {
                anyhow::ensure!((1..=128).contains(&count), "Paragraph count must be1..128");
                emit(K::KEY_LEFTCTRL, 1)?;
                emit(K::KEY_LEFTSHIFT, 1)?;
                for _ in 0..count {
                    emit(K::KEY_UP, 1)?;
                    emit(K::KEY_UP, 0)?;
                }
            }
            "end-down" | "down" | "right" => {
                if action == "end-down" {
                    emit(K::KEY_LEFTCTRL, 1)?;
                }
                let key = if action == "right" {
                    K::KEY_RIGHT
                } else {
                    K::KEY_DOWN
                };
                for _ in 0..count {
                    emit(key, 1)?;
                    emit(key, 0)?;
                }
            }
            "select-tail" | "select-left" | "end" => {
                if action != "select-left" {
                    emit(K::KEY_LEFTCTRL, 1)?;
                    emit(K::KEY_END, 1)?;
                    emit(K::KEY_END, 0)?;
                    emit(K::KEY_LEFTCTRL, 0)?;
                    sleep(Duration::from_millis(100));
                }
                if action == "end" {
                    return Ok(());
                }
                emit(K::KEY_LEFTSHIFT, 1)?;
                for _ in 0..count {
                    emit(K::KEY_LEFT, 1)?;
                    emit(K::KEY_LEFT, 0)?;
                }
            }
            "delete-selection" => {
                emit(K::KEY_BACKSPACE, 1)?;
                emit(K::KEY_BACKSPACE, 0)?;
            }
            "native-undo" | "native-redo" => {
                let key = if action == "native-undo" {
                    K::KEY_Z
                } else {
                    K::KEY_Y
                };
                emit(K::KEY_LEFTCTRL, 1)?;
                emit(key, 1)?;
                emit(key, 0)?;
            }
            _ => bail!("Unknown history diagnostic"),
        }
        Ok(())
    })();
    // A key-down can succeed even when the following write fails. Release all
    // keys this diagnostic can press, trying every release after an error.
    let mut cleanup = Ok(());
    for key in [
        K::KEY_END,
        K::KEY_LEFT,
        K::KEY_DOWN,
        K::KEY_UP,
        K::KEY_RIGHT,
        K::KEY_BACKSPACE,
        K::KEY_Z,
        K::KEY_Y,
        K::KEY_LEFTSHIFT,
        K::KEY_LEFTCTRL,
    ] {
        if let Err(error) = emit(key, 0) {
            cleanup = Err(error);
        }
    }
    result?;
    cleanup
}

#[cfg(target_os = "linux")]
fn main() -> Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("multi-hold") => {
            use evdev::{Device, EventType, InputEvent};
            anyhow::ensure!(
                matches!(
                    remarkable_reader_buddy::device::DeviceModel::detect(),
                    remarkable_reader_buddy::device::DeviceModel::Remarkable2
                ),
                "multi-hold diagnostic currently supports RM2 only"
            );
            let contacts: i32 = args
                .get(2)
                .ok_or_else(|| anyhow::anyhow!("contact count required"))?
                .parse()?;
            let millis: u64 = args
                .get(3)
                .ok_or_else(|| anyhow::anyhow!("duration required"))?
                .parse()?;
            let stagger: u64 = args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(0);
            anyhow::ensure!(
                matches!(contacts, 2 | 4) && millis <= 5000 && stagger <= 500,
                "expected 2/4 contacts, at most 5000ms hold and 500ms stagger"
            );
            let mut device = Device::open("/dev/input/event2")?;
            let abs = |code, value| InputEvent::new(EventType::ABSOLUTE.0, code, value);
            let syn = InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0);
            let mut start = Vec::new();
            for slot in 0..contacts {
                start.extend([
                    abs(47, slot),
                    abs(57, 100 + slot),
                    abs(53, (260 + 80 * slot) * 1404 / 768),
                    abs(54, (1024 - 600) * 1872 / 1024),
                    abs(58, 100),
                    abs(48, 17),
                    abs(49, 17),
                    abs(52, 4),
                ]);
            }
            let result = if stagger == 0 {
                start.push(syn);
                device.send_events(&start)
            } else {
                let mut result = Ok(());
                for chunk in start.as_chunks::<8>().0 {
                    let mut frame = chunk.to_vec();
                    frame.push(syn);
                    result = device.send_events(&frame);
                    if result.is_err() {
                        break;
                    }
                    sleep(Duration::from_millis(stagger));
                }
                result
            };
            if result.is_ok() {
                sleep(Duration::from_millis(millis));
            }
            let mut release = Vec::new();
            for slot in 0..contacts {
                release.extend([abs(47, slot), abs(57, -1)]);
            }
            let released = if stagger == 0 {
                release.push(syn);
                device.send_events(&release)
            } else {
                let mut result = Ok(());
                for chunk in release.as_chunks::<2>().0 {
                    let mut frame = chunk.to_vec();
                    frame.push(syn);
                    if let Err(error) = device.send_events(&frame) {
                        result = Err(error);
                    }
                    sleep(Duration::from_millis(stagger));
                }
                result
            };
            result?;
            released?;
        }
        Some(
            action @ ("select-tail" | "select-left" | "select-paragraphs" | "end" | "end-down"
            | "down" | "right" | "delete-selection" | "native-undo" | "native-redo"),
        ) => {
            let count = if matches!(
                action,
                "select-tail" | "select-left" | "select-paragraphs" | "end-down" | "down" | "right"
            ) {
                args.get(2)
                    .ok_or_else(|| anyhow::anyhow!("character count required"))?
                    .parse()?
            } else {
                0
            };
            history_keys(action, count)?;
        }
        Some("text-file") => {
            let path = args
                .get(2)
                .ok_or_else(|| anyhow::anyhow!("text file required"))?;
            let text = std::fs::read_to_string(path)?;
            anyhow::ensure!(text.len() <= 8000, "diagnostic text exceeds 8000 bytes");
            let mut keyboard = Keyboard::new(false, true);
            sleep(Duration::from_secs(1));
            keyboard.key_cmd_body()?;
            keyboard.string_to_keypresses(&text)?;
        }
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
            let duration = if args[1] == "press" {
                args.get(4)
                    .map(|s| s.parse::<u64>())
                    .transpose()?
                    .unwrap_or(2000)
            } else {
                100
            };
            anyhow::ensure!(
                (1..=5000).contains(&duration),
                "Press must be bounded to five seconds"
            );
            touch.touch_start((x, y))?;
            sleep(Duration::from_millis(duration));
            touch.touch_stop()?;
        }
        Some("strokes") | Some("erase-strokes") => {
            let path = args
                .get(2)
                .ok_or_else(|| anyhow::anyhow!("stroke JSON path required"))?;
            let strokes: Vec<Vec<(i32, i32)>> = serde_json::from_slice(&std::fs::read(path)?)?;
            let mut pen = Pen::new(false);
            anyhow::ensure!(
                strokes.len() <= 100 && strokes.iter().all(|s| s.len() <= 256),
                "Stroke diagnostic exceeds bounded path allowance"
            );
            for stroke in strokes {
                if args[1] == "erase-strokes" {
                    pen.erase_path_screen(&stroke)?;
                    continue;
                }
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
            anyhow::ensure!(
                strokes.len() <= 100 && strokes.iter().all(|s| s.len() <= 256),
                "Stroke diagnostic exceeds bounded path allowance"
            );
            for stroke in strokes {
                if args[1] == "erase-strokes" {
                    pen.erase_path_screen(&stroke)?;
                    continue;
                }
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
        Some("indicator-smoke")
        | Some("indicator-smoke-diagnostic")
        | Some("indicator-current-tool") => {
            let mut workflow =
                if args[1] == "indicator-current-tool" {
                    remarkable_reader_buddy::Workflow::with_device(Box::new(
                    remarkable_reader_buddy::device::backend::RealDevice::current_tool_probe(
                        TriggerCorner::LowerLeft, true)?), true)
                } else {
                    remarkable_reader_buddy::Workflow::new(
                        false,
                        TriggerCorner::LowerLeft,
                        args[1] == "indicator-smoke-diagnostic",
                    )?
                };
            std::fs::write(
                "/tmp/reader-buddy-status-before.png",
                workflow.capture_page_data()?,
            )?;
            let result = (|| -> Result<()> {
                use remarkable_reader_buddy::workflow::indicator::Stage;
                let mut active = Screenshot::new()?;
                for stage in [Stage::Preparing, Stage::AnswerPending, Stage::AnswerReady] {
                    workflow.set_indicator_stage(stage);
                    workflow.finish_indicator_stage()?;
                    active.take_screenshot()?;
                    active.save_image(&format!("/tmp/reader-buddy-status-{stage:?}.png"))?;
                }
                workflow.auxiliary_indicator()?;
                for _ in 0..6 {
                    workflow.tick_indicator()?;
                }
                active.take_screenshot()?;
                active.save_image("/tmp/reader-buddy-status-active.png")?;
                Ok(())
            })();
            let started = std::time::Instant::now();
            let cleanup = workflow.clear_indicator();
            println!("Owned-path cleanup: {} ms", started.elapsed().as_millis());
            result?;
            cleanup?;
        }
        Some("failure-code") => {
            use remarkable_reader_buddy::workflow::indicator::Failure;
            let code = match args.get(2).map(String::as_str) {
                Some("selection") => Failure::Selection,
                Some("transcription") => Failure::Transcription,
                Some("provider") => Failure::Provider,
                Some("no-successor") => Failure::NoSuccessor,
                Some("invalid-successor") => Failure::InvalidSuccessor,
                Some("device") => Failure::Device,
                _ => bail!("failure-code requires selection/transcription/provider/no-successor/invalid-successor/device"),
            };
            let mut workflow =
                remarkable_reader_buddy::Workflow::new(false, TriggerCorner::LowerLeft, false)?;
            workflow.capture_page_data()?;
            workflow.draw_failure(code)?;
        }
        Some("return-check") => {
            let mut workflow =
                remarkable_reader_buddy::Workflow::new(false, TriggerCorner::LowerLeft, false)?;
            let original = workflow.capture_page()?;
            workflow.navigate_to_next_page()?;
            sleep(Duration::from_millis(800));
            let outcome = workflow.return_to_original_page(&original)?;
            workflow.draw_failure(
                if outcome == remarkable_reader_buddy::workflow::ReturnOutcome::Unconfirmed {
                    remarkable_reader_buddy::workflow::indicator::Failure::Device
                } else {
                    remarkable_reader_buddy::workflow::indicator::Failure::InvalidSuccessor
                },
            )?;
            println!("Return outcome: {outcome:?}");
        }
        Some("round-trip") => {
            use remarkable_reader_buddy::workflow::xochitl_integration::{
                NavigationDirection, XochitlIntegration,
            };
            let millis: u64 = args
                .get(2)
                .ok_or_else(|| anyhow::anyhow!("gap milliseconds required"))?
                .parse()?;
            anyhow::ensure!(millis <= 2000, "round-trip gap exceeds 2000ms");
            let mut touch = Touch::new(false, TriggerCorner::LowerLeft);
            XochitlIntegration::navigate_to_page(&mut touch, NavigationDirection::Previous)?;
            sleep(Duration::from_millis(millis));
            XochitlIntegration::navigate_to_page(&mut touch, NavigationDirection::Next)?;
        }
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

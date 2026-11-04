//! Exact measured REM57 fixed stroke with independent raw evdev evidence.
//! Input delivery only; never a Qt commit, rendered ink, save or Writer receipt.
use anyhow::{ensure, Result};
use evdev::{Device, InputEvent};
use remarkable_reader_buddy::device::DeviceModel;
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::{
        fd::{AsFd, AsRawFd},
        unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    },
    path::Path,
    time::{Duration, Instant},
};

fn up() -> Vec<InputEvent> {
    vec![
        InputEvent::new(3, 24, 0),
        InputEvent::new(3, 25, 100),
        InputEvent::new(1, 330, 0),
        InputEvent::new(1, 320, 0),
        InputEvent::new(0, 0, 0),
    ]
}
fn drain(file: &mut File, raw: &mut Vec<u8>) -> Result<()> {
    loop {
        let mut bytes = [0u8; 16 * 128];
        match file.read(&mut bytes) {
            Ok(0) => anyhow::bail!("pen raw trace EOF"),
            Ok(count) => {
                ensure!(
                    count % 16 == 0 && raw.len() + count <= 16 * 1024,
                    "pen raw trace partial/quota"
                );
                raw.extend_from_slice(&bytes[..count]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
}
fn decode(raw: &[u8]) -> Vec<[i32; 3]> {
    raw.chunks_exact(16)
        .map(|record| {
            [
                i32::from(u16::from_le_bytes(record[8..10].try_into().unwrap())),
                i32::from(u16::from_le_bytes(record[10..12].try_into().unwrap())),
                i32::from_le_bytes(record[12..16].try_into().unwrap()),
            ]
        })
        .collect()
}
fn replay(records: &[[i32; 3]], initial: [i32; 4]) -> Result<()> {
    let (mut x, mut y, mut pressure, mut distance) =
        (initial[0], initial[1], initial[2], initial[3]);
    let (mut pen, mut touch, mut contact, mut partial) = (false, false, false, false);
    let (mut intervals, mut start, mut end) = (0usize, false, false);
    for &[kind, code, value] in records {
        match (kind, code, value) {
            (0, 0, 0) => {
                ensure!(
                    (!touch || pen) && touch == (pressure > 0),
                    "pen raw frame contact conflict"
                );
                if touch {
                    ensure!(
                        pressure == 2630
                            && distance == 0
                            && (11056..=11138).contains(&x)
                            && (7371..=7616).contains(&y),
                        "pen raw frame outside declared stroke"
                    );
                    if !contact {
                        intervals += 1;
                    }
                    ensure!(intervals == 1, "pen raw trace repeated contact");
                    start |= x == 11138 && y == 7371;
                    end |= x == 11056 && y == 7616;
                }
                contact = touch;
                partial = false;
            }
            (1, 320, 0..=1) => {
                pen = value != 0;
                partial = true;
            }
            (1, 330, 0..=1) => {
                touch = value != 0;
                partial = true;
            }
            (3, 0, 0..=20966) => {
                x = value;
                partial = true;
            }
            (3, 1, 0..=15725) => {
                y = value;
                partial = true;
            }
            (3, 24, 0..=4095) => {
                pressure = value;
                partial = true;
            }
            (3, 25, 0..=255) => {
                distance = value;
                partial = true;
            }
            _ => anyhow::bail!("pen raw trace unexpected event/drop"),
        }
    }
    ensure!(
        !partial && !pen && !touch && pressure == 0 && intervals == 1 && start && end,
        "pen raw trace lacks complete single released stroke"
    );
    Ok(())
}

pub fn line_once(
    original_deadline: Instant,
    check: &mut dyn FnMut() -> Result<()>,
    retain: &mut dyn FnMut(&str, Value) -> Result<()>,
) -> Result<()> {
    ensure!(
        DeviceModel::detect() == DeviceModel::Remarkable2
            && cfg!(target_endian = "little")
            && std::mem::size_of::<nix::libc::timeval>() == 8,
        "known raw stroke requires ARM32 RM2"
    );
    ensure!(
        Instant::now() < original_deadline,
        "pen preflight original bound expired"
    );
    let path = Path::new("/dev/input/event1");
    let named = fs::symlink_metadata(path)?;
    ensure!(
        named.file_type().is_char_device() && named.uid() == 0,
        "unexpected pen device"
    );
    let mut device = Device::open(path)?;
    let mut trace = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NONBLOCK | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
        .open(path)?;
    let descriptor = format!("/proc/self/fd/{}", device.as_fd().as_raw_fd());
    let owned = fs::metadata(&descriptor)?;
    let trace_meta = trace.metadata()?;
    let trace_descriptor = format!("/proc/self/fd/{}", trace.as_raw_fd());
    let expected = (owned.dev(), owned.ino(), owned.rdev());
    ensure!(
        expected == (named.dev(), named.ino(), named.rdev())
            && expected == (trace_meta.dev(), trace_meta.ino(), trace_meta.rdev()),
        "pen writer/raw reader identity mismatch"
    );
    let current = || -> Result<()> {
        for value in [
            fs::symlink_metadata(path)?,
            fs::metadata(&descriptor)?,
            fs::metadata(&trace_descriptor)?,
        ] {
            ensure!(
                (
                    value.dev(),
                    value.ino(),
                    value.rdev(),
                    value.uid(),
                    value.gid(),
                    value.mode(),
                    value.ctime(),
                    value.ctime_nsec()
                ) == (
                    owned.dev(),
                    owned.ino(),
                    owned.rdev(),
                    owned.uid(),
                    owned.gid(),
                    owned.mode(),
                    owned.ctime(),
                    owned.ctime_nsec()
                ),
                "pen descriptor/path identity changed"
            );
        }
        Ok(())
    };
    let axes = device.get_abs_state()?;
    ensure!(
        axes[0].minimum == 0
            && axes[0].maximum == 20966
            && axes[1].minimum == 0
            && axes[1].maximum == 15725
            && axes[24].maximum == 4095
            && axes[24].value == 0
            && axes[26].value == 0
            && axes[27].value == 0
            && device.get_key_state()?.iter().next().is_none(),
        "pen not in measured released/range state"
    );
    let initial = [axes[0].value, axes[1].value, axes[24].value, axes[25].value];
    let identity = json!({"device":owned.dev(),"inode":owned.ino(),"rdev":owned.rdev()});
    retain(
        "before",
        json!({"writer_identity":identity,"before_abs":initial,"start_abs":[11138,7371],"end_abs":[11056,7616],"both_start_axes_fresh":initial[0]!=11138&&initial[1]!=7371,"frames":56,"records":178,"timing":"initial-up10/hover10/down10/52moves1/final-up10","native_authority":false}),
    )?;
    ensure!(
        initial[0] != 11138 && initial[1] != 7371,
        "both measured stroke start axes must be fresh"
    );
    let mut raw = Vec::new();
    drain(&mut trace, &mut raw)?;
    ensure!(raw.is_empty(), "pen input arrived before stroke");
    let deadline = original_deadline.min(Instant::now() + Duration::from_secs(1));
    let began = Instant::now();
    let mut emitted = Vec::new();
    let mut final_up = false;
    let result = (|| -> Result<()> {
        for frame in 0..56 {
            check()?;
            current()?;
            ensure!(
                Instant::now() < deadline,
                "fixed stroke original bound expired"
            );
            let (events, delay) = match frame {
                0 | 55 => (up(), 10),
                1 => (
                    vec![
                        InputEvent::new(3, 0, 11138),
                        InputEvent::new(3, 1, 7371),
                        InputEvent::new(1, 320, 1),
                        InputEvent::new(1, 330, 0),
                        InputEvent::new(3, 24, 0),
                        InputEvent::new(3, 25, 100),
                        InputEvent::new(0, 0, 0),
                    ],
                    10,
                ),
                2 => (
                    vec![
                        InputEvent::new(1, 320, 1),
                        InputEvent::new(1, 330, 1),
                        InputEvent::new(3, 24, 2630),
                        InputEvent::new(3, 25, 0),
                        InputEvent::new(0, 0, 0),
                    ],
                    10,
                ),
                _ => {
                    let step = frame - 2;
                    (
                        vec![
                            InputEvent::new(3, 0, 11138 + (-82 * step) / 52),
                            InputEvent::new(3, 1, 7371 + (245 * step) / 52),
                            InputEvent::new(0, 0, 0),
                        ],
                        1,
                    )
                }
            };
            emitted.extend(events.iter().map(|event| {
                [
                    i32::from(event.event_type().0),
                    i32::from(event.code()),
                    event.value(),
                ]
            }));
            device.send_events(&events)?;
            if frame == 55 {
                final_up = true;
            }
            std::thread::sleep(Duration::from_millis(delay));
            drain(&mut trace, &mut raw)?;
        }
        Ok(())
    })();
    let recovery = if !final_up {
        device.send_events(&up()).map_err(anyhow::Error::new)
    } else {
        Ok(())
    };
    std::thread::sleep(Duration::from_millis(10));
    let drained = drain(&mut trace, &mut raw);
    let after = device.get_abs_state()?;
    let released = after[24].value == 0 && device.get_key_state()?.iter().next().is_none();
    let records = decode(&raw);
    let replayed = replay(&records, initial);
    let valid = result.is_ok()
        && recovery.is_ok()
        && drained.is_ok()
        && replayed.is_ok()
        && released
        && after[0].value == 11056
        && after[1].value == 7616;
    retain(
        "after",
        json!({"writer_identity":identity,"before_abs":initial,"after_abs":[after[0].value,after[1].value,after[24].value,after[25].value],"emitted":emitted,"raw":records,"raw_hex":raw.iter().map(|byte|format!("{byte:02x}")).collect::<String>(),"trace_valid":valid,"final_released":released,"stroke_elapsed_ms":began.elapsed().as_millis(),"emission":format!("{result:?}"),"conditional_release":format!("{recovery:?}"),"trace_drain":format!("{drained:?}"),"replay":format!("{replayed:?}"),"native_authority":false}),
    )?;
    result?;
    recovery?;
    drained?;
    replayed?;
    ensure!(valid, "fixed stroke raw delivery/release not verified");
    current()?;
    ensure!(
        Instant::now() < original_deadline,
        "pen returned after original packet deadline"
    );
    check()
}

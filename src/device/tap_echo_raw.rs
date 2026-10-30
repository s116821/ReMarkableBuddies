//! Development-only complete-event evidence, not a read-syscall transcript.
use anyhow::{ensure, Result};
use evdev::InputEvent;
use nix::libc;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    mem::{offset_of, size_of},
    path::Path,
};

const EVENTS: usize = 128;
const BATCHES: usize = 32;
#[cfg(all(target_arch = "arm", target_env = "gnu"))]
const _: () = {
    assert!(size_of::<libc::input_event>() == 16);
    assert!(size_of::<libc::time_t>() == 4 && size_of::<libc::suseconds_t>() == 4);
    assert!(offset_of!(libc::input_event, time) == 0);
    assert!(offset_of!(libc::timeval, tv_sec) == 0 && offset_of!(libc::timeval, tv_usec) == 4);
    assert!(offset_of!(libc::input_event, type_) == 8);
    assert!(
        offset_of!(libc::input_event, code) == 10 && offset_of!(libc::input_event, value) == 12
    );
};
pub(super) struct Evidence {
    pub active: bool,
    files: [File; 4],
    pub seed: Vec<u8>,
    bytes: Vec<u8>,
    batches: Vec<(u8, usize, usize)>,
    pub source: Option<(u64, u64)>,
    pub seed_point: Option<(i32, i32, i32)>,
    complete: bool,
    abi: [usize; 6],
}

fn abi() -> Result<[usize; 6]> {
    let widths = (size_of::<libc::time_t>(), size_of::<libc::suseconds_t>());
    let offsets = [
        offset_of!(libc::input_event, time),
        offset_of!(libc::timeval, tv_usec),
        offset_of!(libc::input_event, type_),
        offset_of!(libc::input_event, code),
        offset_of!(libc::input_event, value),
    ];
    let n = size_of::<libc::input_event>();
    ensure!(
        cfg!(target_endian = "little")
            && offset_of!(libc::timeval, tv_sec) == 0
            && ((widths == (4, 4) && offsets == [0, 4, 8, 10, 12] && n == 16)
                || (widths == (8, 8) && offsets == [0, 8, 16, 18, 20] && n == 24)),
        "Unsupported or padded input_event ABI"
    );
    Ok([n, widths.0, offsets[1], offsets[2], offsets[3], offsets[4]])
}

pub(super) fn different_axes(point: (i32, i32), seed: (i32, i32, i32)) -> Result<()> {
    ensure!(
        point.0 != seed.1 && point.1 != seed.2,
        "Both diagnostic axes must differ from fresh slot0"
    );
    Ok(())
}

pub(super) fn fresh_decoder(
    point: (i32, i32),
    fresh: super::contact_frames::ContactFrames,
    initial: (i32, i32, i32),
) -> Result<super::contact_frames::ContactFrames> {
    ensure!(
        fresh.ready_for_timer() && fresh.contacts().is_some_and(|c| c.is_empty()),
        "Diagnostic fresh seed is not released"
    );
    different_axes(point, initial)?;
    Ok(fresh)
}

impl Evidence {
    pub fn new(directory: &Path) -> Result<Self> {
        let abi = abi()?;
        ensure!(directory.is_dir(), "Evidence directory must already exist");
        let open = |name| -> Result<File> {
            Ok(OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(directory.join(name))?)
        };
        Ok(Self {
            active: false,
            files: [
                open("seed.bin")?,
                open("events.bin")?,
                open("receipt.bin")?,
                open("summary.json")?,
            ],
            seed: Vec::with_capacity(1024),
            bytes: Vec::with_capacity(3072),
            batches: Vec::with_capacity(BATCHES),
            source: None,
            seed_point: None,
            complete: true,
            abi,
        })
    }

    pub fn tee(
        &mut self,
        phase: u8,
        events: impl Iterator<Item = InputEvent>,
    ) -> Result<Vec<super::owned_pen_window::Event>> {
        self.collect(phase, events, true)
    }

    fn collect(
        &mut self,
        phase: u8,
        events: impl Iterator<Item = InputEvent>,
        semantic_output: bool,
    ) -> Result<Vec<super::owned_pen_window::Event>> {
        let mut events = events.peekable();
        if events.peek().is_none() {
            return Ok(Vec::new());
        }
        ensure!(self.active, "Raw evidence tee outside owned window");
        let start = self.bytes.len() / self.abi[0];
        if self.batches.len() == BATCHES {
            self.complete = false;
            anyhow::bail!("Raw evidence batch cap");
        }
        let available = EVENTS - start;
        self.batches.push((phase, start, 0));
        // This is the existing semantic output batch, not a second raw collection.
        let mut semantic = Vec::new();
        for (index, event) in events.take(EVENTS + 1).enumerate() {
            if index == available {
                self.complete = false;
                anyhow::bail!("Raw evidence event cap");
            }
            let raw: &libc::input_event = event.as_ref();
            self.bytes.extend_from_slice(&raw.time.tv_sec.to_le_bytes());
            self.bytes
                .extend_from_slice(&raw.time.tv_usec.to_le_bytes());
            self.bytes.extend_from_slice(&raw.type_.to_le_bytes());
            self.bytes.extend_from_slice(&raw.code.to_le_bytes());
            self.bytes.extend_from_slice(&raw.value.to_le_bytes());
            self.batches.last_mut().unwrap().2 += 1;
            if semantic_output {
                semantic.push((event.event_type().0, event.code(), event.value()));
            }
        }
        Ok(semantic)
    }

    pub fn capture(&mut self, phase: u8, events: &[InputEvent]) -> Result<()> {
        if !self.active {
            return Ok(());
        }
        self.collect(phase, events.iter().copied(), false)
            .map(|_| ())
    }

    pub fn persist(
        &mut self,
        virtual_point: (i32, i32),
        native: (i32, i32),
        outcome: &Result<()>,
    ) -> Result<()> {
        if outcome.is_err() {
            self.complete = false;
        }
        let receipt = serde_json::to_vec(&serde_json::json!({
            "format":"complete-input-event-v1", "little_endian":true,
            "target_arch":std::env::consts::ARCH, "target_os":std::env::consts::OS,
            "target_env":if cfg!(target_env="gnu") {"gnu"} else {"other"},
            "seed_format":"i32le ABS_MT_SLOT(value,min,max,fuzz,flat,resolution), then EVIOCGMTSLOTS selector+slots for57,53,54,57; failed ioctl buffer may be partial",
            "failure":outcome.as_ref().err().map(|e| e.to_string().chars().take(128).collect::<String>()),
            "abi_size_sec_usec_type_code_value":self.abi,
            "source_inode_rdev":self.source, "selected_slot_x0_y0":self.seed_point,
            "complete":self.complete, "echo_observed":outcome.is_ok(),
            "batches_phase_start_count":self.batches,
            "phases":{"1":"owned-finish", "2":"final-other-input-check"}
        }))?;
        ensure!(
            self.seed.len() <= 1024 && self.bytes.len() <= 3072,
            "Raw evidence byte cap"
        );
        // Evidence is durable before semantic decoding, including failed attempts.
        for (index, bytes) in [(0, self.seed.as_slice()), (1, self.bytes.as_slice())] {
            self.files[index].write_all(bytes)?;
            self.files[index].flush()?;
            self.files[index].sync_all()?;
        }
        ensure!(receipt.len() <= 2048, "Raw receipt byte cap");
        self.files[2].write_all(&receipt)?;
        self.files[2].flush()?;
        self.files[2].sync_all()?;
        let mut x = Vec::new();
        let mut y = Vec::new();
        let mut tracking = Vec::new();
        let mut syn = Vec::new();
        for (ordinal, bytes) in self.bytes.chunks_exact(self.abi[0]).enumerate() {
            let kind = u16::from_le_bytes(bytes[self.abi[3]..self.abi[3] + 2].try_into()?);
            let code = u16::from_le_bytes(bytes[self.abi[4]..self.abi[4] + 2].try_into()?);
            let value = i32::from_le_bytes(bytes[self.abi[5]..self.abi[5] + 4].try_into()?);
            match (kind, code) {
                (3, 53) => x.push((ordinal, value)),
                (3, 54) => y.push((ordinal, value)),
                (3, 57) => tracking.push((ordinal, value)),
                (0, 0) => syn.push(ordinal),
                _ => (),
            }
        }
        let summary = serde_json::to_vec(&serde_json::json!({
            "virtual_point":virtual_point,"native_point":native,
            "selected_slot_x0_y0":self.seed_point,"complete":self.complete,
            "echo_observed":outcome.is_ok(),"ui_acknowledged":false,
            "native_navigation_qualified":false,
            "x":{"present":!x.is_empty(),"count":x.len(),"ordinal_value":x},
            "y":{"present":!y.is_empty(),"count":y.len(),"ordinal_value":y},
            "tracking_ordinal_value":tracking,"syn_report_ordinals":syn
        }))?;
        ensure!(summary.len() <= 2048, "Raw summary byte cap");
        self.files[3].write_all(&summary)?;
        self.files[3].flush()?;
        self.files[3].sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn evidence() -> (std::path::PathBuf, Evidence) {
        let path = std::env::temp_dir().join(format!(
            "tap-raw-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        let evidence = Evidence::new(&path).unwrap();
        (path, evidence)
    }
    #[test]
    fn safe_abi_encoding_preserves_time_and_order_before_summary() {
        let (path, mut e) = evidence();
        e.active = true;
        let raw = libc::input_event {
            time: libc::timeval {
                tv_sec: -123,
                tv_usec: 987654,
            },
            type_: 3,
            code: 53,
            value: -201,
        };
        e.capture(1, &[InputEvent::from(raw)]).unwrap();
        e.capture(
            2,
            &[InputEvent::new(evdev::EventType::ABSOLUTE.0, 54, 1360)],
        )
        .unwrap();
        assert_eq!(
            &e.bytes[..size_of::<libc::time_t>()],
            &raw.time.tv_sec.to_le_bytes()
        );
        assert_eq!(
            &e.bytes[e.abi[2]..e.abi[3]],
            &raw.time.tv_usec.to_le_bytes()
        );
        assert_eq!(&e.bytes[e.abi[5]..e.abi[5] + 4], &(-201i32).to_le_bytes());
        e.persist((110, 280), (201, 1360), &Ok(())).unwrap();
        assert_eq!(std::fs::read(path.join("events.bin")).unwrap(), e.bytes);
        let summary: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path.join("summary.json")).unwrap()).unwrap();
        assert_eq!(
            summary["x"]["ordinal_value"][0],
            serde_json::json!([0, -201])
        );
        assert_eq!(
            summary["y"]["ordinal_value"][0],
            serde_json::json!([1, 1360])
        );
        assert!(Evidence::new(&path).is_err());
    }
    #[test]
    fn overflow_retains_prefix_and_failed_receipt() {
        let (path, mut e) = evidence();
        e.active = true;
        let event = InputEvent::new(evdev::EventType::SYNCHRONIZATION.0, 0, 0);
        assert!(e.capture(1, &[event; 129]).is_err());
        assert_eq!(e.bytes.len(), 128 * e.abi[0]);
        e.persist(
            (110, 280),
            (201, 1360),
            &Err(anyhow::anyhow!("read failed")),
        )
        .unwrap();
        assert_eq!(
            std::fs::metadata(path.join("events.bin")).unwrap().len(),
            e.bytes.len() as u64
        );
        let receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path.join("receipt.bin")).unwrap()).unwrap();
        assert_eq!(receipt["complete"], false);
    }
    #[test]
    fn batch_cap_and_preflight_are_not_axis_evidence() {
        let (_, mut e) = evidence();
        let event = InputEvent::new(evdev::EventType::ABSOLUTE.0, 53, 201);
        e.capture(2, &[event]).unwrap();
        assert!(e.bytes.is_empty());
        e.active = true;
        for _ in 0..32 {
            e.capture(1, &[event]).unwrap();
        }
        assert!(e.capture(1, &[event]).is_err());
        assert_eq!(e.bytes.len(), 32 * e.abi[0]);
        for point in [(201, 1360), (202, 1360), (201, 1361)] {
            assert!(different_axes(point, (0, 201, 1360)).is_err());
        }
        assert!(different_axes((202, 1361), (0, 201, 1360)).is_ok());
    }
    #[test]
    fn evidence_flush_failure_cannot_emit_semantic_summary() {
        let (path, mut e) = evidence();
        e.active = true;
        e.capture(1, &[InputEvent::new(evdev::EventType::ABSOLUTE.0, 53, 201)])
            .unwrap();
        e.files[1] = OpenOptions::new().write(true).open("/dev/full").unwrap();
        assert!(e.persist((110, 280), (201, 1360), &Ok(())).is_err());
        assert!(std::fs::read(path.join("summary.json")).unwrap().is_empty());
    }

    #[test]
    fn fresh_seed_cannot_inherit_the_old_axis_echo() {
        use super::super::contact_frames::{ContactFrames, Slot};
        let released = ContactFrames::seeded(
            vec![Slot {
                tracking: None,
                x: Some(164),
                y: Some(1396),
            }],
            0,
        )
        .unwrap();
        let mut decoder = fresh_decoder((201, 1360), released, (0, 164, 1396)).unwrap();
        decoder.feed(3, 57, 1);
        decoder.feed(0, 0, 0);
        let contacts = decoder.contacts().unwrap();
        assert_eq!((contacts[0].x, contacts[0].y), (164, 1396));
        let held = ContactFrames::seeded(
            vec![Slot {
                tracking: Some(1),
                x: Some(164),
                y: Some(1396),
            }],
            0,
        )
        .unwrap();
        assert!(fresh_decoder((201, 1360), held, (0, 164, 1396)).is_err());
    }
}

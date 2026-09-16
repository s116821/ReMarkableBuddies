//! Read-only input ownership observation. No input grab, injection or device
//! name based exclusion; caller supplies the exact owned uinput sysfs path.
use super::{
    contact_frames::{ContactFrames, Observation, Slot},
    interaction::{Contact, ContactReducer, Interaction},
    touch::TriggerCorner,
    DeviceModel,
};
use anyhow::{ensure, Context, Result};
use evdev::{raw_stream::RawDevice, AbsoluteAxisCode};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    time::Instant,
};

// Linux input.h EVIOCGMTSLOTS(len): first i32 is the requested MT axis,
// remaining i32 values receive every kernel slot's state.
nix::ioctl_read_buf!(mt_slots, b'E', 0x0a, i32);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    inode: u64,
    device: u64,
    sysfs: PathBuf,
}

fn inventory() -> Result<BTreeMap<PathBuf, Identity>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir("/dev/input")? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name
            .strip_prefix("event")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
        {
            continue;
        }
        let metadata = entry.metadata()?;
        result.insert(
            entry.path(),
            Identity {
                inode: metadata.ino(),
                device: metadata.rdev(),
                sysfs: fs::canonicalize(Path::new("/sys/class/input").join(name).join("device"))?,
            },
        );
    }
    ensure!(!result.is_empty(), "No native input devices");
    Ok(result)
}

fn seed(device: &RawDevice) -> Result<ContactFrames> {
    let axes = device.get_abs_state()?;
    let axis = axes[AbsoluteAxisCode::ABS_MT_SLOT.0 as usize];
    ensure!(
        axis.minimum == 0 && (0..super::contact_frames::MAX_SLOTS as i32).contains(&axis.maximum),
        "Unsupported touch slot range {}..{}",
        axis.minimum,
        axis.maximum
    );
    let count = axis.maximum as usize + 1;
    let read_axis = |code: i32| -> Result<Vec<i32>> {
        let mut values = vec![0i32; count + 1];
        values[0] = code;
        // SAFETY: owned live fd; ioctl writes at most the supplied initialized
        // i32 slice. nix constructs the architecture-specific request length.
        unsafe {
            mt_slots(device.as_raw_fd(), &mut values)?;
        }
        Ok(values[1..].to_vec())
    };
    let ids = read_axis(57)?;
    ensure!(
        ids.iter().all(|id| *id >= -1),
        "Invalid initial tracking ID"
    );
    let xs = read_axis(53)?;
    let ys = read_axis(54)?;
    ensure!(
        ids == read_axis(57)?,
        "Contacts changed during initial snapshot"
    );
    let slots = ids
        .iter()
        .zip(xs)
        .zip(ys)
        .map(|((&id, x), y)| Slot {
            tracking: (id >= 0).then_some(id),
            x: Some(x),
            y: Some(y),
        })
        .collect();
    ContactFrames::seeded(slots, usize::try_from(axis.value)?)
        .context("Incomplete initial touch state")
}

struct Source {
    device: RawDevice,
    touch: bool,
}

pub struct InputObserver {
    sources: Vec<Source>,
    inventory: BTreeMap<PathBuf, Identity>,
    frames: ContactFrames,
    reducer: ContactReducer,
    model: DeviceModel,
    clock: Instant,
    lost: bool,
    initial_input: bool,
}

impl InputObserver {
    pub fn new(corner: TriggerCorner, owned_sysfs: Option<&Path>) -> Result<Self> {
        let model = DeviceModel::detect();
        let touch_path = match model {
            DeviceModel::Remarkable2 => Path::new("/dev/input/event2"),
            DeviceModel::RemarkablePaperPro => Path::new("/dev/input/event3"),
            DeviceModel::Unknown => anyhow::bail!("Unknown input geometry"),
        };
        let inventory_before = inventory()?;
        let owned = owned_sysfs.map(fs::canonicalize).transpose()?;
        let mut sources = Vec::new();
        let mut frames = None;
        let mut initial_input = false;
        for (path, identity) in &inventory_before {
            if owned.as_ref() == Some(&identity.sysfs) {
                ensure!(path != touch_path, "Cannot exclude the touch device");
                continue;
            }
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(nix::libc::O_NONBLOCK)
                .open(path)?;
            let device = RawDevice::from_fd(file.into())?;
            let touch = path == touch_path;
            if touch {
                let state = seed(&device)?;
                initial_input |= !state.contacts().context("Unknown touch state")?.is_empty();
                frames = Some(state);
            } else {
                initial_input |= device.get_key_state()?.iter().next().is_some();
            }
            sources.push(Source { device, touch });
        }
        ensure!(
            inventory_before == inventory()?,
            "Input devices changed while opening observer"
        );
        Ok(Self {
            sources,
            inventory: inventory_before,
            frames: frames.context("Touch device missing")?,
            reducer: ContactReducer::new(corner),
            model,
            clock: Instant::now(),
            lost: false,
            initial_input,
        })
    }

    fn transform(model: DeviceModel, mut contacts: Vec<Contact>) -> Vec<Contact> {
        let (width, height) = match model {
            DeviceModel::RemarkablePaperPro => (2065, 2833),
            _ => (1404, 1872),
        };
        for c in &mut contacts {
            c.x = (i64::from(c.x) * 768 / width) as i32;
            c.y = (i64::from(c.y) * 1024 / height) as i32;
            if model == DeviceModel::Remarkable2 {
                c.y = 1024 - c.y;
            }
        }
        contacts
    }

    /// Call frequently, including while waiting for persistence. A failed poll
    /// permanently invalidates this observer and the caller's saved history.
    /// Process every Invalidated event before considering any action returned
    /// in the same batch: separate input devices have no shared event order.
    pub fn poll(&mut self) -> Result<Vec<Interaction>> {
        if self.lost {
            return Ok(vec![Interaction::Invalidated]);
        }
        let result = self.poll_inner();
        if result.is_err() {
            self.lost = true;
            self.reducer.cancel();
        }
        result
    }

    fn poll_inner(&mut self) -> Result<Vec<Interaction>> {
        ensure!(self.inventory == inventory()?, "Input device set changed");
        let mut output = Vec::new();
        if self.initial_input {
            self.initial_input = false;
            if let Some(contacts) = self.frames.contacts() {
                self.reducer
                    .frame(&Self::transform(self.model, contacts), self.clock.elapsed());
            }
            output.push(self.reducer.cancel());
        }
        for source in &mut self.sources {
            let events: Vec<_> = match source.device.fetch_events() {
                Ok(events) => events.collect(),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(e) => return Err(e.into()),
            };
            ensure!(events.len() <= 8192, "Input observer event limit");
            for event in events {
                let kind = event.event_type().0;
                if !source.touch {
                    if kind != 0 || event.code() == 3 {
                        output.push(self.reducer.cancel());
                    }
                    continue;
                }
                // Finger-count hints are checked against the complete slots.
                // Other buttons are unrelated actions and invalidate history.
                if kind == 1 && !matches!(event.code(), 330 | 325 | 333 | 334 | 335 | 328) {
                    output.push(self.reducer.cancel());
                }
                match self.frames.feed(kind, event.code(), event.value()) {
                    Observation::Pending => {}
                    Observation::Lost => anyhow::bail!("Lost native contact frame"),
                    Observation::Frame(contacts) => output.extend(
                        self.reducer
                            .frame(&Self::transform(self.model, contacts), self.clock.elapsed()),
                    ),
                }
            }
        }
        let contacts = self
            .frames
            .contacts()
            .context("Unknown complete contact frame")?;
        if self.frames.ready_for_timer() {
            output.extend(
                self.reducer
                    .frame(&Self::transform(self.model, contacts), self.clock.elapsed()),
            );
        }
        Ok(output)
    }
}

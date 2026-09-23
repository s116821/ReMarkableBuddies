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
        fd::{AsFd, AsRawFd, BorrowedFd},
        unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DescriptorIdentity {
    inode: u64,
    device: u64,
}

pub(super) fn descriptor_identity(fd: BorrowedFd<'_>) -> Result<DescriptorIdentity> {
    // A duplicate of the live descriptor gives File::metadata (fstat), not a
    // pathname lookup. Closing this duplicate does not close the original reader.
    let metadata = fs::File::from(fd.try_clone_to_owned()?).metadata()?;
    ensure!(
        metadata.file_type().is_char_device(),
        "Input descriptor is not a character device"
    );
    Ok(DescriptorIdentity {
        inode: metadata.ino(),
        device: metadata.rdev(),
    })
}

#[derive(Clone, Copy)]
struct OwnedPen {
    index: usize,
    writer: DescriptorIdentity,
    started: Duration,
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
    identity: Identity,
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
    owned_pen: Option<OwnedPen>,
    #[cfg(test)]
    scripted_polls: Option<std::collections::VecDeque<Result<Vec<Interaction>>>>,
}

impl InputObserver {
    /// Mutation guards cannot wait for a gesture to qualify or be released.
    /// Even an unfinished contact frame means the cursor is no longer owned.
    pub fn quiescent(&self) -> bool {
        self.owned_pen.is_none() && self.quiescent_state()
    }

    fn quiescent_state(&self) -> bool {
        !self.lost
            && !self.initial_input
            && self.frames.ready_for_timer()
            && self
                .frames
                .contacts()
                .is_some_and(|contacts| contacts.is_empty())
    }

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
            let opened = descriptor_identity(device.as_fd())?;
            ensure!(
                opened.inode == identity.inode && opened.device == identity.device,
                "Opened input descriptor changed"
            );
            sources.push(Source {
                device,
                touch,
                identity: identity.clone(),
            });
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
            owned_pen: None,
            #[cfg(test)]
            scripted_polls: None,
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
            anyhow::bail!("Native input observer permanently lost; recreate before waiting");
        }
        let result = if self.owned_pen.is_some() {
            Err(anyhow::anyhow!("Input observer is inside owned pen window"))
        } else {
            self.poll_inner()
        };
        if result.is_err() {
            self.lost = true;
            self.reducer.cancel();
        }
        result
    }

    pub(super) fn begin_owned_pen(&mut self, writer: DescriptorIdentity) -> Result<()> {
        let result = (|| {
            ensure!(
                self.poll()?.is_empty() && self.quiescent(),
                "Input before owned pen window"
            );
            let indices: Vec<_> = self
                .sources
                .iter()
                .enumerate()
                .filter_map(|(index, source)| {
                    (!source.touch
                        && source.identity.inode == writer.inode
                        && source.identity.device == writer.device)
                        .then_some(index)
                })
                .collect();
            ensure!(
                indices.len() == 1,
                "Pen writer does not identify one observed source"
            );
            let index = indices[0];
            ensure!(
                descriptor_identity(self.sources[index].device.as_fd())? == writer,
                "Pen reader/writer descriptor mismatch"
            );
            self.owned_pen = Some(OwnedPen {
                index,
                writer,
                started: self.clock.elapsed(),
            });
            Ok(())
        })();
        if result.is_err() {
            self.lost = true;
            self.reducer.cancel();
        }
        result
    }

    pub(super) fn has_owned_pen(&self) -> bool {
        self.owned_pen.is_some()
    }

    pub(super) fn finish_owned_pen(&mut self) -> Result<()> {
        let window = self.owned_pen.context("No owned pen window")?;
        let result = super::owned_pen_window::finish(
            &mut WindowAdapter {
                observer: self,
                window,
            },
            window.started,
        );
        self.owned_pen = None;
        if result.is_err() {
            self.lost = true;
            self.reducer.cancel();
        }
        result
    }

    fn poll_inner(&mut self) -> Result<Vec<Interaction>> {
        #[cfg(test)]
        if let Some(polls) = self.scripted_polls.as_mut() {
            return polls.pop_front().expect("unexpected diagnostic poll");
        }
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

    #[cfg(test)]
    pub(super) fn scripted(polls: Vec<Result<Vec<Interaction>>>) -> Self {
        Self {
            sources: Vec::new(),
            inventory: BTreeMap::new(),
            frames: ContactFrames::seeded(vec![Slot::default()], 0).unwrap(),
            reducer: ContactReducer::new(TriggerCorner::LowerLeft),
            model: DeviceModel::Remarkable2,
            clock: Instant::now(),
            lost: false,
            initial_input: false,
            owned_pen: None,
            scripted_polls: Some(polls.into()),
        }
    }
}

struct WindowAdapter<'a> {
    observer: &'a mut InputObserver,
    window: OwnedPen,
}
impl super::owned_pen_window::WindowIo for WindowAdapter<'_> {
    fn now(&self) -> Duration {
        self.observer.clock.elapsed()
    }
    fn validate_source(&mut self) -> Result<()> {
        ensure!(!self.observer.lost, "Owned input observer was lost");
        ensure!(
            self.observer.inventory == inventory()?,
            "Input inventory changed during owned pen window"
        );
        let source = self
            .observer
            .sources
            .get(self.window.index)
            .context("Owned pen source missing")?;
        ensure!(
            !source.touch && descriptor_identity(source.device.as_fd())? == self.window.writer,
            "Owned pen descriptor identity changed"
        );
        Ok(())
    }
    fn next_events(&mut self) -> Result<Option<Vec<super::owned_pen_window::Event>>> {
        let source = &mut self.observer.sources[self.window.index];
        match source.device.fetch_events() {
            Ok(events) => Ok(Some(
                events
                    .take(super::owned_pen_window::MAX_EVENTS + 1)
                    .map(|event| (event.event_type().0, event.code(), event.value()))
                    .collect(),
            )),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
    fn released_snapshot(&mut self) -> Result<()> {
        for source in &self.observer.sources {
            if source.touch {
                ensure!(
                    seed(&source.device)?
                        .contacts()
                        .context("Unknown current touch state")?
                        .is_empty(),
                    "Touch held across owned pen window"
                );
            } else {
                ensure!(
                    source.device.get_key_state()?.iter().next().is_none(),
                    "Input key held after owned pen release"
                );
            }
        }
        Ok(())
    }
    fn check_other_input(&mut self) -> Result<()> {
        ensure!(
            self.observer.poll_inner()?.is_empty() && self.observer.quiescent_state(),
            "External or delayed input during owned pen window"
        );
        Ok(())
    }
}

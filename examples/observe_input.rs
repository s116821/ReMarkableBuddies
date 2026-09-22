//! Bounded observation. Optional owned-modifier probe emits Shift down/up.
//! Logs decisions, never typed text or external key values.
#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    use remarkable_reader_buddy::device::{input_observer::InputObserver, touch::TriggerCorner};
    use std::time::{Duration, Instant};
    let seconds: u64 = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "30".into())
        .parse()?;
    anyhow::ensure!(
        (1..=60).contains(&seconds),
        "Observer duration must be 1..60 seconds"
    );
    let path = match remarkable_reader_buddy::device::DeviceModel::detect() {
        remarkable_reader_buddy::device::DeviceModel::Remarkable2 => "/dev/input/event2",
        _ => "/dev/input/event3",
    };
    let axes = evdev::raw_stream::RawDevice::open(path)?.get_abs_state()?;
    let slots = axes[evdev::AbsoluteAxisCode::ABS_MT_SLOT.0 as usize];
    println!(
        "kernel slots {}..{} selected {}",
        slots.minimum, slots.maximum, slots.value
    );
    let probe = std::env::args().nth(2);
    anyhow::ensure!(
        probe.as_deref().is_none_or(|s| s == "--owned-modifier"),
        "Unknown observer probe option"
    );
    let mut keyboard = probe.map(|_| remarkable_reader_buddy::Keyboard::new(false, false));
    if keyboard.is_some() {
        std::thread::sleep(Duration::from_secs(1));
    }
    let owned = match keyboard.as_mut() {
        Some(keyboard) => keyboard.owned_sysfs()?,
        None => None,
    };
    let mut observer = InputObserver::new(TriggerCorner::UpperRight, owned.as_deref())?;
    let start = Instant::now();
    println!(
        "observer ready; owned_modifier={}; {}s",
        keyboard.is_some(),
        seconds
    );
    if let Some(keyboard) = &mut keyboard {
        let down = keyboard.key_down(evdev::KeyCode::KEY_LEFTSHIFT);
        std::thread::sleep(Duration::from_millis(50));
        let up = keyboard.key_up(evdev::KeyCode::KEY_LEFTSHIFT);
        down?;
        up?;
        println!("owned modifier released");
    }
    while start.elapsed() < Duration::from_secs(seconds) {
        for event in observer.poll()? {
            println!("{}ms {:?}", start.elapsed().as_millis(), event);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("Native input observation requires Linux");
}

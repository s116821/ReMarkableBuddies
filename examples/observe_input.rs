//! Bounded, read-only diagnostics. Logs decisions, never key values or text.
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
    let mut observer = InputObserver::new(TriggerCorner::UpperRight, None)?;
    let start = Instant::now();
    println!("observer ready; read-only; {}s", seconds);
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

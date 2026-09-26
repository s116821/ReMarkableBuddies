//! Capture without initializing input devices or requiring provider credentials.
fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,remarkable_reader_buddy=debug"),
    )
    .format_timestamp_millis()
    .init();
    let mut args = std::env::args_os().skip(1);
    let path = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("Usage: screenshot FILE [--image-only]"))?;
    let image_only = match args.next() {
        None => false,
        Some(flag) if flag == "--image-only" => true,
        _ => anyhow::bail!("Usage: screenshot FILE [--image-only]"),
    };
    anyhow::ensure!(
        args.next().is_none(),
        "Usage: screenshot FILE [--image-only]"
    );
    let path = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Output path must be UTF-8"))?;
    let mut screenshot = remarkable_reader_buddy::Screenshot::new()?;
    if image_only {
        // Save outside capture.total, so this measures the same owned-pixel path
        // as status observation. Saving this diagnostic adds separate overhead.
        screenshot.take_image()?.save(path)?;
        return Ok(());
    }
    screenshot.take_screenshot()?;
    screenshot.save_image(path)
}

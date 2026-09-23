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
        .ok_or_else(|| anyhow::anyhow!("Usage: screenshot FILE"))?;
    anyhow::ensure!(args.next().is_none(), "Usage: screenshot FILE");
    let path = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Output path must be UTF-8"))?;
    let mut screenshot = remarkable_reader_buddy::Screenshot::new()?;
    screenshot.take_screenshot()?;
    screenshot.save_image(path)
}

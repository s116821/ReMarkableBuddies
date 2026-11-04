//! Read-only native text diagnostic; never writes to the supplied document.
fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("native .rm path required"))?;
    let value = remarkable_reader_buddy::device::native_text::read(&std::fs::read(path)?)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

//! Explicit diagnostic only. No installer or ordinary application dispatch.
fn main() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        remarkable_reader_buddy::workflow::resident_supervisor::service::bootstrap::entry(
            std::env::args().skip(1).collect(),
        )
    }
    #[cfg(not(target_os = "linux"))]
    anyhow::bail!("Resident bootstrap control requires Linux")
}

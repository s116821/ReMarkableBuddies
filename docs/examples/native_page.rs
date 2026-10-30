//! Read-only persisted identity diagnostic, without printing document text.
fn main() -> anyhow::Result<()> {
    use remarkable_reader_buddy::device::native_page;
    use std::path::Path;
    let session = native_page::xochitl_session(Path::new("/proc"))?;
    let root = Path::new("/home/root/.local/share/remarkable/xochitl");
    let page = if std::env::args().nth(1).as_deref() == Some("--latest-metadata") {
        native_page::candidate(root, session.clone())?
    } else {
        native_page::observed_candidate(
            root,
            Path::new("/home/root/.config/remarkable/xochitl.conf"),
            session.clone(),
        )?
    };
    anyhow::ensure!(
        session == native_page::xochitl_session(Path::new("/proc"))?,
        "Native session changed during observation"
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "document": page.owner.document,
            "page": page.owner.page,
            "visit": page.owner.visit,
            "session": page.owner.session,
            "paragraphs": page.content.paragraphs.len(),
            "characters": page.content.text().chars().count(),
            "seal_bytes": page.seal.len(),
            "editing_authorized": page.supported,
            "meaning": "Persisted last-opened candidate; not proof of the current UI"
        }))?
    );
    Ok(())
}

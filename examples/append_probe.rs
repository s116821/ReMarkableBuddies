//! Explicit bounded append to a selected disposable page, without model/status/undo.
#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    use anyhow::{ensure, Context, Result};
    use remarkable_reader_buddy::{
        device::{
            backend::{DeviceBackend, RealDevice},
            native_page,
        },
        workflow::history::{Owner, PageState},
        TriggerCorner,
    };
    use std::{fs, os::unix::fs::PermissionsExt, path::Path, time::Duration};
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,remarkable_reader_buddy=debug"),
    )
    .init();
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        args.len() == 4,
        "Usage: append_probe QA_FILE NEW_OUTPUT_DIR EXPECTED_DOCUMENT EXPECTED_PAGE"
    );
    let text = fs::read_to_string(&args[0])?;
    ensure!(
        text.is_ascii() && !text.is_empty() && text.len() <= 2000 && text.ends_with('\n'),
        "Bounded ASCII append ending in newline required"
    );
    let output = Path::new(&args[1]);
    fs::create_dir(output)?;
    fs::set_permissions(output, fs::Permissions::from_mode(0o700))?;
    fs::write(output.join("requested.txt"), &text)?;
    fn snapshot() -> Result<PageState> {
        native_page::observed_candidate(
            Path::new("/home/root/.local/share/remarkable/xochitl"),
            Path::new("/home/root/.config/remarkable/xochitl.conf"),
            native_page::xochitl_session(Path::new("/proc"))?,
        )
    }
    fn save(output: &Path, name: &str, page: &PageState) -> Result<()> {
        fs::write(
            output.join(format!("{name}.json")),
            serde_json::to_vec_pretty(&serde_json::json!({
                "owner": {"document":page.owner.document,"page":page.owner.page,"visit":page.owner.visit,"session":page.owner.session},
                "native_text":page.content,"text":page.content.text(),"supported":page.supported
            }))?,
        )?;
        // Exact native metadata/content/page bundle from this same observation.
        fs::write(output.join(format!("{name}.seal")), &page.seal)?;
        Ok(())
    }
    fn same(page: &PageState, owner: &Owner) -> Result<()> {
        ensure!(
            page.owner == *owner,
            "Diagnostic owner/page/visit/session changed"
        );
        Ok(())
    }
    let selected = snapshot()?;
    ensure!(
        selected.owner.document == args[2] && selected.owner.page == args[3],
        "Unexpected selected page"
    );
    let owner = selected.owner.clone();
    save(output, "selected", &selected)?;
    let mut device = RealDevice::new(false, TriggerCorner::LowerLeft, true)?;
    // Same existing input-registration exception as the normal process startup.
    device.delay(Duration::from_secs(1));
    let before_body = device
        .history_snapshot(None)?
        .context("History unsupported")?;
    save(output, "before-body", &before_body)?;
    same(&before_body, &owner)?;
    device.body_mode()?;
    let immediate_body = snapshot()?;
    save(output, "after-body-immediate", &immediate_body)?;
    same(&immediate_body, &owner)?;
    let before = device
        .history_snapshot(None)?
        .context("History unsupported")?;
    save(output, "before-render", &before)?;
    same(&before, &owner)?;
    let expected = format!("{}{text}", before.content.text());
    fs::write(output.join("expected.txt"), &expected)?;
    // Exactly one append, no extra separators, retry, status marks, or history edit.
    device.render_text(&text)?;
    let immediate = snapshot()?;
    save(output, "after-render-immediate", &immediate)?;
    same(&immediate, &owner)?;
    let result = device.history_snapshot(Some(&expected));
    let final_page = snapshot()?;
    save(output, "final", &final_page)?;
    fs::write(
        output.join("result.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "version":env!("READER_BUDDY_VERSION"),"error":result.as_ref().err().map(ToString::to_string),
            "same_owner":final_page.owner == owner,"exact_text":final_page.content.text() == expected,
            "meaning":"Explicit no-model append diagnostic; snapshots and logging add observation cost, not a latency acceptance run"
        }))?,
    )?;
    same(&final_page, &owner)?;
    let settled = result?.context("History unsupported")?;
    same(&settled, &owner)?;
    ensure!(settled.content.text() == expected, "Exact append missing");
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("Native append diagnostic requires Linux tablet hardware")
}

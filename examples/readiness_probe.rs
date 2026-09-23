//! Bounded no-model navigation/readiness diagnostic. Stop the normal service.
//! `next`/`previous` sends one real page swipe; no mode writes answer or status ink.
use anyhow::{ensure, Result};
use remarkable_reader_buddy::{
    device::backend::{DeviceBackend, NavigationCompletion, RealDevice},
    workflow::xochitl_integration::NavigationDirection,
    TriggerCorner, Workflow,
};
use std::{fs, path::Path, time::Duration};

fn owner() -> Result<remarkable_reader_buddy::device::status_style::Identity> {
    use remarkable_reader_buddy::device::{native_page, status_style::Identity};
    let session = native_page::xochitl_session(Path::new("/proc"))?;
    let owner = native_page::observed_owner(
        Path::new("/home/root/.local/share/remarkable/xochitl"),
        Path::new("/home/root/.config/remarkable/xochitl.conf"),
        session,
    )?;
    Ok(Identity {
        document: owner.document,
        page: owner.page,
        visit: owner.visit,
        session: owner.session,
    })
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,remarkable_reader_buddy=debug"),
    )
    .init();
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        (args.len() == 2 && args[0] == "current")
            || (args.len() == 3 && matches!(args[0].as_str(), "next" | "previous")),
        "Usage: readiness_probe current NEW_OUTPUT_DIR | next|previous NEW_OUTPUT_DIR EXPECTED_PAGE_UUID"
    );
    let output = Path::new(&args[1]);
    ensure!(!output.exists(), "Output directory must be new");
    fs::create_dir(output)?;
    let mut device = RealDevice::new(false, TriggerCorner::LowerLeft, true)?;
    let source_owner = owner()?;
    let source = device.capture()?;
    ensure!(
        owner()? == source_owner,
        "Owner changed during source capture"
    );
    fs::write(output.join("source.png"), &source.png)?;
    let mut completion = None;
    let mut expected_owner = source_owner.clone();
    if args[0] != "current" {
        let result = device.navigate(if args[0] == "next" {
            NavigationDirection::Next
        } else {
            NavigationDirection::Previous
        });
        let destination = owner();
        fs::write(
            output.join("navigation.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "source_owner": &source_owner, "expected_target": args[2],
                "completion": result.as_ref().ok().map(|value| format!("{value:?}")),
                "error": result.as_ref().err().map(ToString::to_string),
                "observed_owner": destination.as_ref().ok(),
                "owner_error": destination.as_ref().err().map(ToString::to_string),
                "meaning": "Metadata observation after navigation; inspect captured pixels independently"
            }))?,
        )?;
        let result = result?;
        ensure!(
            result == NavigationCompletion::Settled,
            "Diagnostic destination did not establish verified-layout readiness: {result:?}"
        );
        completion = Some(result);
        let destination = destination?;
        ensure!(
            destination.page == args[2]
                && destination.document == source_owner.document
                && destination.session == source_owner.session,
            "Diagnostic reached an unexpected target"
        );
        expected_owner = destination;
        device.delay(Duration::from_millis(800));
        let moved = device.capture()?;
        fs::write(output.join("after-navigation.png"), &moved.png)?;
        ensure!(
            !Workflow::is_same_page(
                &image::load_from_memory(&source.png)?,
                &image::load_from_memory(&moved.png)?
            ),
            "No page movement observed"
        );
    }
    device.delay(Duration::from_millis(500));
    let classifier_owner = owner()?;
    let classified = device.capture()?;
    ensure!(
        owner()? == classifier_owner,
        "Owner changed during classifier capture"
    );
    ensure!(
        classifier_owner == expected_owner,
        "Diagnostic owner changed after navigation"
    );
    fs::write(output.join("classifier.png"), &classified.png)?;
    let header = device.load_header();
    if let Some(header) = &header {
        header.save(output.join("reference.png"))?;
    }
    let classification =
        Workflow::classify_answer_page(&image::load_from_memory(&classified.png)?, header.as_ref());
    // Same production readiness function; no lease acquisition even on success.
    let result = device.ready_status_observation();
    let owner_matches = result
        .as_ref()
        .ok()
        .and_then(|value| value.as_ref())
        .map(|observed| observed.identity == expected_owner);
    let report = serde_json::json!({
        "version": env!("READER_BUDDY_VERSION"), "mode": args[0],
        "source_owner": source_owner, "classifier_owner": classifier_owner,
        "expected_target": args.get(2), "navigation_completion": completion.map(|value| format!("{value:?}")),
        "classification": format!("{classification:?}"),
        "readiness": match &result { Ok(Some(_)) if owner_matches == Some(true) => "ready", Ok(Some(_)) => "owner-mismatch", Ok(None) => "unavailable", Err(_) => "error" },
        "expected_owner": expected_owner, "readiness_owner_matches": owner_matches,
        "error": result.as_ref().err().map(ToString::to_string),
        "final_owner": result.as_ref().ok().and_then(|r| r.as_ref()).map(|r| &r.identity),
        "meaning": "Production readiness after diagnostic navigation/captures; no model, answer or status ink; not a complete Reader iteration"
    });
    fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    if let Ok(Some(observed)) = &result {
        observed.image.save(output.join("ready.png"))?;
        ensure!(
            observed.identity == expected_owner,
            "Readiness observation belongs to a different diagnostic owner"
        );
    }
    println!("{report}");
    result.map(|_| ())
}

//! Inject as an example into unchanged published 68e42968 source for offline qualification.
//! This is an uncompiled review candidate, not execution evidence.
use remarkable_reader_buddy::{
    conversation::Ledger,
    storage::{Store, StorePaths, Uuid},
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    io::{self, BufRead, Write},
    path::PathBuf,
    sync::Arc,
};

fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    anyhow::ensure!(
        args.len() == 4,
        "expected action, isolated root and conversation"
    );
    let root = PathBuf::from(&args[2]);
    let paths = StorePaths {
        data: root.join("data"),
        cache: root.join("cache"),
        credentials: root.join("credentials"),
    };
    let conversation: Uuid = args[3].parse()?;
    if args[1] == "refuse-reopen" {
        let generation: Uuid = serde_json::from_slice(&std::fs::read(paths.data.join("CURRENT"))?)?;
        let format: u32 = serde_json::from_slice(&std::fs::read(
            paths
                .data
                .join("generations")
                .join(generation.to_string())
                .join("format.json"),
        )?)?;
        anyhow::ensure!(format == 4, "target must actually be adopted format4");
        let error = match Store::open(paths) {
            Ok(_) => anyhow::bail!("predecessor admitted format4"),
            Err(error) => error,
        };
        anyhow::ensure!(
            error
                .chain()
                .any(|cause| cause.to_string() == "unsupported generation format"),
            "refusal must be the pinned predecessor format fence: {error:#}"
        );
        println!(
            "{}",
            json!({"action":"refuse-reopen","observed_format":format,"refused":true,"error":error.to_string(),"handler_not_constructed":true})
        );
        return Ok(());
    }
    anyhow::ensure!(
        args[1] == "positive-controls-and-hold",
        "unsupported driver action"
    );
    let store = Arc::new(Store::open(paths)?);
    let ledger = Ledger::new(store.clone());
    ledger.create(conversation, Uuid::new_v4(), 1)?;
    drop(ledger);
    let manifests = store.manifests()?;
    anyhow::ensure!(
        !manifests.is_empty(),
        "positive control requires committed ordinary history"
    );
    for manifest in &manifests {
        let mut objects = BTreeMap::new();
        for reference in manifest.records.iter().chain(&manifest.media) {
            objects.insert(reference.sha256.clone(), store.read_object(reference)?);
        }
        store.import(manifest.clone(), objects)?;
        // All verified objects already exist; import_staged must preserve idempotence.
        store.import_staged(manifest.clone(), &root.join("unused-incoming"))?;
    }
    let archive = root.join("ordinary-positive-control-backup");
    anyhow::ensure!(
        store.export(&archive)?,
        "positive archive must retain complete media"
    );
    Store::inspect_backup(&archive)?;
    store.restore(&archive)?;
    println!(
        "{}",
        json!({"action":"positive-controls-and-hold","ready":true,"conversation":conversation,"import":true,"import_staged":true,"ordinary_export":true,"ordinary_restore":true,"transport_route_included":false})
    );
    io::stdout().flush()?;
    let mut release = String::new();
    io::stdin().lock().read_line(&mut release)?;
    anyhow::ensure!(
        release.trim() == "release",
        "explicit coordinator release required"
    );
    drop(store);
    println!("{}", json!({"action":"released","closed":true}));
    Ok(())
}

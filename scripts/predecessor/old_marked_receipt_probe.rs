//! Thin hosted adapter for the unchanged pinned predecessor's public APIs.
use anyhow::{ensure, Context, Result};
use remarkable_reader_buddy::{
    conversation::{ExpectedHeads, IncompleteStore, Ledger, Outcome, Record},
    storage::{digest, Manifest, Namespace, ObjectRef, Store, StorePaths, Uuid},
};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};

#[derive(Deserialize)]
struct Prepared {
    conversation: Uuid,
    user: Uuid,
    capture: Uuid,
    root_revision: Uuid,
    user_revision: Uuid,
    overview_sha256: String,
    manifests: BTreeSet<Uuid>,
}

#[derive(Deserialize)]
struct Published {
    receipt: Uuid,
    receipt_revision: Uuid,
    transaction: Uuid,
    manifest_sha256: String,
    members: BTreeMap<Uuid, ObjectRef>,
}

fn incomplete<T>(result: Result<T>) -> Result<()> {
    let error = result
        .err()
        .context("predecessor admitted incomplete Ledger")?;
    ensure!(
        error
            .downcast_ref::<IncompleteStore>()
            .is_some_and(|e| e.unavailable_commits == 1),
        "expected typed one-transaction refusal: {error:#}"
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    ensure!(
        matches!(args.len(), 3 | 4),
        "expected phase, root and phase receipts"
    );
    let phase = args[0].to_str().context("invalid phase")?;
    ensure!(
        (phase == "prepared" && args.len() == 3) || (phase == "refuse-marked" && args.len() == 4),
        "unsupported predecessor phase"
    );
    let root = PathBuf::from(&args[1]);
    let paths = StorePaths {
        data: root.join("data"),
        cache: root.join("cache"),
        credentials: root.join("credentials"),
    };
    let prepared: Prepared = serde_json::from_slice(&std::fs::read(&args[2])?)?;
    let generation: Uuid = serde_json::from_slice(&std::fs::read(paths.data.join("CURRENT"))?)?;
    let generation_root = paths.data.join("generations").join(generation.to_string());
    let format: u32 = serde_json::from_slice(&std::fs::read(generation_root.join("format.json"))?)?;
    ensure!(format == 1, "expected ordinary format 1");
    let store = Arc::new(Store::open(paths)?);
    let ledger = Ledger::new(store.clone());
    let unavailable = store.unavailable_commits()?;
    let manifests: BTreeSet<_> = store
        .manifests()?
        .into_iter()
        .map(|m| m.transaction_id)
        .collect();
    ensure!(
        manifests == prepared.manifests,
        "predecessor visible manifests changed"
    );
    let root_heads = ledger.expected(Namespace::Conversation, prepared.conversation)?;
    let user_heads = ledger.expected(Namespace::Conversation, prepared.user)?;
    ensure!(
        root_heads == ExpectedHeads(BTreeSet::from([prepared.root_revision]))
            && user_heads == ExpectedHeads(BTreeSet::from([prepared.user_revision])),
        "predecessor exposed a different root or turn"
    );
    let user = store
        .value(Namespace::Conversation, prepared.user)?
        .context("Prepared turn absent")?;
    let Record::Turn(mut turn) = serde_json::from_value(user.payload)? else {
        anyhow::bail!("expected a Prepared turn");
    };
    ensure!(
        turn.id == prepared.user
            && turn.conversation == prepared.conversation
            && turn.outcome == Outcome::Prepared
            && turn.sources == [prepared.capture],
        "predecessor exposed different Prepared facts"
    );
    if phase == "prepared" {
        ensure!(unavailable == 0, "predecessor baseline is incomplete");
        let records = ledger.inspect(prepared.conversation, false)?;
        ensure!(
            records.contains(&Record::Turn(turn)),
            "Prepared Ledger inspection differs"
        );
        let capture = ledger.stored_sdk_images(prepared.capture)?;
        let overview = capture
            .images
            .first()
            .context("predecessor overview absent")?;
        ensure!(
            capture.turn == prepared.user
                && capture.capture == prepared.capture
                && digest(&overview.bytes) == prepared.overview_sha256,
            "predecessor could not read exact overview"
        );
    } else {
        ensure!(
            unavailable == 1,
            "expected exactly one unavailable marked transaction"
        );
        let published: Published = serde_json::from_slice(&std::fs::read(&args[3])?)?;
        let manifest: Manifest = {
            let bytes = std::fs::read(
                generation_root
                    .join("commits")
                    .join(format!("{}.json", published.transaction)),
            )?;
            ensure!(
                digest(&bytes) == published.manifest_sha256,
                "original manifest differs"
            );
            serde_json::from_slice(&bytes)?
        };
        let reference = published
            .members
            .get(&published.receipt_revision)
            .context("receipt reference absent")?;
        ensure!(
            manifest.transaction_id == published.transaction
                && manifest.records.contains(reference),
            "receipt is outside the rejected transaction"
        );
        ensure!(
            store
                .read_object(reference)
                .err()
                .is_some_and(|error| error.to_string()
                    == "object is not reachable from a committed manifest"),
            "predecessor made marked receipt bytes reachable"
        );
        ensure!(
            store
                .heads(Namespace::Conversation, published.receipt)?
                .is_empty(),
            "predecessor exposed the marked receipt head"
        );
        incomplete(ledger.inspect(prepared.conversation, false))?;
        incomplete(ledger.stored_sdk_images(prepared.capture))?;
        turn.outcome = Outcome::Interpreted;
        turn.text = Some("G? unc. here".into());
        turn.updated_ms = 2;
        incomplete(ledger.advance(Uuid::new_v4(), root_heads, user_heads, turn))?;
        ensure!(
            store
                .manifests()?
                .into_iter()
                .map(|m| m.transaction_id)
                .collect::<BTreeSet<_>>()
                == manifests,
            "refused Ledger advance modified visible manifests"
        );
    }
    drop(ledger);
    drop(store);
    println!(
        "{}",
        json!({"phase":phase,"generation":generation,"format":format,
        "conversation":prepared.conversation,"user":prepared.user,"capture":prepared.capture,
        "root_revision":prepared.root_revision,"user_revision":prepared.user_revision,
        "overview_sha256":prepared.overview_sha256,"manifests":manifests,
        "unavailable_commits":unavailable,"ledger_refused":phase == "refuse-marked","closed":true})
    );
    Ok(())
}

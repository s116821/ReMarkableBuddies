//! Synthetic-provider mechanics only. No live engine, CLI or evaluator truth input.
use super::*;

struct Receipt {
    cells: usize,
    confirmation: Uuid,
    source: [Uuid; 3],
}

fn receive(
    store: &Arc<Store>,
    local: &Local,
    bytes: &[u8],
    engine: &mut Calls,
) -> anyhow::Result<(Ledger, RequestScope, ImageRef)> {
    let (ledger, scope, image) =
        retained_source_fixture(store.clone(), Mode::Writer, bytes.to_vec());
    let mut dispatch = Dispatch::local(&local.config, &ledger)?;
    let report = dispatch.writer_source_image(
        &ledger,
        engine,
        &WriterImageRequest {
            image: image.clone(),
            request: "Write a note".into(),
            guidance: None,
            model_profile: "fixture-model".into(),
        },
        &scope,
        &mut || Ok(scope.source.clone()),
    );
    report
        .result
        .as_ref()
        .map_err(|_| anyhow::anyhow!("receiving refused"))?;
    report.write_to(&mut Vec::new())?;
    drop(report);
    drop(dispatch);
    Ok((ledger, scope, image))
}

fn compare(seed: &[u8], targets: &[Vec<u8>], engine: &mut Calls) -> anyhow::Result<Receipt> {
    anyhow::ensure!((1..=3).contains(&targets.len()), "target count");
    let mut hashes = BTreeSet::new();
    for bytes in std::iter::once(seed).chain(targets.iter().map(Vec::as_slice)) {
        image::load_from_memory_with_format(bytes, image::ImageFormat::Png)?;
        anyhow::ensure!(hashes.insert(digest(bytes)), "repeated source bytes");
    }
    let mut local = Local::ready_notebook(true);
    enable(&mut local);
    let store = Arc::new(Store::open(local.config.paths.clone())?);
    // Same Store, enabled empty profile. Every operation/source is freshly bound.
    for target in targets {
        receive(&store, &local, target, engine)?;
    }
    let (ledger, scope, image) = receive(&store, &local, seed, engine)?;
    let normalized = ledger
        .inspect(scope.conversation, false)?
        .into_iter()
        .find_map(|r| {
            if let Record::NormalizedText(value) = r {
                Some(value)
            } else {
                None
            }
        })
        .ok_or_else(|| anyhow::anyhow!("normalized seed absent"))?;
    let revision = *ledger
        .expected(Namespace::Source, normalized.id)?
        .0
        .iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("normalized seed revision absent"))?;
    let confirmation_scope = RequestScope {
        operation: Uuid::new_v4(),
        root: ledger.expected(Namespace::Conversation, scope.conversation)?,
        conversation: scope.conversation,
        source: scope.source.clone(),
    };
    let mut dispatch = Dispatch::local(&local.config, &ledger)?;
    let prepared = dispatch.prepare_handwriting_confirmation(
        &ledger,
        &ConfirmIntent {
            image: image.clone(),
            corrected_text: "CONTROLLED_SEED_READING".into(),
            normalized_source: Some((normalized.id, revision)),
        },
        &confirmation_scope,
        &mut || Ok(scope.source.clone()),
    )?;
    let confirmed = prepared.execute(&ledger, &mut || Ok(scope.source.clone()))?;
    anyhow::ensure!(
        confirmed.report.status == handwriting::Status::Complete,
        "confirmation refused"
    );
    let value = confirmed.report.facts[0].value.as_ref().unwrap();
    assert_eq!(value.original_text, "café x²");
    assert_eq!(
        value.provenance.source,
        Some([scope.conversation, image.turn, image.source])
    );
    let confirmation = confirmed.report.facts[0].record;
    let confirmed_revision = confirmed.report.facts[0].revision;
    confirmed.write_to(&mut Vec::new())?;
    drop(confirmed);
    drop(dispatch);
    for target in targets {
        receive(&store, &local, target, engine)?;
    }
    assert_eq!(
        store
            .heads(Namespace::Handwriting, confirmation)?
            .iter()
            .map(|e| e.revision_id)
            .collect::<Vec<_>>(),
        vec![confirmed_revision]
    );
    Ok(Receipt {
        cells: targets.len() * 2,
        confirmation,
        source: [scope.conversation, image.turn, image.source],
    })
}

fn sample(size: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::DynamicImage::new_rgba8(size, size)
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .unwrap();
    bytes
}
fn engine(operations: usize) -> Calls {
    Calls {
        parts: vec![],
        calls: vec![],
        replies: (0..operations)
            .flat_map(|_| [good_reply(), "# Title\n\nSynthetic note.".into()])
            .collect(),
    }
}

#[test]
fn six_cells_use_actual_receiving_and_one_explicit_seed_confirmation() {
    let mut engine = engine(7);
    let receipt = compare(&sample(2), &[sample(3), sample(4), sample(5)], &mut engine).unwrap();
    assert_eq!(receipt.cells, 6);
    assert_eq!(engine.calls.len(), 14);
    assert!(engine.replies.is_empty() && engine.parts.is_empty());
    for (n, call) in engine.calls.iter().enumerate() {
        let text = serde_json::to_string(call).unwrap();
        assert_eq!(
            text.contains("CONTROLLED_SEED_READING"),
            n >= 8 && n % 2 == 0
        );
        if n >= 8 && n % 2 == 0 {
            // Production payloads deliberately omit internal record identifiers.
            assert!(text.contains(&STANDARD.encode(sample(2))));
        }
        // Caller provenance is source/turn correspondence, never a UI assertion.
        assert!(!receipt.source.contains(&receipt.confirmation));
    }
}

#[test]
fn duplicate_sources_and_cell_bound_refuse_before_any_model_call() {
    for targets in [
        vec![],
        vec![sample(2)],
        vec![sample(3), sample(3)],
        vec![sample(3), sample(4), sample(5), sample(6)],
        vec![b"invalid".to_vec()],
    ] {
        let mut engine = engine(7);
        assert!(compare(&sample(2), &targets, &mut engine).is_err());
        assert!(engine.calls.is_empty());
    }
}

#[test]
fn first_receiving_failure_does_not_retry_or_reach_confirmation() {
    let mut engine = engine(7);
    engine.replies[0] = "invalid normalizer reply".into();
    assert!(compare(&sample(2), &[sample(3)], &mut engine).is_err());
    assert_eq!(engine.calls.len(), 1);
    assert!(engine.parts.is_empty());
    assert!(!serde_json::to_string(&engine.calls)
        .unwrap()
        .contains("CONTROLLED_SEED_READING"));
}

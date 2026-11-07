//! Bounded receiving comparison using actual Writer and explicit confirmation.
mod live;
use super::*;

struct Receipt {
    cells: usize,
    confirmation: Uuid,
    source: [Uuid; 3],
}

fn receive(
    store: &Arc<Store>,
    dispatch: &mut Dispatch<'_>,
    bytes: &[u8],
    engine: &mut impl LLMEngine,
    evidence: &mut impl FnMut(Value) -> anyhow::Result<()>,
) -> anyhow::Result<(Ledger, RequestScope, ImageRef)> {
    let (ledger, scope, image) =
        retained_source_fixture(store.clone(), Mode::Writer, bytes.to_vec());
    let report = dispatch.writer_source_image(
        &ledger,
        engine,
        &WriterImageRequest {
            image: image.clone(),
            request: "Write a note".into(),
            guidance: None,
            model_profile: live::MODEL.into(),
        },
        &scope,
        &mut || Ok(scope.source.clone()),
    );
    let mut report_bytes = Vec::new();
    report.write_to(&mut report_bytes)?;
    let succeeded = report.result.as_ref().is_ok_and(|(result, _)| {
        matches!(
            result.status,
            crate::writer::Status::Generated | crate::writer::Status::HistoricalGenerated
        ) && result.failure.is_none()
    });
    let diagnostic = report.diagnostic;
    drop(report);
    let normalized: Vec<_> = ledger
        .inspect(scope.conversation, false)?
        .into_iter()
        .filter_map(|record| match record {
            Record::NormalizedText(value) => Some(value),
            _ => None,
        })
        .collect();
    evidence(
        json!({"operation":scope.operation,"source":image.source,"image_sha256":digest(bytes),
        "report":serde_json::from_slice::<Value>(&report_bytes)?,"normalized":normalized,
        "diagnostic":diagnostic}),
    )?;
    anyhow::ensure!(succeeded, "receiving refused");
    Ok((ledger, scope, image))
}

fn compare(seed: &[u8], targets: &[Vec<u8>], engine: &mut Calls) -> anyhow::Result<Receipt> {
    let mut local = Local::ready_notebook(true);
    enable(&mut local);
    compare_gated(
        &local.config,
        seed,
        targets,
        engine,
        |_| Ok("CONTROLLED_SEED_READING".into()),
        |_| Ok(()),
    )
}

fn compare_gated(
    config: &Config,
    seed: &[u8],
    targets: &[Vec<u8>],
    engine: &mut impl LLMEngine,
    review: impl FnOnce(Value) -> anyhow::Result<String>,
    mut evidence: impl FnMut(Value) -> anyhow::Result<()>,
) -> anyhow::Result<Receipt> {
    anyhow::ensure!((1..=3).contains(&targets.len()), "target count");
    let mut hashes = BTreeSet::new();
    for bytes in std::iter::once(seed).chain(targets.iter().map(Vec::as_slice)) {
        image::load_from_memory_with_format(bytes, image::ImageFormat::Png)?;
        anyhow::ensure!(hashes.insert(digest(bytes)), "repeated source bytes");
    }
    let store = Arc::new(Store::open(config.paths.clone())?);
    let owner = Ledger::new(store.clone());
    let mut dispatch = Dispatch::local(config, &owner)?;
    for target in targets {
        receive(&store, &mut dispatch, target, engine, &mut evidence)?;
    }
    let (ledger, scope, image) = receive(&store, &mut dispatch, seed, engine, &mut evidence)?;
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
    let reviewed = review(json!({
        "seed_operation":scope.operation, "confirmation_operation":confirmation_scope.operation,
        "conversation":scope.conversation, "root_heads":confirmation_scope.root.0,
        "source":image.source,"source_revision":image.revision,"turn":image.turn,
        "turn_revision":image.turn_revision,"page":image.page,"image_sha256":digest(seed),
        "normalized":normalized,"normalized_revision":revision
    }))?;
    let prepared = dispatch.prepare_handwriting_confirmation(
        &ledger,
        &ConfirmIntent {
            image: image.clone(),
            corrected_text: reviewed,
            normalized_source: Some((normalized.id, revision)),
        },
        &confirmation_scope,
        &mut || Ok(scope.source.clone()),
    )?;
    let frozen: Value = serde_json::from_slice(prepared.frozen_input())?;
    evidence(json!({"confirmation_prepared":{
        "sha256":digest(prepared.frozen_input()),"operation":frozen["operation"],
        "confirmed_ms":frozen["confirmed_ms"],"expected_heads":frozen["expected_heads"]
    }}))?;
    let confirmed = prepared.execute(&ledger, &mut || Ok(scope.source.clone()))?;
    let mut receipt = Vec::new();
    confirmed.write_to(&mut receipt)?;
    evidence(
        json!({"confirmation":serde_json::from_slice::<Value>(&receipt)?,
        "committed":confirmed.report.status == handwriting::Status::Complete}),
    )?;
    anyhow::ensure!(
        confirmed.report.status == handwriting::Status::Complete,
        "confirmation refused"
    );
    let value = confirmed.report.facts[0].value.as_ref().unwrap();

    assert_eq!(
        value.provenance.source,
        Some([scope.conversation, image.turn, image.source])
    );
    let confirmation = confirmed.report.facts[0].record;
    let confirmed_revision = confirmed.report.facts[0].revision;
    drop(confirmed);
    for target in targets {
        receive(&store, &mut dispatch, target, engine, &mut evidence)?;
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

#[test]
fn review_refusal_stops_after_seed_and_never_starts_context() {
    let mut local = Local::ready_notebook(true);
    enable(&mut local);
    let mut engine = engine(7);
    let mut observed = false;
    let result = compare_gated(
        &local.config,
        &sample(2),
        &[sample(3), sample(4), sample(5)],
        &mut engine,
        |gate| {
            observed = true;
            assert!(gate["normalized"].is_object());
            assert!(gate["source_revision"].is_string());
            anyhow::bail!("review refused")
        },
        |_| Ok(()),
    );
    assert!(result.is_err() && observed);
    assert_eq!(engine.calls.len(), 8);
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

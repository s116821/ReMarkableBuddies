//! Explicit synthetic historical evidence for receiving controls, never a native lease.
use super::*;
use crate::conversation::{
    capture_facts::ExactU64,
    resident_capture::{ResidentCorrespondence, ResidentFacts, ResidentImage, ResidentOrigin},
    ResidentCaptureHistory,
};
use crate::storage::{Envelope, Kind};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn seed(
    ledger: &Ledger,
    conversation: Uuid,
    source: &SourceObservation,
    png: Vec<u8>,
) -> Result<(ResidentCaptureHistory, Vec<Vec<u8>>)> {
    use image::GenericImageView;
    let dimensions = image::load_from_memory(&png)?.dimensions();
    let [w, h] = [dimensions.0, dimensions.1];
    let images = crate::device::screenshot::Screenshot::reader_images_from_owned_png(&png)?;
    let th = h * 2 / 5;
    let crops = [
        [0, 0, w, h],
        [0, 0, w, th],
        [0, (h - th) / 2, w, th],
        [0, h - th, w, th],
    ];
    let media = |bytes: &[u8]| Media {
        sha256: digest(bytes),
        bytes: bytes.len() as u64,
        media_type: "image/png".into(),
    };
    let facts = ResidentFacts {
        document: source.document,
        page: source.page,
        order: vec![source.page],
    };
    let turn_id = Uuid::new_v4();
    let capture = ResidentCaptureHistory {
        schema: 1,
        sdk_source: crate::conversation::resident_capture::RESIDENT_SDK_SOURCE.into(),
        id: Uuid::new_v4(),
        conversation,
        turn: turn_id,
        origin: ResidentOrigin::ResidentHistoricalUnqualified,
        qualification: (),
        correspondence: ResidentCorrespondence {
            boot: Uuid::new_v4(),
            activation: Uuid::new_v4(),
            pid: 123,
            start_ticks: ExactU64::try_from("456".to_owned()).map_err(anyhow::Error::msg)?,
            profile_sha256: "11".repeat(32),
            executable_sha256: "22".repeat(32),
            adapter_sha256: "33".repeat(32),
            observe_operation: Uuid::new_v4(),
            capture_operation: Uuid::new_v4(),
            expected: facts.clone(),
        },
        before: facts.clone(),
        after: facts,
        dpr_bits: ExactU64::try_from(1.0f64.to_bits().to_string()).map_err(anyhow::Error::msg)?,
        procedure: remarkable_open_sdk::native_adapter::CAPTURE_PROCEDURE.into(),
        original_png: media(&png),
        original_dimensions: [w, h],
        provider_procedure: "reader-nearest-overview-overlapping-strips-v1".into(),
        images: images
            .iter()
            .enumerate()
            .map(|(index, bytes)| ResidentImage {
                media: media(bytes),
                dimensions: if index == 0 { [768, 1024] } else { [w, th] },
                provider_ordinal: index as u32,
                parent_crop: crops[index],
            })
            .collect(),
    };
    let old = ledger
        .store()
        .value(Namespace::Conversation, conversation)?
        .ok_or_else(|| anyhow::anyhow!("fixture root absent"))?;
    let Record::Root(mut root) = Ledger::decode(&old)? else {
        anyhow::bail!("fixture root kind")
    };
    let user = Turn {
        id: turn_id,
        conversation,
        exchange: Uuid::new_v4(),
        sequence: root.next_sequence,
        role: Role::User,
        mode: Mode::Reader,
        outcome: Outcome::Interpreted,
        text: Some("Earlier image question?".into()),
        sources: vec![capture.id],
        correction_of: None,
        created_ms: 1,
        updated_ms: 1,
        completion: None,
    };
    root.next_sequence += 1;
    let envelope =
        |namespace, id, schema, parents, record, media_descriptors| -> Result<Envelope> {
            Ok(Envelope {
                envelope_version: 1,
                namespace,
                domain_schema_version: schema,
                record_id: id,
                revision_id: Uuid::new_v4(),
                parents,
                operation_id: Uuid::new_v4(),
                actor_id: ledger.store().actor_id,
                kind: Kind::Value,
                payload: serde_json::to_value(record)?,
                media_descriptors,
            })
        };
    let mut objects = BTreeMap::from([(capture.original_png.sha256.clone(), png)]);
    let mut descriptors = BTreeMap::from([(
        capture.original_png.sha256.clone(),
        capture.original_png.clone(),
    )]);
    for (image, bytes) in capture.images.iter().zip(&images) {
        objects.insert(image.media.sha256.clone(), bytes.clone());
        descriptors.insert(image.media.sha256.clone(), image.media.clone());
    }
    // Fixture-only transaction: no acquired Session, binding, completion or effect permit.
    ledger.store().commit(
        vec![
            envelope(
                Namespace::Conversation,
                conversation,
                1,
                BTreeSet::from([old.revision_id]),
                Record::Root(root),
                vec![],
            )?,
            envelope(
                Namespace::Conversation,
                turn_id,
                1,
                BTreeSet::new(),
                Record::Turn(user),
                vec![],
            )?,
            envelope(
                Namespace::Source,
                capture.id,
                2,
                BTreeSet::new(),
                Record::ResidentCapture(Box::new(capture.clone())),
                descriptors.into_values().collect(),
            )?,
        ],
        objects,
    )?;
    Ok((capture, images))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn broken_recipe(ledger: &Ledger, capture: &mut ResidentCaptureHistory) -> Result<()> {
        let [width, height] = capture.images[1].dimensions;
        let mut png = Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(width, height, image::Rgba([17, 29, 41, 255]))
            .write_to(&mut png, image::ImageFormat::Png)?;
        let bytes = png.into_inner();
        let media = Media {
            sha256: digest(&bytes),
            bytes: bytes.len() as u64,
            media_type: "image/png".into(),
        };
        ledger.stage_image(&media, bytes.as_slice())?;
        capture.images[1].media = media;
        let mut envelope = ledger
            .store()
            .value(Namespace::Source, capture.id)?
            .unwrap();
        envelope.parents = BTreeSet::from([envelope.revision_id]);
        envelope.revision_id = Uuid::new_v4();
        envelope.operation_id = Uuid::new_v4();
        envelope.payload =
            serde_json::to_value(Record::ResidentCapture(Box::new(capture.clone())))?;
        let descriptors: BTreeMap<_, _> = std::iter::once(&capture.original_png)
            .chain(capture.images.iter().map(|image| &image.media))
            .map(|media| (media.sha256.clone(), media.clone()))
            .collect();
        envelope.media_descriptors = descriptors.into_values().collect();
        ledger.store().commit(vec![envelope], BTreeMap::new())?;
        Ok(())
    }

    fn document(document: Uuid) -> NormalizedDocument {
        NormalizedDocument {
            document,
            format: crate::retrieval::SourceFormat::Notebook,
            source_sha256: digest(b"declared comparison source"),
            extractor_revision: "fixture-v1".into(),
            units: vec![crate::retrieval::SourceUnit {
                page: Some(1),
                section: None,
                text: "Separate comparison source text.".into(),
            }],
        }
    }

    #[test]
    fn resident_false_derivative_refuses_actual_reader_writer_and_notebook_before_model(
    ) -> Result<()> {
        let fixture = SimulatorLedger::new();
        let prior = fixture.open()?;
        let paths = prior.store().paths.clone();
        drop(prior);
        let ledger = Ledger::new(std::sync::Arc::new(
            crate::storage::Store::prepare_notebook(paths)?,
        ));
        let conversation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 1)?;
        let source = fixture_source(Uuid::new_v4());
        let (mut capture, _) = seed(
            &ledger,
            conversation,
            &source,
            include_bytes!("../../../docs/validation/images/selected-memory/source-page.png")
                .to_vec(),
        )?;
        let frozen = ledger.context(
            conversation,
            &crate::conversation::ContextBudget {
                max_text_bytes: 64 * 1024,
                max_turns: 128,
                provider_token_limit: Some(64 * 1024),
            },
            |_| Ok(1),
        )?;
        broken_recipe(&ledger, &mut capture)?;
        // The resolver consumes the admitted evidence, not the new malformed head.
        assert_eq!(
            crate::conversation::receiving::images::resolve(&ledger, &frozen)?
                .attachments
                .len(),
            4
        );
        let config = Config::default();
        let mut dispatch = Dispatch::local(&config, &ledger)?;
        let mut model = Model::default();
        assert!(dispatch
            .reader(
                &ledger,
                &mut model,
                "Explain the diagram.",
                None,
                &scope(&ledger, conversation, &source)?,
                &mut || Ok(source.clone())
            )
            .is_err());
        assert!(model.seen.is_empty());
        let admitted = scope(&ledger, conversation, &source)?;
        let request = writer::Request::Generate {
            conversation,
            operation: admitted.operation,
            request: "Summarize this source.".into(),
            document: document(source.document),
            selection: crate::conversation::TextScope::SuppliedNotebook,
            guidance: None,
        };
        let (report, _) = dispatch.writer(&ledger, &mut model, &request, &admitted, &mut || {
            Ok(source.clone())
        })?;
        assert_ne!(report.status, writer::Status::Generated);
        assert!(model.seen.is_empty());
        let admitted = scope(&ledger, conversation, &source)?;
        let raw = serde_json::to_vec(&document(source.document))?;
        let hash = digest(&raw);
        let frozen = writer::notebook::ValidatedNotebook::from_bytes(&raw, &hash)?;
        let request = writer::notebook::command::Request::GenerateNotebook(Box::new(
            writer::notebook::command::Generate {
                conversation,
                operation: admitted.operation,
                request: "Compare these sources.".into(),
                document_file: fixture.0.join("unused-frozen-document.json"),
                document_sha256: hash,
                selection: crate::conversation::TextScope::SuppliedNotebook,
                guidance: None,
                max_model_calls: 2,
                history_range: None,
            },
        ));
        assert!(dispatch
            .notebook(
                &ledger,
                &mut model,
                &request,
                Some(frozen),
                &admitted,
                &mut || Ok(source.clone())
            )
            .is_err());
        assert!(model.seen.is_empty());
        Ok(())
    }

    #[test]
    fn resident_cumulative_proof_admission_precedes_missing_object_reads() -> Result<()> {
        use crate::conversation::{receiving::images, ContextBudget};
        let fixture = SimulatorLedger::new();
        let ledger = fixture.open()?;
        let conversation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 1)?;
        let source = fixture_source(Uuid::new_v4());
        let (capture, _) = seed(
            &ledger,
            conversation,
            &source,
            include_bytes!("../../../docs/validation/images/selected-memory/source-page.png")
                .to_vec(),
        )?;
        for byte_limit in [false, true] {
            let mut context = ledger.context(
                conversation,
                &ContextBudget {
                    max_text_bytes: 64 * 1024,
                    max_turns: 128,
                    provider_token_limit: Some(64 * 1024),
                },
                |_| Ok(1),
            )?;
            context.resident_captures.clear();
            context.turns[0].sources.clear();
            for _ in 0..if byte_limit { 12 } else { 13 } {
                let mut copy = capture.clone();
                copy.id = Uuid::new_v4();
                copy.original_png.sha256 = "ff".repeat(32); // No matching object exists.
                if byte_limit {
                    copy.original_png.bytes =
                        remarkable_open_sdk::native_adapter::MAX_CAPTURE_PNG as u64;
                    // Shared descriptors stay below the16MiB attachment bound;
                    // repeated proof work exceeds64MiB before any object read.
                    for image in &mut copy.images {
                        image.media.bytes = 2 * 1024 * 1024;
                    }
                }
                context.turns[0].sources.push(copy.id);
                context.resident_captures.push(copy);
            }
            let error = images::resolve(&ledger, &context)
                .err()
                .expect("unbounded proof admitted")
                .to_string();
            assert!(
                error.contains(if byte_limit {
                    "proof byte bound"
                } else {
                    "proof count"
                }),
                "unexpected admission failure: {error}"
            );
        }
        Ok(())
    }

    #[test]
    fn resident_derivatives_deduplicate_without_losing_capture_or_imageuse_associations(
    ) -> Result<()> {
        let fixture = SimulatorLedger::new();
        let ledger = fixture.open()?;
        let conversation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 1)?;
        let source = fixture_source(Uuid::new_v4());
        let png = include_bytes!("../../../docs/validation/images/selected-memory/source-page.png")
            .to_vec();
        let (first, images) = seed(&ledger, conversation, &source, png.clone())?;
        let (second, _) = seed(&ledger, conversation, &source, png)?;
        let (shared, _) = seed_image_bytes(&ledger, conversation, &source, images[0].clone(), 7)?;
        let config = Config::default();
        let mut dispatch = Dispatch::local(&config, &ledger)?;
        let mut model = Model {
            replies: ["Reply.".into()].into(),
            ..Default::default()
        };
        dispatch.reader(
            &ledger,
            &mut model,
            "Compare the original sources.",
            None,
            &scope(&ledger, conversation, &source)?,
            &mut || Ok(source.clone()),
        )?;
        assert_eq!(model.seen[0].1.len(), 4);
        let payload: Value = model.seen[0]
            .0
            .iter()
            .find_map(|part| {
                serde_json::from_str::<Value>(part)
                    .ok()
                    .filter(Value::is_object)
            })
            .unwrap();
        let associations = payload["historical_images"].as_array().unwrap();
        assert_eq!(associations.len(), 9);
        for (index, target) in [first.id, second.id].into_iter().enumerate() {
            for ordinal in 0..4 {
                let association = &associations[index * 4 + ordinal];
                assert_eq!(association["source"], target.to_string());
                assert_eq!(association["ordinal"], ordinal);
                assert_eq!(association["attachment_index"], ordinal);
                assert_eq!(association["sha256"], digest(&images[ordinal]));
            }
        }
        assert_eq!(associations[8]["source"], shared.to_string());
        assert_eq!(associations[8]["ordinal"], 7);
        assert_eq!(associations[8]["attachment_index"], 0);
        Ok(())
    }

    #[test]
    fn resident_missing_parent_and_corrupt_derivative_refuse_writer_refinement_without_call(
    ) -> Result<()> {
        for original in [true, false] {
            let fixture = SimulatorLedger::new();
            let ledger = fixture.open()?;
            let conversation = Uuid::new_v4();
            ledger.create(conversation, Uuid::new_v4(), 1)?;
            let source = fixture_source(Uuid::new_v4());
            let (capture, _) = seed(
                &ledger,
                conversation,
                &source,
                include_bytes!("../../../docs/validation/images/selected-memory/source-page.png")
                    .to_vec(),
            )?;
            let config = Config::default();
            let mut dispatch = Dispatch::local(&config, &ledger)?;
            let mut model = Model {
                replies: ["# Original draft\n\nExact source note.".into()].into(),
                ..Default::default()
            };
            let admitted = scope(&ledger, conversation, &source)?;
            let request = writer::Request::Generate {
                conversation,
                operation: admitted.operation,
                request: "Summarize this source.".into(),
                document: document(source.document),
                selection: crate::conversation::TextScope::SuppliedNotebook,
                guidance: None,
            };
            let (generated, _) =
                dispatch.writer(&ledger, &mut model, &request, &admitted, &mut || {
                    Ok(source.clone())
                })?;
            assert_eq!(generated.status, writer::Status::Generated);
            let generation: Uuid =
                serde_json::from_slice(&std::fs::read(ledger.store().paths.data.join("CURRENT"))?)?;
            let media = if original {
                &capture.original_png
            } else {
                &capture.images[1].media
            };
            let path = ledger
                .store()
                .paths
                .data
                .join("generations")
                .join(generation.to_string())
                .join("objects")
                .join(&media.sha256);
            if original {
                std::fs::remove_file(path)?;
            } else {
                std::fs::write(path, b"corrupt retained derivative")?;
            }
            model.seen.clear();
            let admitted = scope(&ledger, conversation, &source)?;
            let request = writer::Request::Refine {
                conversation,
                operation: admitted.operation,
                request: "Retain exact source evidence.".into(),
                draft: generated.draft.unwrap(),
            };
            let (report, _) =
                dispatch.writer(&ledger, &mut model, &request, &admitted, &mut || {
                    Ok(source.clone())
                })?;
            assert_ne!(report.status, writer::Status::Generated);
            assert!(model.seen.is_empty());
        }
        Ok(())
    }
    #[test]
    fn resident_history_reaches_the_actual_reader_receiver() -> Result<()> {
        let fixture = SimulatorLedger::new();
        let ledger = fixture.open()?;
        let conversation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 1)?;
        let source = fixture_source(Uuid::new_v4());
        let png = include_bytes!("../../../docs/validation/images/selected-memory/source-page.png")
            .to_vec();
        let (capture, expected) = seed(&ledger, conversation, &source, png)?;
        let config = Config::default();
        let mut dispatch = Dispatch::local(&config, &ledger)?;
        let mut model = Model {
            replies: ["Actual Reader reply.".into()].into(),
            ..Default::default()
        };
        dispatch.reader(
            &ledger,
            &mut model,
            "Explain the original diagram.",
            None,
            &scope(&ledger, conversation, &source)?,
            &mut || Ok(source.clone()),
        )?;
        let (parts, images) = &model.seen[0];
        assert_eq!(images.len(), 4);
        for (actual, expected) in images.iter().zip(&expected) {
            assert_eq!(STANDARD.decode(actual)?, *expected);
        }
        let payload: Value = parts
            .iter()
            .find_map(|part| {
                serde_json::from_str::<Value>(part)
                    .ok()
                    .filter(Value::is_object)
            })
            .unwrap();
        assert_eq!(payload["historical_images"].as_array().unwrap().len(), 4);
        assert_eq!(
            payload["historical_images"][0]["source"],
            capture.id.to_string()
        );
        assert!(!images
            .iter()
            .any(|image| digest(&STANDARD.decode(image).unwrap()) == capture.original_png.sha256));
        Ok(())
    }
}

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
    use crate::{
        conversation::{
            receiving::{ReceivingBudget, RequestSource, SourceAssociation},
            resolution::SourceDirective,
        },
        retrieval::{Index, IndexKey, Provider, Retrieved, SourceFormat, SourceUnit},
        workflow::conversation_dispatch::FollowupSource,
    };
    use std::{cell::Cell, sync::Arc};

    struct FollowupIndex<'a> {
        original: Index,
        calls: Cell<usize>,
        after_query: Option<&'a dyn Fn() -> Result<()>>,
    }

    impl Provider for FollowupIndex<'_> {
        fn query(
            &self,
            key: &IndexKey,
            text: &str,
            count: usize,
            bytes: usize,
            neighbors: bool,
        ) -> Result<Retrieved> {
            self.calls.set(self.calls.get() + 1);
            let result = self.original.query(key, text, count, bytes, neighbors)?;
            if let Some(after_query) = self.after_query {
                after_query()?;
            }
            Ok(result)
        }
    }

    #[derive(Default)]
    struct FollowupModel {
        model: Model,
        submits: usize,
        before_submission: Option<Box<dyn FnOnce() -> Result<()>>>,
    }

    impl LLMEngine for FollowupModel {
        fn clear_content(&mut self) {
            self.model.clear_content();
        }
        fn add_text_content(&mut self, text: &str) {
            self.model.add_text_content(text);
        }
        fn add_image_content(&mut self, image: &str) {
            self.model.add_image_content(image);
        }
        fn execute(&mut self) -> Result<String> {
            anyhow::bail!("unguarded follow-up")
        }
        fn execute_with_handoff(
            &mut self,
            progress: &mut dyn FnMut() -> Result<()>,
            handoff: &mut crate::llm::ModelHandoff<'_>,
        ) -> Result<String> {
            progress()?;
            if let Some(change) = self.before_submission.take() {
                change()?;
            }
            handoff(&mut || {
                self.submits += 1;
                Ok(())
            })?;
            progress()?;
            self.model.execute()
        }
    }

    fn pdf_index(document: Uuid, original: &[u8], revision: &str) -> Result<Index> {
        Index::build(NormalizedDocument {
            document,
            format: SourceFormat::Pdf,
            source_sha256: digest(original),
            extractor_revision: revision.into(),
            units: [
                "Image introduction.",
                "Trajectory calibration procedure.",
                "Pressure compensation example.",
            ]
            .into_iter()
            .enumerate()
            .map(|(offset, text)| SourceUnit {
                page: Some(offset as u32 + 1),
                section: None,
                text: text.into(),
            })
            .collect(),
        })
    }

    fn conflict_followup_evidence(store: &crate::storage::Store, operation: Uuid) -> Result<()> {
        let mut conflicting = store
            .value(
                Namespace::Conversation,
                crate::conversation::evidence::RequestEvidence::identity(operation),
            )?
            .unwrap();
        conflicting.revision_id = Uuid::new_v4();
        conflicting.operation_id = Uuid::new_v4();
        let bytes = serde_json::to_vec(&conflicting)?;
        let sha256 = digest(&bytes);
        store.import(
            crate::storage::Manifest {
                format: crate::storage::FORMAT,
                transaction_id: Uuid::new_v4(),
                scope: crate::storage::Scope::LocalTransaction,
                records: vec![crate::storage::ObjectRef {
                    sha256: sha256.clone(),
                    bytes: bytes.len() as u64,
                }],
                record_namespaces: BTreeMap::from([(sha256.clone(), Namespace::Conversation)]),
                media: vec![],
                media_coverage: vec![],
            },
            BTreeMap::from([(sha256, bytes)]),
        )?;
        Ok(())
    }

    #[test]
    fn ordinary_reader_followups_keep_exact_source_across_new_questions() -> Result<()> {
        let fixture = SimulatorLedger::new();
        let ledger = fixture.open()?;
        let conversation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 1)?;
        let original = fixture_source(Uuid::new_v4());
        let buddy = fixture_source(Uuid::new_v4());
        let (capture, images) = seed(
            &ledger,
            conversation,
            &original,
            include_bytes!("../../../docs/validation/images/selected-memory/source-page.png")
                .to_vec(),
        )?;
        let old = pdf_index(original.document, b"original modeled PDF", "pdf-fixture-v1")?;
        let old_operation = Uuid::new_v4();
        let choice = RequestSource::Original {
            origin_turn: capture.turn,
            source_record: capture.id,
            expected: old.key().clone(),
        };
        let evidence = ledger.prepare_request_evidence(
            conversation,
            capture.turn,
            None,
            choice,
            ReceivingBudget {
                context: crate::conversation::ContextBudget {
                    max_text_bytes: 16 * 1024,
                    max_turns: 64,
                    provider_token_limit: Some(96 * 1024),
                },
                max_encoded_bytes: 96 * 1024,
                max_passages: 8,
                max_passage_bytes: 16 * 1024,
                neighbors: false,
            },
            Some(&old),
            |text| Ok(text.len()),
        )?;
        assert_eq!(
            evidence.context().retrieval().unwrap().passages[0].page,
            Some(1)
        );
        ledger.persist_request_evidence(old_operation, 1, &evidence)?;

        let mut config = Config::default();
        config.memory.enabled = true;
        let profile = Uuid::new_v4();
        config.memory.profile = Some(profile);
        let mut dispatch = Dispatch::local(&config, &ledger)?;
        let memory = dispatch.local_memory().unwrap();
        for (scope, value) in [
            (crate::memory::Scope::Global, "GLOBAL_READER_FOLLOWUP"),
            (
                crate::memory::Scope::Document {
                    document: original.document,
                },
                "ORIGINAL_READER_FOLLOWUP",
            ),
            (
                crate::memory::Scope::Document {
                    document: buddy.document,
                },
                "VISIBLE_PAGE_MEMORY",
            ),
        ] {
            use crate::memory::Provider as _;
            memory.put(crate::memory::Mutation {
                operation: Uuid::new_v4(),
                expected: Default::default(),
                fact: crate::memory::Fact {
                    key: crate::memory::Key {
                        profile,
                        scope,
                        fact: Uuid::new_v4(),
                    },
                    claim: value.to_ascii_lowercase(),
                    value: value.into(),
                    subject: "pressure compensation trajectory calibration summary".into(),
                    tags: vec![],
                    provenance: crate::memory::Provenance {
                        basis: crate::memory::Basis::UserConfirmed,
                        description: "Modeled Reader follow-up".into(),
                        conversation: None,
                        turn: None,
                        source: None,
                    },
                    confidence_basis_points: 9000,
                    observed_ms: 1,
                    expires_ms: None,
                },
            })?;
        }
        let mut model = FollowupModel::default();
        model.model.replies.extend([
            "First answer".into(),
            "Second answer".into(),
            "Source-less answer".into(),
            "Inherited source-less answer".into(),
            "Retargeted answer".into(),
            "No source matches".into(),
        ]);
        let same = FollowupIndex {
            original: pdf_index(original.document, b"original modeled PDF", "pdf-fixture-v2")?,
            calls: Cell::new(0),
            after_query: None,
        };
        let mut anchor = (capture.turn, old_operation);
        for (question, page) in [
            ("Trajectory calibration?", 2),
            ("Pressure compensation?", 3),
        ] {
            let admitted = scope(&ledger, conversation, &buddy)?;
            let key = same.original.key().clone();
            let range = if page == 3 {
                let root = ledger
                    .store()
                    .value(Namespace::Conversation, conversation)?
                    .unwrap();
                let Record::Root(root) = Ledger::decode(&root)? else {
                    anyhow::bail!("fixture root kind");
                };
                Some(crate::conversation::TurnRange {
                    start: 1,
                    end_exclusive: root.next_sequence + 1,
                })
            } else {
                None
            };
            let expected_history = ledger
                .context_range(
                    conversation,
                    range.map(|range| crate::conversation::TurnRange {
                        start: range.start,
                        end_exclusive: range.end_exclusive - 1,
                    }),
                    &crate::conversation::ContextBudget {
                        max_text_bytes: 16 * 1024,
                        max_turns: 64,
                        provider_token_limit: Some(96 * 1024),
                    },
                    |_| Ok(1),
                )?
                .turns;
            let output = dispatch.reader_with_source(
                &ledger,
                &mut model,
                question,
                range,
                &admitted,
                SourceDirective::Inherit {
                    request_turn: anchor.0,
                    operation: Some(anchor.1),
                    expected: Some(key.clone()),
                },
                Some(FollowupSource {
                    key,
                    provider: &same,
                    check: Arc::new(|| Ok(())),
                }),
                &mut || Ok(buddy.clone()),
            )?;
            let (parts, attached) = model.model.seen.last().unwrap();
            let payload: Value =
                serde_json::from_str(parts.iter().find(|part| part.starts_with('{')).unwrap())?;
            assert_eq!(payload["request"], question);
            assert_eq!(payload["history"], json!(expected_history));
            assert_eq!(payload["selection"], json!(range));
            assert_eq!(payload["source"]["origin_turn"], json!(capture.turn));
            assert_eq!(payload["source"]["source_record"], json!(capture.id));
            if range.is_some() {
                assert!(!expected_history.iter().any(|turn| turn.id == capture.turn));
                assert_eq!(payload["history"][0]["text"], "Trajectory calibration?");
            }
            assert_eq!(
                payload["source"]["expected"]["document"],
                json!(original.document)
            );
            assert!(payload["retrieval"]["passages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|passage| passage["page"] == page && passage["direct_match"] == true));
            assert!(parts
                .iter()
                .any(|part| part.contains("ORIGINAL_READER_FOLLOWUP")));
            assert!(parts
                .iter()
                .all(|part| !part.contains("VISIBLE_PAGE_MEMORY")));
            assert_eq!(
                *attached,
                images
                    .iter()
                    .map(|bytes| STANDARD.encode(bytes))
                    .collect::<Vec<_>>()
            );
            anchor = (
                writer::stage(admitted.operation, "reader-user"),
                writer::stage(admitted.operation, "reader-source-evidence"),
            );
            let saved = ledger
                .request_evidence(conversation, anchor.0, anchor.1)?
                .unwrap();
            let saved_frame: Value = serde_json::from_str(&saved.record.frame_json)?;
            assert_eq!(saved_frame["request"]["text"], question);
            assert_eq!(saved_frame["history"], payload["history"]);
            assert_eq!(saved_frame["selection"], json!(range));
            assert_eq!(saved_frame["source"]["origin_turn"], json!(capture.turn));
            assert!(
                ledger
                    .expected(Namespace::Conversation, conversation)?
                    .0
                    .len()
                    == 1
            );
            let completion_scope = RequestScope {
                conversation,
                operation: admitted.operation,
                root: output.root().clone(),
                source: buddy.clone(),
            };
            let effect = Cell::new(0);
            output.admit_followup_output(
                &ledger,
                &completion_scope,
                &mut || Ok(buddy.clone()),
                || {
                    effect.set(effect.get() + 1);
                    Ok(())
                },
            )?;
            assert!(output
                .admit_followup_output(
                    &ledger,
                    &completion_scope,
                    &mut || Ok(buddy.clone()),
                    || {
                        effect.set(effect.get() + 1);
                        Ok(())
                    }
                )
                .is_err());
            assert_eq!(effect.get(), 1);
            dispatch.record_output(
                &ledger,
                &completion_scope,
                &output,
                crate::conversation::CompletionEvidence {
                    native_operation: admitted.operation,
                    exact_text: if page == 2 {
                        "First answer"
                    } else {
                        "Second answer"
                    }
                    .into(),
                    observation: buddy.clone(),
                    procedure: buddy.evidence_procedure.clone(),
                },
                &mut || Ok(buddy.clone()),
            )?;
        }
        assert_eq!(same.calls.get(), 2);
        assert_eq!(model.submits, 2);

        let changed = FollowupIndex {
            original: pdf_index(original.document, b"changed modeled PDF", "pdf-fixture-v2")?,
            calls: Cell::new(0),
            after_query: None,
        };
        let key = changed.original.key().clone();
        let admitted = scope(&ledger, conversation, &buddy)?;
        assert!(dispatch
            .reader_with_source(
                &ledger,
                &mut model,
                "Pressure again?",
                None,
                &admitted,
                SourceDirective::Inherit {
                    request_turn: anchor.0,
                    operation: Some(anchor.1),
                    expected: Some(key.clone())
                },
                Some(FollowupSource {
                    key,
                    provider: &changed,
                    check: Arc::new(|| Ok(()))
                }),
                &mut || Ok(buddy.clone())
            )
            .is_err());
        assert_eq!(changed.calls.get(), 0);
        assert_eq!(model.submits, 2);

        let mut source_less = SourceDirective::Explicit(RequestSource::NoDocument);
        for (attempt, question) in ["Give me a summary.", "Expand that summary."]
            .into_iter()
            .enumerate()
        {
            let admitted = scope(&ledger, conversation, &buddy)?;
            dispatch.reader_with_source(
                &ledger,
                &mut model,
                question,
                None,
                &admitted,
                source_less,
                None,
                &mut || Ok(buddy.clone()),
            )?;
            let (parts, _) = model.model.seen.last().unwrap();
            let payload: Value =
                serde_json::from_str(parts.iter().find(|part| part.starts_with('{')).unwrap())?;
            let no_document = serde_json::to_value(SourceAssociation::NoDocument)?;
            assert_eq!(payload["request"], question);
            assert_eq!(payload["source"], no_document);
            assert!(payload["retrieval"].is_null());
            assert!(parts
                .iter()
                .any(|part| part.contains("GLOBAL_READER_FOLLOWUP")));
            assert!(parts
                .iter()
                .all(|part| !part.contains("ORIGINAL_READER_FOLLOWUP")
                    && !part.contains("VISIBLE_PAGE_MEMORY")));
            assert_eq!(same.calls.get(), 2);
            assert_eq!(changed.calls.get(), 0);
            assert_eq!(model.submits, 3 + attempt);
            let user = writer::stage(admitted.operation, "reader-user");
            let evidence = writer::stage(admitted.operation, "reader-source-evidence");
            let saved = ledger
                .request_evidence(conversation, user, evidence)?
                .unwrap();
            let frame: Value = serde_json::from_str(&saved.record.frame_json)?;
            assert_eq!(frame["source"], no_document);
            assert!(frame["retrieval"].is_null());
            source_less = SourceDirective::Inherit {
                request_turn: user,
                operation: Some(evidence),
                expected: None,
            };
        }

        let replacement = fixture_source(Uuid::new_v4());
        let (other, _) = seed_image(&ledger, conversation, &replacement, 117)?;
        let other_turn = ledger
            .inspect(conversation, false)?
            .into_iter()
            .find_map(|record| match record {
                Record::Turn(turn) if turn.sources.contains(&other) => Some(turn.id),
                _ => None,
            })
            .unwrap();
        let index = pdf_index(
            replacement.document,
            b"second modeled PDF",
            "pdf-fixture-v1",
        )?;
        let key = index.key().clone();
        let admitted = scope(&ledger, conversation, &buddy)?;
        dispatch.reader_with_source(
            &ledger,
            &mut model,
            "Pressure compensation?",
            None,
            &admitted,
            SourceDirective::Explicit(RequestSource::Original {
                origin_turn: other_turn,
                source_record: other,
                expected: key.clone(),
            }),
            Some(FollowupSource {
                key,
                provider: &index,
                check: Arc::new(|| Ok(())),
            }),
            &mut || Ok(buddy.clone()),
        )?;
        let (parts, _) = model.model.seen.last().unwrap();
        let payload: Value =
            serde_json::from_str(parts.iter().find(|part| part.starts_with('{')).unwrap())?;
        assert_eq!(
            payload["source"]["expected"]["document"],
            json!(replacement.document)
        );
        assert!(payload["retrieval"]["passages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|passage| passage["page"] == 3 && passage["direct_match"] == true));
        assert_eq!(model.submits, 5);

        let healthy = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let check = healthy.clone();
        let admitted = scope(&ledger, conversation, &buddy)?;
        let key = same.original.key().clone();
        let query_count = same.calls.get();
        let output = dispatch.reader_with_source(
            &ledger,
            &mut model,
            "Zygomorphic?",
            None,
            &admitted,
            SourceDirective::Inherit {
                request_turn: anchor.0,
                operation: Some(anchor.1),
                expected: Some(key.clone()),
            },
            Some(FollowupSource {
                key: key.clone(),
                provider: &same,
                check: Arc::new(move || {
                    ensure!(
                        check.load(std::sync::atomic::Ordering::SeqCst),
                        "modeled original changed"
                    );
                    Ok(())
                }),
            }),
            &mut || Ok(buddy.clone()),
        )?;
        assert_eq!(same.calls.get(), query_count + 1);
        assert_eq!(model.submits, 6);
        let (parts, _) = model.model.seen.last().unwrap();
        let payload: Value =
            serde_json::from_str(parts.iter().find(|part| part.starts_with('{')).unwrap())?;
        assert_eq!(payload["source"]["kind"], "original");
        assert_eq!(payload["source"]["origin_turn"], json!(capture.turn));
        assert_eq!(payload["source"]["source_record"], json!(capture.id));
        assert_eq!(payload["retrieval"]["source"], json!(key));
        assert_eq!(payload["retrieval"]["passages"], json!([]));
        assert_eq!(payload["retrieval"]["budget_limited"], false);
        let draft = writer::stage(admitted.operation, "reader-draft");
        let original_output = ledger
            .store()
            .value(Namespace::Conversation, draft)?
            .unwrap();
        let Record::Turn(turn) = Ledger::decode(&original_output)? else {
            anyhow::bail!("fixture Reader draft kind");
        };
        assert_eq!(turn.outcome, Outcome::Generated);
        assert_eq!(turn.text.as_deref(), Some("No source matches"));
        let completion_scope = RequestScope {
            conversation,
            operation: admitted.operation,
            root: output.root().clone(),
            source: buddy.clone(),
        };
        healthy.store(false, std::sync::atomic::Ordering::SeqCst);
        let effect = Cell::new(0);
        assert!(output
            .admit_followup_output(
                &ledger,
                &completion_scope,
                &mut || Ok(buddy.clone()),
                || {
                    effect.set(effect.get() + 1);
                    Ok(())
                },
            )
            .is_err());
        assert_eq!(effect.get(), 0);
        assert!(dispatch
            .record_output(
                &ledger,
                &completion_scope,
                &output,
                crate::conversation::CompletionEvidence {
                    native_operation: admitted.operation,
                    exact_text: "No source matches".into(),
                    observation: buddy.clone(),
                    procedure: buddy.evidence_procedure.clone(),
                },
                &mut || Ok(buddy.clone()),
            )
            .is_err());
        let still_generated = ledger
            .store()
            .value(Namespace::Conversation, draft)?
            .unwrap();
        assert_eq!(still_generated.revision_id, original_output.revision_id);
        assert_eq!(Ledger::decode(&still_generated)?, Record::Turn(turn));

        let admitted = scope(&ledger, conversation, &buddy)?;
        assert!(dispatch
            .reader_with_source(
                &ledger,
                &mut model,
                "Pressure again?",
                None,
                &admitted,
                SourceDirective::Inherit {
                    request_turn: anchor.0,
                    operation: Some(Uuid::new_v4()),
                    expected: Some(same.original.key().clone())
                },
                Some(FollowupSource {
                    key: same.original.key().clone(),
                    provider: &same,
                    check: Arc::new(|| Ok(()))
                }),
                &mut || Ok(buddy.clone())
            )
            .is_err());
        assert_eq!(same.calls.get(), 3);

        let fault = || ledger.store().set_fault(crate::storage::Fault::AfterCommit);
        let uncertain = FollowupIndex {
            original: pdf_index(original.document, b"original modeled PDF", "pdf-fixture-v2")?,
            calls: Cell::new(0),
            after_query: Some(&fault),
        };
        let admitted = scope(&ledger, conversation, &buddy)?;
        let key = uncertain.original.key().clone();
        let directive = SourceDirective::Inherit {
            request_turn: anchor.0,
            operation: Some(anchor.1),
            expected: Some(key.clone()),
        };
        for _ in 0..2 {
            assert!(dispatch
                .reader_with_source(
                    &ledger,
                    &mut model,
                    "Pressure compensation?",
                    None,
                    &admitted,
                    directive.clone(),
                    Some(FollowupSource {
                        key: key.clone(),
                        provider: &uncertain,
                        check: Arc::new(|| Ok(()))
                    }),
                    &mut || Ok(buddy.clone())
                )
                .is_err());
        }
        let user = writer::stage(admitted.operation, "reader-user");
        let evidence = writer::stage(admitted.operation, "reader-source-evidence");
        assert!(ledger
            .request_evidence(conversation, user, evidence)?
            .is_some());
        assert_eq!(uncertain.calls.get(), 1);
        assert_eq!(model.submits, 6);
        assert!(ledger
            .expected(
                Namespace::Conversation,
                writer::stage(admitted.operation, "reader-draft")
            )?
            .0
            .is_empty());

        let healthy = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let change = healthy.clone();
        model.before_submission = Some(Box::new(move || {
            change.store(false, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }));
        let admitted = scope(&ledger, conversation, &buddy)?;
        let key = same.original.key().clone();
        assert!(dispatch
            .reader_with_source(
                &ledger,
                &mut model,
                "Pressure compensation?",
                None,
                &admitted,
                SourceDirective::Inherit {
                    request_turn: anchor.0,
                    operation: Some(anchor.1),
                    expected: Some(key.clone())
                },
                Some(FollowupSource {
                    key,
                    provider: &same,
                    check: Arc::new(move || {
                        ensure!(
                            healthy.load(std::sync::atomic::Ordering::SeqCst),
                            "modeled original changed"
                        );
                        Ok(())
                    }),
                }),
                &mut || Ok(buddy.clone())
            )
            .is_err());
        assert_eq!(same.calls.get(), 4);
        assert_eq!(model.submits, 6);

        let drift_after_answer = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let change = drift_after_answer.clone();
        model.model.after_execute = Some(Box::new(move || {
            change.store(false, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }));
        model.model.replies.push_back("An invalidated draft".into());
        let admitted = scope(&ledger, conversation, &buddy)?;
        let key = same.original.key().clone();
        assert!(dispatch
            .reader_with_source(
                &ledger,
                &mut model,
                "Pressure compensation?",
                None,
                &admitted,
                SourceDirective::Inherit {
                    request_turn: anchor.0,
                    operation: Some(anchor.1),
                    expected: Some(key.clone()),
                },
                Some(FollowupSource {
                    key,
                    provider: &same,
                    check: Arc::new(move || {
                        ensure!(
                            drift_after_answer.load(std::sync::atomic::Ordering::SeqCst),
                            "modeled original changed"
                        );
                        Ok(())
                    }),
                }),
                &mut || Ok(buddy.clone()),
            )
            .is_err());
        assert_eq!(same.calls.get(), 5);
        assert_eq!(model.submits, 7);
        assert!(ledger
            .expected(
                Namespace::Conversation,
                writer::stage(admitted.operation, "reader-draft")
            )?
            .0
            .is_empty());

        let lose_read_set = || {
            ledger.append(
                Uuid::new_v4(),
                ledger.expected(Namespace::Conversation, conversation)?,
                Turn {
                    id: Uuid::new_v4(),
                    conversation,
                    exchange: Uuid::new_v4(),
                    sequence: 0,
                    role: Role::User,
                    mode: Mode::Reader,
                    outcome: Outcome::Interpreted,
                    text: Some("A newer request.".into()),
                    sources: vec![],
                    correction_of: None,
                    created_ms: 1,
                    updated_ms: 1,
                    completion: None,
                },
                vec![],
            )?;
            Ok(())
        };
        let stale = FollowupIndex {
            original: pdf_index(original.document, b"original modeled PDF", "pdf-fixture-v2")?,
            calls: Cell::new(0),
            after_query: Some(&lose_read_set),
        };
        let admitted = scope(&ledger, conversation, &buddy)?;
        let key = stale.original.key().clone();
        assert!(dispatch
            .reader_with_source(
                &ledger,
                &mut model,
                "Pressure compensation?",
                None,
                &admitted,
                SourceDirective::Inherit {
                    request_turn: anchor.0,
                    operation: Some(anchor.1),
                    expected: Some(key.clone()),
                },
                Some(FollowupSource {
                    key,
                    provider: &stale,
                    check: Arc::new(|| Ok(()))
                }),
                &mut || Ok(buddy.clone()),
            )
            .is_err());
        assert_eq!(stale.calls.get(), 1);
        assert_eq!(model.submits, 7);
        assert!(ledger
            .request_evidence(
                conversation,
                writer::stage(admitted.operation, "reader-user"),
                writer::stage(admitted.operation, "reader-source-evidence"),
            )?
            .is_none());

        let admitted = scope(&ledger, conversation, &buddy)?;
        let published = writer::stage(admitted.operation, "reader-source-evidence");
        let store = ledger.store().clone();
        let original_anchor = anchor.1;
        model.before_submission = Some(Box::new(move || {
            let id = crate::conversation::evidence::RequestEvidence::identity(published);
            assert!(store.value(Namespace::Conversation, id)?.is_some());
            conflict_followup_evidence(&store, original_anchor)
        }));
        let key = same.original.key().clone();
        let result = dispatch.reader_with_source(
            &ledger,
            &mut model,
            "Pressure compensation?",
            None,
            &admitted,
            SourceDirective::Inherit {
                request_turn: anchor.0,
                operation: Some(anchor.1),
                expected: Some(key.clone()),
            },
            Some(FollowupSource {
                key,
                provider: &same,
                check: Arc::new(|| Ok(())),
            }),
            &mut || Ok(buddy.clone()),
        );
        assert_eq!(
            result
                .err()
                .expect("changed ledger reference must refuse")
                .to_string(),
            "captured reference changed"
        );
        assert!(model.before_submission.is_none());
        assert!(ledger
            .store()
            .value(
                Namespace::Conversation,
                crate::conversation::evidence::RequestEvidence::identity(published),
            )?
            .is_some());
        assert!(ledger
            .expected(
                Namespace::Conversation,
                writer::stage(admitted.operation, "reader-draft"),
            )?
            .0
            .is_empty());
        assert_eq!(same.calls.get(), 6);
        assert_eq!(model.submits, 7);

        let admitted = scope(&ledger, conversation, &buddy)?;
        let key = same.original.key().clone();
        assert!(dispatch
            .reader_with_source(
                &ledger,
                &mut model,
                "Pressure compensation?",
                None,
                &admitted,
                SourceDirective::Inherit {
                    request_turn: anchor.0,
                    operation: Some(anchor.1),
                    expected: Some(key.clone())
                },
                Some(FollowupSource {
                    key,
                    provider: &same,
                    check: Arc::new(|| Ok(()))
                }),
                &mut || Ok(buddy.clone()),
            )
            .is_err());
        assert_eq!(same.calls.get(), 6);
        assert_eq!(model.submits, 7);
        Ok(())
    }
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

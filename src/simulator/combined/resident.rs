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
#[path = "../../../tests/fixtures/epub.rs"]
mod epub_fixture;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        conversation::{
            receiving::{
                memory::research::{ResearchDirective, ResearchPolicy},
                ReceivingBudget, RequestSource, SourceAssociation,
            },
            resolution::SourceDirective,
        },
        retrieval::{Index, IndexKey, Provider, Retrieved, SourceFormat, SourceUnit},
        workflow::conversation_dispatch::{FollowupResearch, FollowupSource},
    };
    use std::{
        cell::Cell,
        sync::Arc,
        time::{Duration, Instant},
    };

    fn use_saved_policy<'a>(scope: &'a RequestScope, question: &str) -> FollowupResearch<'a> {
        FollowupResearch::new(
            scope,
            question,
            ResearchDirective::UsePolicy,
            None,
            Instant::now() + Duration::from_secs(15),
        )
    }

    fn followup_payload(parts: &[String]) -> Result<Value> {
        let researched: Value = serde_json::from_str(&parts[1])?;
        Ok(researched["context"]["request_context"].clone())
    }

    fn research_response(listener: &std::net::TcpListener, succeed: bool) -> Result<Value> {
        use std::io::{Read, Write};
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    ensure!(Instant::now() < deadline, "missing loopback search");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(error.into()),
            }
        };
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            ensure!(headers.len() < 16 * 1024, "loopback header bound");
            let mut byte = [0];
            stream.read_exact(&mut byte)?;
            headers.push(byte[0]);
        }
        let headers = std::str::from_utf8(&headers)?;
        ensure!(headers.starts_with("POST /v1/responses HTTP/1.1"));
        let size: usize = headers
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("content-length")
                    .then_some(value.trim())
            })
            .expect("loopback Content-Length")
            .parse()?;
        ensure!(size <= 1024 * 1024, "loopback request bound");
        let mut bytes = vec![0; size];
        stream.read_exact(&mut bytes)?;
        let request = serde_json::from_slice(&bytes)?;
        if !succeed {
            stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
            return Ok(request);
        }
        let url = "https://example.org/pressure";
        let selection = json!({"sources":[{"url":url,"title":"Pressure reference","excerpt":"Pressure changes under load."}]});
        let body = json!({
            "status":"completed",
            "output":[
                {"type":"web_search_call", "status":"completed", "action":{
                    "type":"search","queries":["pressure compensation"],"sources":[{"type":"url","url":url}]
                }},
                {"type":"message","role":"assistant","status":"completed","content":[
                    {"type":"output_text","text":selection.to_string()}
                ]}
            ]
        }).to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body)?;
        Ok(request)
    }

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
        structured_reply: bool,
        before_submission: Option<Box<dyn FnOnce() -> Result<()>>>,
    }

    impl LLMEngine for FollowupModel {
        fn guarded_timeout(&self) -> Option<Duration> {
            Some(Duration::from_millis(5))
        }
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
            let reply = self.model.execute()?;
            if self.structured_reply {
                return Ok(reply);
            }
            Ok(json!({"status": "answered", "text": reply, "citations": []}).to_string())
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
    fn ordinary_reader_followup_research_uses_the_owned_frozen_context() -> Result<()> {
        use crate::{config::researched_reader::OrdinarySearch, memory};
        use memory::Provider as _;
        use std::net::TcpListener;

        const QUESTION: &str = "Pressure compensation?";
        const OFFLINE: &str = "Pressure compensation? Do not use the internet.";
        const SEARCH: &str = "gpt-5.6-terra";
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
        let index = FollowupIndex {
            original: pdf_index(
                original.document,
                b"researched modeled PDF",
                "pdf-fixture-v1",
            )?,
            calls: Cell::new(0),
            after_query: None,
        };
        let key = index.original.key().clone();
        let choice = RequestSource::Original {
            origin_turn: capture.turn,
            source_record: capture.id,
            expected: key.clone(),
        };
        let original_operation = Uuid::new_v4();
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
            Some(&index.original),
            |text| Ok(text.len()),
        )?;
        let original_request = evidence.context().request().clone();
        ledger.persist_request_evidence(original_operation, 1, &evidence)?;

        let mut config = Config {
            paths: ledger.store().paths.clone(),
            model_credential: Some("fixture_key".into()),
            ..Config::default()
        };
        let profile = Uuid::new_v4();
        config.memory.enabled = true;
        config.memory.profile = Some(profile);
        let credentials = &config.paths.credentials;
        std::fs::create_dir_all(credentials)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(credentials, std::fs::Permissions::from_mode(0o700))?;
        }
        crate::storage::files::atomic(&credentials.join("fixture_key"), b"fixture-secret")?;
        let mut dispatch = Dispatch::local(&config, &ledger)?;
        let local_memory = dispatch.local_memory().unwrap();
        let fact = Uuid::new_v4();
        local_memory.put(memory::Mutation {
            operation: Uuid::new_v4(),
            expected: Default::default(),
            fact: memory::Fact {
                key: memory::Key {
                    profile,
                    scope: memory::Scope::Document {
                        document: original.document,
                    },
                    fact,
                },
                claim: "pressure compensation".into(),
                value: "Historical pressure offset".into(),
                subject: "pressure compensation".into(),
                tags: vec![],
                provenance: memory::Provenance {
                    basis: memory::Basis::UserConfirmed,
                    description: "Modeled Reader research".into(),
                    conversation: None,
                    turn: None,
                    source: None,
                },
                confidence_basis_points: 9000,
                observed_ms: 1,
                expires_ms: None,
            },
        })?;

        let run = |dispatch: &mut Dispatch<'_>,
                   model: &mut FollowupModel,
                   admitted: &RequestScope,
                   question: &str,
                   research: FollowupResearch<'_>,
                   check: Arc<dyn Fn() -> Result<()> + Send + Sync>| {
            dispatch.reader_with_source(
                &ledger,
                model,
                question,
                None,
                admitted,
                SourceDirective::Inherit {
                    request_turn: capture.turn,
                    operation: Some(original_operation),
                    expected: Some(key.clone()),
                },
                Some(FollowupSource {
                    key: key.clone(),
                    provider: &index,
                    check,
                }),
                research,
                &mut || Ok(buddy.clone()),
            )
        };
        let mut model = FollowupModel {
            structured_reply: true,
            ..Default::default()
        };
        model.model.replies.push_back(json!({
            "status":"answered","text":"Pressure evidence [S1] [M1] [R1].","citations":["S1","M1","R1"]
        }).to_string());
        let mut answered = None;
        for (case, search_ok, should_answer, drift) in [
            ("required success", true, true, false),
            ("failed search", false, false, false),
            ("source drift at answer handoff", true, false, true),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0")?;
            listener.set_nonblocking(true)?;
            let endpoint = format!("http://{}/v1/responses", listener.local_addr()?);
            let admitted = scope(&ledger, conversation, &buddy)?;
            let valid = Arc::new(std::sync::atomic::AtomicBool::new(true));
            if drift {
                let valid = valid.clone();
                model.before_submission = Some(Box::new(move || {
                    valid.store(false, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                }));
            }
            let research = FollowupResearch::new(
                &admitted,
                QUESTION,
                ResearchDirective::UsePolicy,
                Some(OrdinarySearch {
                    model: SEARCH,
                    fixture_endpoint: &endpoint,
                }),
                Instant::now() + Duration::from_secs(15),
            );
            let (result, web_request) = std::thread::scope(|thread| {
                let witness = &listener;
                let peer = thread.spawn(move || research_response(witness, search_ok));
                let result = run(
                    &mut dispatch,
                    &mut model,
                    &admitted,
                    QUESTION,
                    research,
                    Arc::new(move || {
                        ensure!(
                            valid.load(std::sync::atomic::Ordering::SeqCst),
                            "modeled source drift"
                        );
                        Ok(())
                    }),
                );
                (result, peer.join().expect("loopback search peer panicked"))
            });
            let web_request = web_request?;
            ensure!(
                web_request["model"] == SEARCH
                    && web_request["tool_choice"] == "required"
                    && web_request["store"] == false
            );
            let content = &web_request["input"][0]["content"];
            assert_eq!(content.as_array().unwrap().len(), 1);
            assert_eq!(content[0]["type"], "input_text");
            let search_frame: Value = serde_json::from_str(content[0]["text"].as_str().unwrap())?;
            assert_eq!(
                result.is_ok(),
                should_answer,
                "{case}: {:?}",
                result.as_ref().err()
            );
            if drift {
                assert_eq!(result.err().unwrap().to_string(), "modeled source drift");
            }
            if should_answer {
                answered = Some((admitted.operation, search_frame));
            }
            assert_eq!(
                ledger
                    .store()
                    .value(
                        Namespace::Conversation,
                        writer::stage(admitted.operation, "reader-draft")
                    )?
                    .is_some(),
                should_answer,
                "{case}"
            );
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            let queries = index.calls.get();
            let memory_queries = local_memory.query_count();
            let submissions = model.submits;
            let repeat = FollowupResearch::new(
                &admitted,
                QUESTION,
                ResearchDirective::UsePolicy,
                Some(OrdinarySearch {
                    model: SEARCH,
                    fixture_endpoint: &endpoint,
                }),
                Instant::now() + Duration::from_secs(15),
            );
            assert!(
                run(
                    &mut dispatch,
                    &mut model,
                    &admitted,
                    QUESTION,
                    repeat,
                    Arc::new(|| Ok(())),
                )
                .is_err(),
                "{case} replay"
            );
            assert_eq!(index.calls.get(), queries);
            assert_eq!(local_memory.query_count(), memory_queries);
            assert_eq!(model.submits, submissions);
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
        assert_eq!(model.submits, 1);
        assert_eq!(index.calls.get(), 3);
        assert_eq!(local_memory.query_count(), 3);
        let (answered, search_frame) = answered.unwrap();
        let (parts, attached) = &model.model.seen[0];
        let frame: Value = serde_json::from_str(&parts[1])?;
        assert_eq!(frame["context"], search_frame);
        assert_eq!(
            frame["context"]["request_context"]["request"]["text"],
            QUESTION
        );
        assert_eq!(
            frame["context"]["request_context"]["history"],
            json!([original_request])
        );
        assert_eq!(
            frame["context"]["request_context"]["source"]["source_record"]["record"]["id"],
            json!(capture.id)
        );
        assert_eq!(
            frame["context"]["memory"]["query"]["document"],
            json!(original.document)
        );
        assert!(parts[1].contains("Historical pressure offset"));
        assert_eq!(frame["research"]["basis"], "provider-asserted");
        let associations: Value = serde_json::from_str(parts.last().unwrap())?;
        assert_eq!(
            associations["historical_images"].as_array().unwrap().len(),
            images.len()
        );
        assert_eq!(
            *attached,
            images
                .iter()
                .map(|image| STANDARD.encode(image))
                .collect::<Vec<_>>()
        );
        let draft = ledger
            .store()
            .value(
                Namespace::Conversation,
                writer::stage(answered, "reader-draft"),
            )?
            .unwrap();
        let Record::Turn(turn) = Ledger::decode(&draft)? else {
            anyhow::bail!("fixture Reader draft kind");
        };
        assert_eq!(turn.outcome, Outcome::Generated);
        let text = turn.text.unwrap();
        assert!(text.contains(&format!("[S1] Document {}, page 3", original.document)));
        assert!(text.contains("[M1] Memory ") && text.contains("(author asserted)"));
        assert!(text.contains("[R1] Pressure reference - https://example.org/pressure (provider reported, unverified)"));

        drop(dispatch);
        drop(local_memory);
        config.model_credential = None;
        let mut disabled = config.clone();
        disabled.research_policy = Some(Some(ResearchPolicy::Disabled));
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let endpoint = format!("http://{}/v1/responses", listener.local_addr()?);
        let mismatched = scope(&ledger, conversation, &buddy)?;
        let mut dispatch = Dispatch::local(&config, &ledger)?;
        let intent = FollowupResearch::new(
            &mismatched,
            OFFLINE,
            ResearchDirective::NoInternet,
            None,
            Instant::now() + Duration::from_secs(15),
        );
        let error = run(
            &mut dispatch,
            &mut model,
            &mismatched,
            QUESTION,
            intent,
            Arc::new(|| Ok(())),
        )
        .err()
        .expect("mismatched intent");
        assert_eq!(
            error.to_string(),
            "Reader research intent does not match the current question"
        );
        let user = writer::stage(mismatched.operation, "reader-user");
        assert!(ledger
            .store()
            .value(Namespace::Conversation, user)?
            .is_none());
        assert_eq!(index.calls.get(), 3);
        assert_eq!(model.submits, 1);
        for (case, settings, directive, question, search, expected) in [
            (
                "no internet",
                &config,
                ResearchDirective::NoInternet,
                OFFLINE,
                None,
                Some("request-no-internet"),
            ),
            (
                "saved disabled",
                &disabled,
                ResearchDirective::UsePolicy,
                QUESTION,
                Some(SEARCH),
                Some("configured-disabled"),
            ),
            (
                "missing required",
                &config,
                ResearchDirective::UsePolicy,
                QUESTION,
                None,
                None,
            ),
            (
                "unqualified required",
                &config,
                ResearchDirective::UsePolicy,
                QUESTION,
                Some("unqualified"),
                None,
            ),
            (
                "invalid citation",
                &disabled,
                ResearchDirective::UsePolicy,
                QUESTION,
                None,
                None,
            ),
        ] {
            let mut dispatch = Dispatch::local(settings, &ledger)?;
            let admitted = scope(&ledger, conversation, &buddy)?;
            let research = FollowupResearch::new(
                &admitted,
                question,
                directive,
                search.map(|model| OrdinarySearch {
                    model,
                    fixture_endpoint: &endpoint,
                }),
                Instant::now() + Duration::from_secs(15),
            );
            let submits = model.submits;
            let adapter = expected.is_some() || case == "invalid citation";
            if adapter {
                let (text, citations) = if case == "invalid citation" {
                    ("Invented [R1]", vec!["R1"])
                } else {
                    ("Offline answer", vec![])
                };
                model.model.replies.push_back(
                    json!({"status":"answered","text":text,"citations":citations}).to_string(),
                );
            }
            let result = run(
                &mut dispatch,
                &mut model,
                &admitted,
                question,
                research,
                Arc::new(|| Ok(())),
            );
            assert_eq!(result.is_ok(), expected.is_some(), "{case}");
            assert_eq!(model.submits, submits + usize::from(adapter), "{case}");
            assert_eq!(
                ledger
                    .store()
                    .value(
                        Namespace::Conversation,
                        writer::stage(admitted.operation, "reader-draft")
                    )?
                    .is_some(),
                expected.is_some(),
                "{case}"
            );
            if let Some(expected) = expected {
                let frame: Value = serde_json::from_str(&model.model.seen.last().unwrap().0[1])?;
                assert_eq!(frame["research"]["disabled_reason"], expected);
                assert_eq!(
                    frame["context"]["request_context"]["request"]["text"],
                    question
                );
            }
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock,
                "{case}"
            );
        }
        Ok(())
    }

    fn seed_notebook_archive(
        ledger: &Ledger,
        config: &Config,
        scope: &RequestScope,
        input: &writer::notebook::ValidatedNotebook,
    ) -> Result<crate::conversation::ArchivedNormalizedSource> {
        use writer::notebook::command::{self, Generate, Request};

        let request = Request::GenerateNotebook(Box::new(Generate {
            conversation: scope.conversation,
            operation: scope.operation,
            request: "Summarize the supplied notebook.".into(),
            document_file: "owner-frozen-unused.json".into(),
            document_sha256: input.raw_sha256().into(),
            selection: crate::conversation::TextScope::SuppliedNotebook,
            guidance: None,
            max_model_calls: 2,
            history_range: None,
        }));
        let prepared = command::prepare_receiving(
            ledger,
            &request,
            &config.personalization_snapshot()?,
            Some(input.clone()),
            &scope.root,
            scope.source.document,
            None,
        )?;
        let archive = prepared.admission.archive.clone();
        assert!(ledger.admit_notebook(prepared.admission)?.is_some());
        Ok(archive)
    }

    #[test]
    fn ordinary_reader_notebook_followups_require_fresh_supplied_ocr() -> Result<()> {
        use crate::{
            conversation::{receiving::ReceivingRefusal, TurnRange},
            retrieval::notebook::{self, Options, PageImage},
            workflow::conversation_dispatch::NotebookFollowupSource,
            writer::notebook::ValidatedNotebook,
        };
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        let fixture = SimulatorLedger::new();
        let base = fixture.open()?;
        let paths = base.store().paths.clone();
        drop(base);
        let ledger = Ledger::new(Arc::new(crate::storage::Store::prepare_notebook(paths)?));
        let conversation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 1)?;
        let source = fixture_source(Uuid::new_v4());
        let target = fixture_source(Uuid::new_v4());
        let buddy = fixture_source(Uuid::new_v4());
        let config = Config {
            research_policy: Some(Some(ResearchPolicy::Disabled)),
            ..Config::default()
        };
        let one = fixture_png(17)?;
        let blank = fixture_png(29)?;
        let last = fixture_png(41)?;
        let pages = [
            PageImage { page: 3, png: &one },
            PageImage {
                page: 9,
                png: &blank,
            },
            PageImage {
                page: 14,
                png: &last,
            },
        ];
        let normalize = |document, profile: &str, text: &str| {
            let mut model = Model::default();
            model.replies.push_back(
                json!({"pages": [
                    {"page":3,"status":"transcribed","text":"Trajectory calibration."},
                    {"page":9,"status":"blank","text":""},
                    {"page":14,"status":"transcribed","text":text}
                ]})
                .to_string(),
            );
            notebook::normalize(
                document,
                &pages,
                profile,
                &mut model,
                Options::default(),
                &mut |_| Ok(()),
            )
        };
        let normalize_input = |ocr: &notebook::NotebookText| {
            let bytes = serde_json::to_vec(&ocr.document)?;
            ValidatedNotebook::from_bytes(&bytes, &digest(&bytes))
        };
        let pressure = "Pressure compensation procedure.";
        let ocr = normalize(source.document, "notebook-fixture", pressure)?;
        let input = normalize_input(&ocr)?;
        let original = seed_notebook_archive(
            &ledger,
            &config,
            &scope(&ledger, conversation, &source)?,
            &input,
        )?;
        let other_ocr = normalize(
            target.document,
            "notebook-fixture",
            "Thermal insulation procedure.",
        )?;
        let other_input = normalize_input(&other_ocr)?;
        let other = seed_notebook_archive(
            &ledger,
            &config,
            &scope(&ledger, conversation, &target)?,
            &other_input,
        )?;
        let (_, png) = seed_image(&ledger, conversation, &buddy, 61)?;
        let key = Index::build(input.document().clone())?.key().clone();
        let first_choice = RequestSource::Original {
            origin_turn: original.turn,
            source_record: original.id,
            expected: key.clone(),
        };
        let first = scope(&ledger, conversation, &source)?;
        let mut dispatch = Dispatch::local(&config, &ledger)?;
        let mut model = FollowupModel {
            structured_reply: true,
            ..Default::default()
        };
        let response =
            json!({"status":"answered","text":"See [S1].","citations":["S1"]}).to_string();
        for _ in 0..3 {
            model.model.replies.push_back(response.clone());
        }
        model.model.replies.push_back(
            json!({"status":"clarification","text":"No match in the supplied pages.","citations":[]})
                .to_string(),
        );
        let admission = || NotebookFollowupSource {
            input: &input,
            ocr: &ocr,
            pages: &pages,
            check: Arc::new(|| Ok(())),
        };
        let question = "Trajectory calibration?";
        dispatch.reader_with_notebook(
            &ledger,
            &mut model,
            question,
            None,
            &first,
            SourceDirective::Explicit(first_choice.clone()),
            admission(),
            use_saved_policy(&first, question),
            &mut || Ok(source.clone()),
        )?;
        let first_user = writer::stage(first.operation, "reader-user");
        let first_operation = writer::stage(first.operation, "reader-source-evidence");
        let inherited = || SourceDirective::Inherit {
            request_turn: first_user,
            operation: Some(first_operation),
            expected: Some(key.clone()),
        };
        let first_payload = followup_payload(&model.model.seen[0].0)?;
        let first_passages = first_payload["retrieval"]["passages"].as_array().unwrap();
        assert_eq!(
            first_passages
                .iter()
                .filter(|p| p["direct_match"] == true)
                .count(),
            1
        );
        assert_eq!(first_passages[0]["page"], 3);
        assert!(first_passages.iter().all(|p| p["page"] != 14));

        let second = scope(&ledger, conversation, &buddy)?;
        let read = |visit: &RequestScope| {
            let user = writer::stage(visit.operation, "reader-user");
            let operation = writer::stage(visit.operation, "reader-source-evidence");
            ledger.request_evidence(conversation, user, operation)
        };
        let question = "Pressure compensation?";
        let second_output = dispatch.reader_with_notebook(
            &ledger,
            &mut model,
            question,
            Some(TurnRange {
                start: 2,
                end_exclusive: 6,
            }),
            &second,
            inherited(),
            admission(),
            use_saved_policy(&second, question),
            &mut || Ok(buddy.clone()),
        )?;
        let completion_scope = RequestScope {
            operation: second.operation,
            root: second_output.root().clone(),
            ..scope(&ledger, conversation, &buddy)?
        };
        let effect = Cell::new(0);
        assert!(second_output
            .admit_followup_output(
                &ledger,
                &completion_scope,
                &mut || Ok(source.clone()),
                || {
                    effect.set(effect.get() + 1);
                    Ok(())
                },
            )
            .is_err());
        assert_eq!(effect.get(), 0);
        let (parts, attachments) = &model.model.seen[1];
        assert_eq!(attachments.as_slice(), std::slice::from_ref(&png));
        assert!(parts[0].contains("unverified historical OCR"));
        let payload = followup_payload(parts)?;
        assert_eq!(payload["request"]["text"], question);
        let expected_history: Vec<_> = ledger
            .inspect(conversation, false)?
            .into_iter()
            .filter_map(|record| match record {
                Record::Turn(turn) if [2, 3].contains(&turn.sequence) => Some(turn),
                _ => None,
            })
            .collect();
        assert_eq!(payload["history"], json!(expected_history));
        assert!(expected_history.iter().all(|turn| turn.id != original.turn));
        assert_eq!(payload["source"]["origin_turn"], json!(original.turn));
        assert_eq!(
            payload["source"]["source_record"]["record"]["id"],
            json!(original.id)
        );
        let later = &payload["retrieval"]["passages"][0];
        assert_eq!(later["page"], 14);
        assert_eq!(later["unit"], 2);
        assert_eq!(later["range"], json!([0, pressure.len()]));
        assert_eq!(later["text"], pressure);
        assert!(later["section"].is_null());
        let saved = read(&second)?.unwrap();
        assert!(saved.record.sources.iter().any(|r| r.record == original.id));
        assert_eq!(
            serde_json::from_str::<Value>(&saved.record.frame_json)?["source"],
            payload["source"]
        );
        assert!(saved.unavailable_media.is_empty());
        let second_draft = writer::stage(second.operation, "reader-draft");
        let record = ledger
            .store()
            .value(Namespace::Conversation, second_draft)?
            .unwrap();
        let Record::Turn(draft) = Ledger::decode(&record)? else {
            anyhow::bail!("notebook answer draft absent")
        };
        assert_eq!(draft.outcome, Outcome::Generated);
        assert!(draft
            .text
            .unwrap()
            .contains("page 14 (unverified historical OCR)"));

        let retarget = scope(&ledger, conversation, &buddy)?;
        let other_key = Index::build(other_input.document().clone())?.key().clone();
        let question = "Thermal insulation?";
        dispatch.reader_with_notebook(
            &ledger,
            &mut model,
            question,
            None,
            &retarget,
            SourceDirective::Explicit(RequestSource::Original {
                origin_turn: other.turn,
                source_record: other.id,
                expected: other_key,
            }),
            NotebookFollowupSource {
                input: &other_input,
                ocr: &other_ocr,
                pages: &pages,
                check: Arc::new(|| Ok(())),
            },
            use_saved_policy(&retarget, question),
            &mut || Ok(buddy.clone()),
        )?;
        let payload = followup_payload(&model.model.seen[2].0)?;
        assert_eq!(payload["source"]["origin_turn"], json!(other.turn));
        assert_eq!(
            payload["retrieval"]["passages"][0]["text"],
            "Thermal insulation procedure."
        );

        let empty = scope(&ledger, conversation, &buddy)?;
        let question = "Unlisted geology?";
        dispatch.reader_with_notebook(
            &ledger,
            &mut model,
            question,
            None,
            &empty,
            inherited(),
            admission(),
            use_saved_policy(&empty, question),
            &mut || Ok(buddy.clone()),
        )?;
        let payload = followup_payload(&model.model.seen[3].0)?;
        assert_eq!(payload["retrieval"]["source"], json!(key));
        assert_eq!(payload["retrieval"]["passages"], json!([]));
        assert_eq!(model.submits, 4);
        for success in [&first, &second, &retarget, &empty] {
            let generated_id = writer::stage(success.operation, "reader-draft");
            let expected = ledger.expected(Namespace::Conversation, generated_id)?;
            assert_eq!(expected.0.len(), 1);
            assert!(read(success)?.is_some());
        }

        let budget = ReceivingBudget {
            context: crate::conversation::ContextBudget {
                max_text_bytes: 16 * 1024,
                max_turns: 64,
                provider_token_limit: Some(96 * 1024),
            },
            max_encoded_bytes: 96 * 1024,
            max_passages: 8,
            max_passage_bytes: 16 * 1024,
            neighbors: false,
        };
        let probe = FollowupIndex {
            original: Index::build(input.document().clone())?,
            calls: Cell::new(0),
            after_query: None,
        };
        assert!(ledger
            .source_request_context(
                conversation,
                first_user,
                None,
                first_choice.clone(),
                budget.clone(),
                Some(&probe),
                |text| Ok(text.len()),
            )
            .is_err());
        assert!(ledger
            .prepare_request_evidence(
                conversation,
                first_user,
                None,
                first_choice,
                budget.clone(),
                Some(&probe),
                |text| Ok(text.len()),
            )
            .is_err());
        let no_document = ledger.prepare_request_evidence(
            conversation,
            first_user,
            None,
            RequestSource::NoDocument,
            budget.clone(),
            None,
            |text| Ok(text.len()),
        )?;
        let no_document_operation = Uuid::new_v4();
        ledger.persist_request_evidence(no_document_operation, 1, &no_document)?;
        let history = ledger
            .request_evidence(conversation, first_user, no_document_operation)?
            .unwrap();
        assert!(history
            .record
            .sources
            .iter()
            .any(|r| r.record == original.id));

        let captured_origin = ledger
            .inspect(conversation, false)?
            .into_iter()
            .find_map(|record| match record {
                Record::Source(source) if source.conversation == conversation => {
                    Some((source.turn, source.id))
                }
                _ => None,
            })
            .unwrap();
        let pdf = FollowupIndex {
            original: pdf_index(buddy.document, b"ordinary history control", "pdf-fixture")?,
            calls: Cell::new(0),
            after_query: None,
        };
        let pdf_evidence = ledger.prepare_request_evidence(
            conversation,
            first_user,
            None,
            RequestSource::Original {
                origin_turn: captured_origin.0,
                source_record: captured_origin.1,
                expected: pdf.original.key().clone(),
            },
            budget,
            Some(&pdf),
            |text| Ok(text.len()),
        )?;
        let pdf_operation = Uuid::new_v4();
        ledger.persist_request_evidence(pdf_operation, 1, &pdf_evidence)?;
        assert_eq!(pdf.calls.get(), 1);
        let history = ledger
            .request_evidence(conversation, first_user, pdf_operation)?
            .unwrap();
        assert!(history
            .record
            .sources
            .iter()
            .any(|r| r.record == original.id));

        let refused = scope(&ledger, conversation, &buddy)?;
        let question = "Pressure compensation?";
        assert!(dispatch
            .reader_with_source(
                &ledger,
                &mut model,
                question,
                None,
                &refused,
                inherited(),
                Some(FollowupSource {
                    key: key.clone(),
                    provider: &probe,
                    check: Arc::new(|| Ok(()))
                }),
                use_saved_policy(&refused, question),
                &mut || Ok(buddy.clone()),
            )
            .is_err());
        assert_eq!(probe.calls.get(), 0);

        let mut raw = serde_json::to_vec(&ocr.document)?;
        raw.push(b' ');
        let altered_raw = ValidatedNotebook::from_bytes(&raw, &digest(&raw))?;
        let altered_profile = normalize(source.document, "different-profile", pressure)?;
        let altered_profile_input = normalize_input(&altered_profile)?;
        let altered_units = normalize(source.document, "notebook-fixture", "Unexpected words.")?;
        let altered_units_input = normalize_input(&altered_units)?;
        let changed_images = [
            PageImage {
                page: 3,
                png: &last,
            },
            PageImage {
                page: 9,
                png: &blank,
            },
            PageImage {
                page: 14,
                png: &one,
            },
        ];
        for (frozen, report, set) in [
            (&altered_raw, &ocr, &pages[..]),
            (&altered_profile_input, &altered_profile, &pages[..]),
            (&altered_units_input, &altered_units, &pages[..]),
            (&input, &ocr, &changed_images[..]),
            (&input, &ocr, &pages[..2]),
        ] {
            let attempt = scope(&ledger, conversation, &buddy)?;
            let attempt_key = Index::build(frozen.document().clone())?.key().clone();
            assert!(dispatch
                .reader_with_notebook(
                    &ledger,
                    &mut model,
                    question,
                    None,
                    &attempt,
                    SourceDirective::Inherit {
                        request_turn: first_user,
                        operation: Some(first_operation),
                        expected: Some(attempt_key)
                    },
                    NotebookFollowupSource {
                        input: frozen,
                        ocr: report,
                        pages: set,
                        check: Arc::new(|| Ok(()))
                    },
                    use_saved_policy(&attempt, question),
                    &mut || Ok(buddy.clone()),
                )
                .is_err());
            assert!(read(&attempt)?.is_none());
        }
        assert_eq!(model.submits, 4);

        let healthy = Arc::new(AtomicBool::new(true));
        let drift = healthy.clone();
        model.before_submission = Some(Box::new(move || {
            drift.store(false, Ordering::SeqCst);
            Ok(())
        }));
        let attempt = scope(&ledger, conversation, &buddy)?;
        assert!(dispatch
            .reader_with_notebook(
                &ledger,
                &mut model,
                question,
                None,
                &attempt,
                inherited(),
                NotebookFollowupSource {
                    check: Arc::new(move || {
                        ensure!(
                            healthy.load(Ordering::SeqCst),
                            "modeled notebook owner revoked"
                        );
                        Ok(())
                    }),
                    ..admission()
                },
                use_saved_policy(&attempt, question),
                &mut || Ok(buddy.clone()),
            )
            .is_err());
        assert!(model.before_submission.is_none());
        assert!(read(&attempt)?.is_some());
        assert_eq!(model.submits, 4);
        assert!(ledger
            .expected(
                Namespace::Conversation,
                writer::stage(attempt.operation, "reader-draft")
            )?
            .0
            .is_empty());

        let uncertain = scope(&ledger, conversation, &buddy)?;
        let user = writer::stage(uncertain.operation, "reader-user");
        let evidence = writer::stage(uncertain.operation, "reader-source-evidence");
        let store = ledger.store().clone();
        let after_append = Arc::new(AtomicUsize::new(0));
        let seen = after_append.clone();
        let fault = Arc::new(move || {
            if store.value(Namespace::Conversation, user)?.is_some()
                && seen.fetch_add(1, Ordering::SeqCst) == 1
            {
                // The second source check after append is the post-query guard.
                store.set_fault(crate::storage::Fault::AfterCommit)?;
            }
            Ok(())
        });
        for _ in 0..2 {
            assert!(dispatch
                .reader_with_notebook(
                    &ledger,
                    &mut model,
                    question,
                    None,
                    &uncertain,
                    inherited(),
                    NotebookFollowupSource {
                        check: fault.clone(),
                        ..admission()
                    },
                    use_saved_policy(&uncertain, question),
                    &mut || Ok(buddy.clone()),
                )
                .is_err());
        }
        let saved = read(&uncertain)?.expect("original notebook evidence receipt");
        assert_eq!(saved.acknowledgment.operation, evidence);
        assert_eq!(after_append.load(Ordering::SeqCst), 3);
        assert_eq!(model.submits, 4);
        assert!(ledger
            .expected(
                Namespace::Conversation,
                writer::stage(uncertain.operation, "reader-draft")
            )?
            .0
            .is_empty());

        let generation = saved.record.snapshot.generation;
        let path = ledger
            .store()
            .paths
            .data
            .join("generations")
            .join(generation.to_string())
            .join("objects")
            .join(&original.media.sha256);
        std::fs::remove_file(path)?;
        let unavailable = scope(&ledger, conversation, &buddy)?;
        let error = dispatch
            .reader_with_notebook(
                &ledger,
                &mut model,
                question,
                None,
                &unavailable,
                inherited(),
                admission(),
                use_saved_policy(&unavailable, question),
                &mut || Ok(buddy.clone()),
            )
            .err()
            .expect("missing canonical JSON must refuse");
        assert_eq!(
            error.downcast_ref::<ReceivingRefusal>(),
            Some(&ReceivingRefusal::OriginalUnavailable)
        );
        assert!(read(&unavailable)?.is_none());
        assert!(read(&second)?
            .unwrap()
            .unavailable_media
            .contains(&original.media));
        assert_eq!(model.submits, 4);
        Ok(())
    }

    #[test]
    fn ordinary_reader_epub_followup_preserves_archive_identity_and_section() -> Result<()> {
        use super::epub_fixture::{book, package, xhtml, CONTAINER};
        use crate::retrieval::epub;
        use std::sync::atomic::{AtomicBool, Ordering};

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
        let opf = package(
            r#"<item id="later" href="later.xhtml" media-type="application/xhtml+xml"/><item id="intro" href="intro.xhtml" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="intro"/><itemref idref="later"/>"#,
        );
        let archive = |comment: &str| {
            book(
                CONTAINER,
                &opf,
                &[
                    (
                        "book/later.xhtml",
                        xhtml("<p>Pressure compensation procedure.</p>"),
                    ),
                    (
                        "book/intro.xhtml",
                        xhtml(&format!("<p>Earlier image question.</p><!-- {comment} -->")),
                    ),
                ],
            )
        };
        let bytes = archive("original");
        let baseline = epub::normalize(original.document, bytes.as_slice())?.document;
        assert_eq!(baseline.source_sha256, digest(&bytes));
        let original_index = Index::build(baseline.clone())?;
        let operation = Uuid::new_v4();
        let prior = ledger.prepare_request_evidence(
            conversation,
            capture.turn,
            None,
            RequestSource::Original {
                origin_turn: capture.turn,
                source_record: capture.id,
                expected: original_index.key().clone(),
            },
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
            Some(&original_index),
            |text| Ok(text.len()),
        )?;
        let prior_passages = &prior.context().retrieval().unwrap().passages;
        assert_eq!(prior_passages.len(), 1);
        assert_eq!(
            prior_passages[0].section.as_deref(),
            Some("book/intro.xhtml")
        );
        ledger.persist_request_evidence(operation, 1, &prior)?;

        // Model a new extractor revision over the independently normalized
        // same archive. A new complete key does not change original bytes.
        let mut reindexed = epub::normalize(original.document, bytes.as_slice())?.document;
        assert_eq!(reindexed, baseline);
        reindexed.extractor_revision = "epub-fixture-reindex-v2".into();
        let index = FollowupIndex {
            original: Index::build(reindexed)?,
            calls: Cell::new(0),
            after_query: None,
        };
        let key = index.original.key().clone();
        assert_ne!(&key, original_index.key());
        assert_eq!(key.source_sha256, original_index.key().source_sha256);

        let config = Config {
            research_policy: Some(Some(ResearchPolicy::Disabled)),
            ..Config::default()
        };
        let mut dispatch = Dispatch::local(&config, &ledger)?;
        let mut model = FollowupModel::default();
        model
            .model
            .replies
            .push_back("See the pressure section.".into());
        let healthy = Arc::new(AtomicBool::new(true));
        let observer = healthy.clone();
        let check = Arc::new(move || {
            ensure!(
                observer.load(Ordering::SeqCst),
                "modeled EPUB archive changed"
            );
            Ok(())
        });
        let admitted = scope(&ledger, conversation, &buddy)?;
        let question = "Pressure compensation?";
        let old_request = prior.context().request().clone();
        dispatch.reader_with_source(
            &ledger,
            &mut model,
            question,
            None,
            &admitted,
            SourceDirective::Inherit {
                request_turn: capture.turn,
                operation: Some(operation),
                expected: Some(key.clone()),
            },
            Some(FollowupSource {
                key: key.clone(),
                provider: &index,
                check: check.clone(),
            }),
            use_saved_policy(&admitted, question),
            &mut || Ok(buddy.clone()),
        )?;
        assert_ne!(buddy.document, original.document);
        assert_eq!(index.calls.get(), 1);
        assert_eq!(model.submits, 1);
        let (parts, attached) = &model.model.seen[0];
        let payload = followup_payload(parts)?;
        assert_eq!(payload["request"]["text"], question);
        assert_eq!(payload["history"], json!([old_request]));
        assert_eq!(payload["source"]["origin_turn"], json!(capture.turn));
        assert_eq!(
            payload["source"]["source_record"]["record"]["id"],
            json!(capture.id)
        );
        assert_eq!(payload["source"]["expected"], json!(key));
        let passages = payload["retrieval"]["passages"].as_array().unwrap();
        assert_eq!(passages.len(), 1);
        assert_eq!(passages[0]["section"], "book/later.xhtml");
        assert!(passages[0]["page"].is_null());
        assert_eq!(passages[0]["text"], "Pressure compensation procedure.");
        assert_eq!(
            *attached,
            images
                .iter()
                .map(|image| STANDARD.encode(image))
                .collect::<Vec<_>>()
        );

        let prior_turn = writer::stage(admitted.operation, "reader-user");
        let prior_evidence = writer::stage(admitted.operation, "reader-source-evidence");
        let persisted = ledger
            .request_evidence(conversation, prior_turn, prior_evidence)?
            .unwrap();
        let frame: Value = serde_json::from_str(&persisted.record.frame_json)?;
        assert_eq!(frame["retrieval"], payload["retrieval"]);
        assert_eq!(frame["history"], payload["history"]);

        let changed_bytes = archive("changed bytes without changed text");
        let changed_archive =
            epub::normalize(original.document, changed_bytes.as_slice())?.document;
        assert_eq!(changed_archive.document, baseline.document);
        assert_eq!(changed_archive.units, baseline.units);
        assert_ne!(changed_archive.source_sha256, baseline.source_sha256);
        let mut changed_archive = changed_archive;
        changed_archive.extractor_revision = key.extractor_revision.clone();
        let changed = FollowupIndex {
            original: Index::build(changed_archive)?,
            calls: Cell::new(0),
            after_query: None,
        };
        assert_eq!(
            changed.original.key().normalized_sha256,
            key.normalized_sha256
        );
        let changed_key = changed.original.key().clone();
        let changed_scope = scope(&ledger, conversation, &buddy)?;
        let result = dispatch.reader_with_source(
            &ledger,
            &mut model,
            question,
            None,
            &changed_scope,
            SourceDirective::Inherit {
                request_turn: prior_turn,
                operation: Some(prior_evidence),
                expected: Some(changed_key.clone()),
            },
            Some(FollowupSource {
                key: changed_key,
                provider: &changed,
                check: check.clone(),
            }),
            use_saved_policy(&changed_scope, question),
            &mut || Ok(buddy.clone()),
        );
        assert_eq!(
            result
                .err()
                .expect("changed archive must refuse")
                .downcast_ref::<crate::conversation::resolution::SourceResolutionRefusal>(),
            Some(&crate::conversation::resolution::SourceResolutionRefusal::ChangedSource),
        );
        assert_eq!(changed.calls.get(), 0);
        assert_eq!(model.submits, 1);

        model.before_submission = Some(Box::new(move || {
            healthy.store(false, Ordering::SeqCst);
            Ok(())
        }));
        let stale_scope = scope(&ledger, conversation, &buddy)?;
        let result = dispatch.reader_with_source(
            &ledger,
            &mut model,
            question,
            None,
            &stale_scope,
            SourceDirective::Inherit {
                request_turn: prior_turn,
                operation: Some(prior_evidence),
                expected: Some(key.clone()),
            },
            Some(FollowupSource {
                key,
                provider: &index,
                check,
            }),
            use_saved_policy(&stale_scope, question),
            &mut || Ok(buddy.clone()),
        );
        assert_eq!(
            result.err().expect("stale EPUB must refuse").to_string(),
            "modeled EPUB archive changed"
        );
        assert_eq!(index.calls.get(), 2);
        assert_eq!(model.submits, 1);
        for refused in [changed_scope, stale_scope] {
            assert!(ledger
                .expected(
                    Namespace::Conversation,
                    writer::stage(refused.operation, "reader-draft")
                )?
                .0
                .is_empty());
        }
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

        let mut config = Config {
            research_policy: Some(Some(ResearchPolicy::Disabled)),
            ..Config::default()
        };
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
                use_saved_policy(&admitted, question),
                &mut || Ok(buddy.clone()),
            )?;
            let (parts, attached) = model.model.seen.last().unwrap();
            let payload = followup_payload(parts)?;
            assert_eq!(payload["request"]["text"], question);
            assert_eq!(payload["history"], json!(expected_history));
            assert_eq!(payload["selection"], json!(range));
            assert_eq!(payload["source"]["origin_turn"], json!(capture.turn));
            assert_eq!(
                payload["source"]["source_record"]["record"]["id"],
                json!(capture.id)
            );
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
                use_saved_policy(&admitted, "Pressure again?"),
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
                use_saved_policy(&admitted, question),
                &mut || Ok(buddy.clone()),
            )?;
            let (parts, _) = model.model.seen.last().unwrap();
            let payload = followup_payload(parts)?;
            let no_document = serde_json::to_value(SourceAssociation::NoDocument)?;
            assert_eq!(payload["request"]["text"], question);
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
            use_saved_policy(&admitted, "Pressure compensation?"),
            &mut || Ok(buddy.clone()),
        )?;
        let (parts, _) = model.model.seen.last().unwrap();
        let payload = followup_payload(parts)?;
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
            use_saved_policy(&admitted, "Zygomorphic?"),
            &mut || Ok(buddy.clone()),
        )?;
        assert_eq!(same.calls.get(), query_count + 1);
        assert_eq!(model.submits, 6);
        let (parts, _) = model.model.seen.last().unwrap();
        let payload = followup_payload(parts)?;
        assert_eq!(payload["source"]["kind"], "original");
        assert_eq!(payload["source"]["origin_turn"], json!(capture.turn));
        assert_eq!(
            payload["source"]["source_record"]["record"]["id"],
            json!(capture.id)
        );
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
                use_saved_policy(&admitted, "Pressure again?"),
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
                    use_saved_policy(&admitted, "Pressure compensation?"),
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
                use_saved_policy(&admitted, "Pressure compensation?"),
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
                use_saved_policy(&admitted, "Pressure compensation?"),
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
                use_saved_policy(&admitted, "Pressure compensation?"),
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
            use_saved_policy(&admitted, "Pressure compensation?"),
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
                use_saved_policy(&admitted, "Pressure compensation?"),
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

use super::*;
use crate::{
    conversation::receiving::memory::research::ResearchPolicy,
    memory::{self, Provider as _},
    workflow::orchestrator::researched_fixture,
};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::Path,
};

const QUESTION: &str = "Which compass should I use?";
const MARKED: &str = "G? unc. here";
const NOTEBOOK: &str = "The brass compass is for this notebook.";
const OTHER: &str = "The plastic compass is for the other notebook.";
const GLOBAL: &str = "Check the compass before departure.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Case {
    Required,
    NoInternet,
    Disabled,
    Unavailable,
    StaleRequest,
    MarkedOnly,
    MarkedUnavailable,
    ProviderRefused,
}

impl Case {
    fn has_mark(self) -> bool {
        matches!(
            self,
            Self::Required | Self::MarkedOnly | Self::MarkedUnavailable
        )
    }

    fn question_only(self) -> bool {
        matches!(self, Self::MarkedOnly | Self::MarkedUnavailable)
    }
}

type ModelFrames = Vec<(Vec<String>, Vec<String>)>;

struct ReaderModel {
    parts: Vec<String>,
    images: Vec<String>,
    calls: Rc<RefCell<ModelFrames>>,
    question: &'static str,
    intent: Value,
    case: Case,
}

impl crate::LLMEngine for ReaderModel {
    fn add_text_content(&mut self, text: &str) {
        self.parts.push(text.to_owned());
    }

    fn add_image_content(&mut self, image: &str) {
        self.images.push(image.to_owned());
    }

    fn clear_content(&mut self) {
        self.parts.clear();
        self.images.clear();
    }

    fn guarded_timeout(&self) -> Option<Duration> {
        Some(Duration::from_secs(2))
    }

    fn execute_with_handoff(
        &mut self,
        progress: &mut dyn FnMut() -> Result<()>,
        handoff: &mut crate::llm::ModelHandoff<'_>,
    ) -> Result<String> {
        progress()?;
        let mut result = None;
        handoff(&mut || {
            result = Some(self.execute());
            Ok(())
        })?;
        result.context("Notebook Reader did not dispatch")?
    }

    fn execute(&mut self) -> Result<String> {
        let mut calls = self.calls.borrow_mut();
        calls.push((self.parts.clone(), self.images.clone()));
        if self
            .parts
            .first()
            .is_some_and(|part| part.starts_with("Answer the Reader request using"))
        {
            return Ok("Typed continuation answer.".into());
        }
        let legacy_analysis = self.parts.first().is_some_and(|part| {
            part.contains("QUESTION_BOX:") && !part.contains("REQUEST_WEB_JSON")
        });
        let legacy_verification = self
            .parts
            .first()
            .is_some_and(|part| part.contains("return one line: TRANSCRIPTION:"));
        Ok(match calls.len() {
            1 if legacy_analysis => format!(
                "QUESTION: {}\nQUESTION_BOX: 130,35,200,50\nSELECTION_CENTER: 385,230\nSELECTION_SUBJECT: compass\n---\nANSWER: {NOTEBOOK}",
                self.question
            ),
            2 if legacy_verification => format!("TRANSCRIPTION: {}", self.question),
            1 if self.case == Case::MarkedOnly => {
                assert!(self.parts[0].contains("HANDWRITTEN_QUESTION_MARK:"));
                format!(
                    "QUESTION: {}\nQUESTION_BOX: 130,35,200,50\nHANDWRITTEN_QUESTION_MARK: present\n---\nANSWER: NONE",
                    self.question
                )
            }
            1 => {
                let mark = if self.case.has_mark() {
                    assert!(self.parts[0].contains("HANDWRITTEN_QUESTION_MARK:"));
                    "\nHANDWRITTEN_QUESTION_MARK: present"
                } else if self.parts[0].contains("HANDWRITTEN_QUESTION_MARK:") {
                    "\nHANDWRITTEN_QUESTION_MARK: absent"
                } else {
                    ""
                };
                format!(
                    "QUESTION: {}\nQUESTION_BOX: 130,35,200,50\nSELECTION_CENTER: 385,230\nSELECTION_SUBJECT: compass\nREQUEST_WEB_JSON: {}{mark}\n---\nANSWER: Deferred",
                    self.question, self.intent
                )
            }
            2 => json!({"version":1, "transcription":self.question, "request_web":self.intent}).to_string(),
            3 if self.case == Case::ProviderRefused => {
                return Err(crate::llm::ProviderRefused.into());
            }
            3 if self.case == Case::Required => json!({
                "status":"answered", "text":format!("{NOTEBOOK} [R1]"), "citations":["R1"]
            }).to_string(),
            3 => json!({"status":"answered", "text":NOTEBOOK, "citations":[]}).to_string(),
            _ => anyhow::bail!("Unexpected Reader request"),
        })
    }
}

// The real provider transport talks only to this bounded in-process fixture.
fn research_peer() -> Result<(String, std::thread::JoinHandle<Result<Value>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let endpoint = format!("http://{}/v1/responses", listener.local_addr()?);
    let worker = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    anyhow::ensure!(Instant::now() < deadline, "Fixture search never connected");
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => return Err(error.into()),
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.set_write_timeout(Some(Duration::from_secs(3)))?;
        let mut reader = BufReader::new(&mut stream);
        let mut header = String::new();
        reader.read_line(&mut header)?;
        assert_eq!(header.trim_end(), "POST /v1/responses HTTP/1.1");
        let mut size = None;
        loop {
            header.clear();
            anyhow::ensure!(
                reader.read_line(&mut header)? > 0,
                "Missing fixture headers"
            );
            if header == "\r\n" {
                break;
            }
            if let Some((_, value)) = header
                .split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            {
                size = Some(value.trim().parse::<usize>()?);
            }
        }
        let size = size
            .filter(|n| *n <= 512 * 1024)
            .context("Invalid fixture request size")?;
        let mut body = vec![0; size];
        reader.read_exact(&mut body)?;
        let request = serde_json::from_slice(&body)?;
        let reply = json!({"status":"completed","error":null,"incomplete_details":null,
            "output":[{"type":"web_search_call","status":"completed","action":{
                "type":"search","queries":["compass use"],"sources":[
                    {"type":"url","url":"https://example.invalid/compass"}
                ]}},
                {"type":"message","status":"completed","role":"assistant","content":[
                    {"type":"output_text","text":json!({"sources":[
                        {"url":"https://example.invalid/compass","title":"Fixture compass",
                        "excerpt":"Provider fixture summary"}
                    ]}).to_string()}
                ]}
            ]
        })
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            reply.len()
        )?;
        stream.write_all(reply.as_bytes())?;
        Ok(request)
    });
    Ok((endpoint, worker))
}

#[test]
fn installed_reader_uses_only_its_notebook_and_global_memory_after_transcription() -> Result<()> {
    for case in [
        Case::Required,
        Case::NoInternet,
        Case::Disabled,
        Case::Unavailable,
        Case::StaleRequest,
    ] {
        check_captured_page(case)
            .with_context(|| format!("Captured notebook Reader case {case:?}"))?;
    }
    check_bound_reader_page(BoundStart::Original)
        .context("Reader on an adjacent and current bound page")?;
    check_bound_reader_page(BoundStart::Legacy).context("Reader with a legacy original capture")?;
    check_bound_reader_page(BoundStart::Blank).context("Reader after blank start")
}

#[test]
fn installed_marked_notebook_question_is_saved_and_shown_without_an_answer() -> Result<()> {
    for case in [Case::MarkedOnly, Case::MarkedUnavailable] {
        check_captured_page(case)
            .with_context(|| format!("Installed marked notebook Reader case {case:?}"))?;
    }
    Ok(())
}

#[test]
fn installed_provider_failure_is_shown_only_on_the_existing_buddy_page() -> Result<()> {
    check_bound_reader_page(BoundStart::FailureOnBuddy)?;
    check_bound_reader_page(BoundStart::FailureOnOriginal)?;
    check_bound_reader_page(BoundStart::FailureWithUnconfirmedTarget)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BoundStart {
    Original,
    Legacy,
    Blank,
    FailureOnBuddy,
    FailureOnOriginal,
    FailureWithUnconfirmedTarget,
}

pub(super) fn bind_existing_reader_page(
    ledger: &Ledger,
    runtime: &RuntimeIdentity,
    conversation: ConversationId,
    document: ConversationId,
    original: ConversationId,
    buddy: ConversationId,
) -> Result<()> {
    use crate::conversation::{
        BindingAcquisition, BindingReceipt, ExpectedHeads, SourceObservation,
    };
    let hex = |bytes: [u8; 32]| {
        bytes
            .into_iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    };
    ledger.create(conversation, ConversationId::new_v4(), 1)?;
    let operation = ConversationId::new_v4();
    ledger.bind(
        operation,
        ledger.expected(Namespace::Conversation, conversation)?,
        ExpectedHeads::default(),
        BindingReceipt {
            operation,
            conversation,
            request_fingerprint: "bound-reader-research".into(),
            adapter_fingerprint: hex(runtime.adapter_sha256),
            source: SourceObservation {
                document,
                page: original,
                session: "fixture-session".into(),
                visit: "fixture-visit".into(),
                content_revision: None,
                order_revision: None,
                capability_revision: hex(runtime.profile_sha256),
                evidence_procedure: "fixture-research-pages".into(),
            },
            intended_target: Some(buddy),
            observed_document: document,
            observed_page: buddy,
            expected_order: vec![original, buddy],
            observed_order: vec![original, buddy],
            persisted_revision: "fixture-binding".into(),
            observed_ms: 3,
            evidence_origin: "installed-native-guarded-v1".into(),
            acquisition: BindingAcquisition::ExistingAdjacent,
        },
    )?;
    Ok(())
}

fn check_bound_reader_page(start: BoundStart) -> Result<()> {
    let mode = "reconnect-source-handoff-research-pages";
    let blank_start = start == BoundStart::Blank;
    let provider_failure = matches!(
        start,
        BoundStart::FailureOnBuddy
            | BoundStart::FailureOnOriginal
            | BoundStart::FailureWithUnconfirmedTarget
    );
    let rounds = if provider_failure {
        1
    } else if blank_start {
        2
    } else {
        3
    };
    let releases = (0..rounds).map(|_| released(Instant::now())).collect();
    let (mut workflow, store, base, events, _notify) = workflow(mode, releases, Duration::ZERO);
    let document = ConversationId::new_v4();
    let original = ConversationId::new_v4();
    let buddy = ConversationId::new_v4();
    let conversation = ConversationId::new_v4();
    let native = Fixture::new();
    native.declare_source(document, original);
    native.declare_research_successor(buddy);
    let position = native.successor_position();
    if blank_start
        || matches!(
            start,
            BoundStart::FailureOnBuddy | BoundStart::FailureWithUnconfirmedTarget
        )
    {
        std::fs::write(&position, b"buddy")?;
    }
    let runtime = native.start_with_store(store.clone(), mode, Duration::from_secs(10))?;
    let ledger = Ledger::new(store.clone());
    bind_existing_reader_page(
        &ledger,
        runtime.identity(),
        conversation,
        document,
        original,
        buddy,
    )?;
    let owner = workflow.resident_source.as_mut().unwrap();
    let rendered = Rc::new(RefCell::new(vec![]));
    owner.selection = if provider_failure {
        use crate::workflow::selected_backend::RefusingBackend;

        workflow.device = Box::new(
            RefusingBackend::with_resident_releases(vec![released(Instant::now())])
                .with_test_output(Box::new(RecognitionProbe {
                    confirmed_source: start == BoundStart::FailureOnOriginal,
                    successor: (start == BoundStart::FailureOnOriginal).then(|| SuccessorProbe {
                        position: position.clone(),
                        interrupt_restore: false,
                    }),
                    rendered_text: (start != BoundStart::FailureWithUnconfirmedTarget)
                        .then(|| rendered.clone()),
                    ..RecognitionProbe::default()
                })),
        );
        InstalledReaderSelection::fixture_for_installed_output(
            &base.join("output-profile"),
            runtime.identity(),
        )?
    } else {
        InstalledReaderSelection::fixture_with_installed_output()
    };
    owner.session = Some(Box::new(OwnedSession {
        runtime,
        _fixture: native,
        events: events.clone(),
        delivery_fault: false,
        observe_file_change: None,
        writer_slot: None,
        writer_slot_fault: false,
        refinement_slot: None,
    }));

    let mut config = crate::config::Config {
        paths: store.paths.clone(),
        research_policy: Some(Some(ResearchPolicy::Disabled)),
        ..Default::default()
    };
    let profile = ConversationId::new_v4();
    seed_memory(&store, profile, document)?;
    config.memory.enabled = true;
    config.memory.profile = Some(profile);
    let calls = Rc::new(RefCell::new(vec![]));
    let model = ReaderModel {
        parts: vec![],
        images: vec![],
        calls: calls.clone(),
        question: QUESTION,
        intent: json!({"version":1, "intent":"no-restriction"}),
        case: if provider_failure {
            Case::ProviderRefused
        } else {
            Case::Disabled
        },
    };
    let dispatch = crate::workflow::conversation_dispatch::Dispatch::local(&config, &ledger)?;
    let mut reader = Orchestrator::new(workflow, model, Ledger::new(store.clone()));
    if provider_failure {
        let error = reader
            .run_iteration_with_trigger(true, Some(&dispatch), Some(&config))
            .expect_err("The scripted answer provider refused");
        assert!(error.is::<crate::llm::ProviderRefused>(), "{error:#}");
        assert_eq!(calls.borrow().len(), 3);
        assert!(!events.borrow().contains(&"insert"));
        if start == BoundStart::FailureWithUnconfirmedTarget {
            assert!(rendered.borrow().is_empty());
        } else {
            assert_eq!(
                rendered.borrow().as_slice(),
                [format!(
                    "Q: {QUESTION}\n\nA: The response service could not complete this request.\n"
                )]
            );
        }
        let records = ledger.inspect(conversation, false)?;
        let fact = records
            .iter()
            .find_map(|record| match record {
                Record::OutcomeFact(fact) => Some(fact),
                _ => None,
            })
            .context("Provider failure fact absent")?;
        assert!(records
            .iter()
            .any(|record| matches!(record, Record::Turn(turn)
            if turn.id == fact.turn && turn.outcome == crate::conversation::Outcome::Failed
                && turn.text.as_deref() == Some(QUESTION))));
        assert!(!records
            .iter()
            .any(|record| matches!(record, Record::Turn(turn)
            if turn.role == crate::conversation::Role::Assistant)));
        assert_eq!(
            crate::conversation::feedback::describe_stored(&ledger, conversation, fact.id)?.code,
            "RMB-RESPONSE-UNAVAILABLE",
        );
        drop((dispatch, reader, ledger, store));
        std::fs::remove_dir_all(base)?;
        return Ok(());
    }
    let mut original_images = Vec::new();
    let mut origin_turn = None;
    for round in 0..rounds {
        if round == 0 && start == BoundStart::Legacy {
            reader.run_iteration_with_trigger(true, None, None)?;
            let frames = std::mem::take(&mut *calls.borrow_mut());
            let [analysis, verification] = frames.as_slice() else {
                anyhow::bail!("Expected two legacy Reader readings");
            };
            assert_eq!(analysis.1, verification.1);
            original_images = verification.1.clone();
            let records = ledger.inspect(conversation, false)?;
            assert!(!records
                .iter()
                .any(|record| matches!(record, Record::RequestEvidence(_))));
            let capture = records
                .iter()
                .find_map(|record| match record {
                    Record::ResidentCapture(capture) if capture.before.page == original => {
                        Some(capture)
                    }
                    _ => None,
                })
                .context("Legacy Reader original capture not saved")?;
            origin_turn = Some(json!(capture.turn));
            std::fs::write(&position, b"buddy")?;
            continue;
        }
        reader.run_iteration_with_trigger(true, Some(&dispatch), Some(&config))?;
        let frames = std::mem::take(&mut *calls.borrow_mut());
        let [analysis, verification, answer] = frames.as_slice() else {
            anyhow::bail!("Expected two readings and an answer for round {round}");
        };
        assert_eq!(analysis.1, verification.1);
        let payload: Value = serde_json::from_str(&answer.0[1])?;
        let request = &payload["context"]["request_context"];
        assert_eq!(request["request"]["conversation"], json!(conversation));
        assert_eq!(request["request"]["text"], QUESTION);
        let memory = &payload["context"]["memory"];
        if blank_start {
            assert_eq!(request["source"]["kind"], "no-document");
            assert!(answer.1.is_empty());
            assert!(memory["query"]["document"].is_null());
            assert_eq!(memory["context"]["facts"].as_array().unwrap().len(), 1);
            assert_eq!(memory["context"]["facts"][0]["fact"]["value"], GLOBAL);
        } else {
            assert_eq!(request["source"]["kind"], "captured-page");
            assert_eq!(
                request["source"]["source_record"]["record"]["before"]["page"],
                json!(original)
            );
            assert_eq!(memory["query"]["document"], json!(document));
            if round == 0 {
                assert_eq!(request["source"]["origin_turn"], request["request"]["id"]);
                original_images = answer.1.clone();
                assert!(!original_images.is_empty());
                origin_turn = Some(request["request"]["id"].clone());
                std::fs::write(&position, b"buddy")?;
            } else {
                assert_eq!(
                    Some(&request["source"]["origin_turn"]),
                    origin_turn.as_ref()
                );
                assert_ne!(request["request"]["id"], request["source"]["origin_turn"]);
                assert_ne!(
                    analysis.1, original_images,
                    "Buddy and original must use different pixels"
                );
                assert_eq!(answer.1, original_images);
            }
        }
    }
    // Model the typed receiving boundary with a verified owner observation.
    // The native keyboard event path is covered separately.
    let crate::conversation::HistoricalPageAssociationInspection::HistoricalCandidate(binding) =
        ledger.inspect_page_association(conversation, document, buddy)?
    else {
        anyhow::bail!("Expected the existing Buddy binding");
    };
    let mut visit = binding.receipt().source.clone();
    visit.page = buddy;
    visit.content_revision = Some("current-typed-fixture".into());
    let scope = crate::workflow::conversation_dispatch::RequestScope {
        conversation,
        operation: ConversationId::new_v4(),
        root: ledger.expected(Namespace::Conversation, conversation)?,
        source: visit.clone(),
    };
    let mut typed_dispatch =
        crate::workflow::conversation_dispatch::Dispatch::local(&config, &ledger)?;
    let mut typed_model = ReaderModel {
        parts: vec![],
        images: vec![],
        calls: calls.clone(),
        question: QUESTION,
        intent: json!({"version":1,"intent":"no-restriction"}),
        case: Case::Disabled,
    };
    let typed = "Clarify that in one line.";
    typed_dispatch.reader(&ledger, &mut typed_model, typed, None, &scope, &mut || {
        Ok(visit.clone())
    })?;
    {
        let frames = calls.borrow();
        let [(parts, images)] = frames.as_slice() else {
            anyhow::bail!("Expected one typed Reader request");
        };
        let frame: Value = serde_json::from_str(&parts[1])?;
        assert_eq!(frame["request"], typed);
        assert!(frame["history"]
            .as_array()
            .is_some_and(|turns| turns.iter().any(|turn| { turn["text"] == QUESTION })));
        assert_eq!(frame["archived_sources"], json!([]));
        assert_eq!(frame["normalized_text"], json!([]));
        assert_eq!(images, &original_images);
    }

    let records = ledger.inspect(conversation, false)?;
    assert_eq!(
        records
            .iter()
            .filter(|record| matches!(record,
        Record::ResidentCapture(capture) if capture.before.page == buddy))
            .count(),
        2
    );
    drop((dispatch, reader, ledger, store));
    std::fs::remove_dir_all(base)?;
    Ok(())
}

pub(super) fn check_ordinary_reader_with_archived_notebook(
    store: &Arc<Store>,
    root: &Path,
    templates: &Path,
    document: ConversationId,
    pages: &[ConversationId],
) -> Result<()> {
    // The preceding installed Writer request produced real retained notebook
    // normalization, including text from an original other than the current page.
    let archives = store.snapshot_heads_matching(&[Namespace::Source], 8, |head| {
        head.payload["record_kind"].as_str() == Some("archived-normalized-source")
    })?;
    let [archive] = archives.as_slice() else {
        anyhow::bail!("Expected one retained notebook archive before Reader");
    };
    let Record::ArchivedNormalizedSource(archive) = Ledger::decode(archive)? else {
        anyhow::bail!("Expected a retained notebook source");
    };
    assert_eq!(archive.document, document);
    assert!(archive
        .load(store)?
        .document()
        .units
        .iter()
        .any(|unit| { unit.page == Some(2) && unit.text.contains("Last page.") }));

    let mode = "reconnect-source-handoff";
    let (mut workflow, events, _) =
        workflow_with_store(mode, vec![released(Instant::now())], Duration::ZERO, store);
    let native = Fixture::new();
    native.declare_source(document, pages[0]);
    native.declare_source_order(pages);
    let runtime = native.start_with_store(store.clone(), mode, Duration::from_secs(10))?;
    let owner = workflow.resident_source.as_mut().unwrap();
    owner.selection = InstalledReaderSelection::fixture_with_installed_output();
    owner.session = Some(Box::new(OwnedSession {
        runtime,
        _fixture: native,
        events,
        delivery_fault: false,
        observe_file_change: None,
        writer_slot: None,
        writer_slot_fault: false,
        refinement_slot: None,
    }));
    let config = crate::config::Config {
        paths: store.paths.clone(),
        research_policy: Some(Some(ResearchPolicy::Disabled)),
        ..Default::default()
    };
    let ledger = Ledger::new(store.clone());
    let dispatch = crate::workflow::conversation_dispatch::Dispatch::local(&config, &ledger)?;
    let calls = Rc::new(RefCell::new(vec![]));
    let model = ReaderModel {
        parts: vec![],
        images: vec![],
        calls: calls.clone(),
        question: "What does the last page say?",
        intent: json!({"version":1,"intent":"no-restriction"}),
        case: Case::Disabled,
    };
    let mut reader = Orchestrator::new(workflow, model, Ledger::new(store.clone()));
    super::original_pdf::saved_notebook::with_fixture_paths(root, templates, || {
        reader.run_iteration_with_trigger(true, Some(&dispatch), Some(&config))
    })?;

    let calls = calls.borrow();
    let [_, verification, answer] = calls.as_slice() else {
        anyhow::bail!("Expected only two readings and the captured-page answer");
    };
    let frame: Value = serde_json::from_str(&answer.0[1])?;
    let request = &frame["context"]["request_context"];
    assert_eq!(request["source"]["kind"], "captured-page");
    assert_eq!(
        request["source"]["source_record"]["record"]["before"]["page"],
        json!(pages[0])
    );
    assert!(request["retrieval"].is_null());
    assert_eq!(answer.1, verification.1);
    assert!(answer.0.iter().all(|part| !part.contains("Last page.")));
    assert!(answer
        .0
        .iter()
        .all(|part| !part.contains(&archive.id.to_string())));
    Ok(())
}

fn check_captured_page(case: Case) -> Result<()> {
    use crate::conversation::Role;
    let mode = if case.question_only() {
        "reconnect-source-handoff-writer-insert"
    } else {
        "reconnect-source-handoff"
    };
    let (mut workflow, store, base, events, _notify) =
        workflow(mode, vec![released(Instant::now())], Duration::ZERO);
    let document = ConversationId::new_v4();
    let page = ConversationId::new_v4();
    let native = Fixture::new();
    native.declare_source(document, page);
    let runtime = native.start_with_store(store.clone(), mode, Duration::from_secs(10))?;
    let owner = workflow.resident_source.as_mut().unwrap();
    owner.selection = if case.question_only() {
        InstalledReaderSelection::fixture_for_installed_output(
            &base.join("output-profile"),
            runtime.identity(),
        )?
    } else {
        InstalledReaderSelection::fixture_with_installed_output()
    };
    owner.session = Some(Box::new(OwnedSession {
        runtime,
        _fixture: native,
        events: events.clone(),
        delivery_fault: false,
        observe_file_change: None,
        writer_slot: None,
        writer_slot_fault: false,
        refinement_slot: None,
    }));

    let rendered_text = Rc::new(RefCell::new(vec![]));
    if case.question_only() {
        use crate::workflow::selected_backend::RefusingBackend;
        workflow.device = Box::new(
            RefusingBackend::with_resident_releases(vec![released(Instant::now())])
                .with_test_output(Box::new(RecognitionProbe {
                    confirmed_source: true,
                    insertion_source: Some(page),
                    rendered_text: Some(rendered_text.clone()),
                    ..RecognitionProbe::default()
                })),
        );
    }
    let profile = ConversationId::new_v4();
    seed_memory(&store, profile, document)?;
    let mut config = crate::config::Config {
        paths: store.paths.clone(),
        ..Default::default()
    };
    config.memory.enabled = true;
    config.memory.profile = Some(profile);
    if case == Case::Disabled {
        config.research_policy = Some(Some(ResearchPolicy::Disabled));
    }
    let peer = if case == Case::Required {
        let name = "fixture_model_key";
        crate::storage::files::atomic(
            &store.paths.credentials.join(name),
            b"SYNTHETIC_FIXTURE_KEY",
        )?;
        config.model_credential = Some(name.into());
        Some(research_peer()?)
    } else {
        None
    };
    let fixture = researched_fixture::Scope::new(peer.as_ref().map(|(url, _)| url.clone()), false);
    if case == Case::StaleRequest {
        fixture.replace_interpreted_pdf_turn();
    }
    let (question, intent) = if case == Case::NoInternet {
        (
            "Which compass should I use? Skip the internet.",
            json!({"version":1, "intent":"no-internet", "quote":"Skip the internet"}),
        )
    } else {
        let question = if case.question_only() {
            MARKED
        } else {
            QUESTION
        };
        (question, json!({"version":1, "intent":"no-restriction"}))
    };
    let calls = Rc::new(RefCell::new(vec![]));
    let model = ReaderModel {
        parts: vec![],
        images: vec![],
        calls: calls.clone(),
        question,
        intent,
        case,
    };
    let ledger = Ledger::new(store.clone());
    let dispatch = crate::workflow::conversation_dispatch::Dispatch::local(&config, &ledger)?;
    let mut reader = Orchestrator::new(workflow, model, Ledger::new(store.clone()));
    let result = reader.run_iteration_with_trigger(true, Some(&dispatch), Some(&config));
    let refused = matches!(
        case,
        Case::Unavailable | Case::MarkedUnavailable | Case::StaleRequest
    );
    if refused {
        let error = result.expect_err("Expected refusal before answering");
        if matches!(case, Case::Unavailable | Case::MarkedUnavailable) {
            assert!(
                error.is::<crate::conversation::receiving::memory::research::ResearchRefusal>(),
                "{case:?}: {error:#}"
            );
        } else {
            assert!(
                error
                    .to_string()
                    .contains("changed before source preparation"),
                "{case:?}: {error:#}"
            );
        }
    } else {
        result?;
    }
    let search = peer
        .map(|(_, worker)| worker.join().expect("search fixture thread panicked"))
        .transpose()?;
    assert_eq!(fixture.constructions(), usize::from(case == Case::Required));
    let calls = calls.borrow();
    let readings = if case == Case::MarkedOnly { 1 } else { 2 };
    assert_eq!(
        calls.len(),
        readings + usize::from(!refused && !case.question_only())
    );
    for (parts, _) in &calls[..readings] {
        assert!(parts.iter().all(|part| ![NOTEBOOK, OTHER, GLOBAL]
            .iter()
            .any(|fact| part.contains(*fact))));
    }
    if readings == 2 {
        assert_eq!(calls[0].1, calls[1].1);
    }
    if let Some((parts, images)) = calls.get(2) {
        assert!(!images.is_empty());
        let answer: Value = serde_json::from_str(&parts[1])?;
        let source = &answer["context"]["request_context"];
        assert_eq!(source["request"]["text"], question);
        assert_eq!(source["source"]["kind"], "captured-page");
        assert!(source["retrieval"].is_null());
        match case {
            Case::NoInternet => {
                assert_eq!(answer["research"]["disabled_reason"], "request-no-internet")
            }
            Case::Disabled => {
                assert_eq!(answer["research"]["disabled_reason"], "configured-disabled")
            }
            Case::Required => assert_eq!(answer["research"]["status"], "complete"),
            _ => unreachable!(),
        }
        assert_eq!(
            answer["context"]["memory"]["query"]["document"],
            json!(document)
        );
        let facts = &answer["context"]["memory"]["context"]["facts"];
        let mut values: Vec<_> = facts
            .as_array()
            .context("Notebook subject memory absent")?
            .iter()
            .filter_map(|item| item["fact"]["value"].as_str())
            .collect();
        values.sort_unstable();
        assert_eq!(values, [GLOBAL, NOTEBOOK]);
    }
    if let Some(search) = search {
        let request = search.to_string();
        assert!(request.contains(question));
        assert!(request.contains(&document.to_string()));
    }

    let ledger = Ledger::new(store.clone());
    let captures = store.snapshot_heads_matching(&[Namespace::Source], 2, |_| true)?;
    let [capture] = captures.as_slice() else {
        anyhow::bail!("Expected the source capture")
    };
    let Record::ResidentCapture(capture) = Ledger::decode(capture)? else {
        anyhow::bail!("Expected resident evidence")
    };
    assert_eq!(capture.correspondence.expected.document, document);
    let turns = ledger.inspect(capture.conversation, false)?;
    assert!(turns.iter().any(|record| matches!(record, Record::Turn(t) if t.role == Role::User && t.text.as_deref() == Some(question))));
    let assistant = turns.iter().find_map(|record| match record {
        Record::Turn(t) if t.role == Role::Assistant => t.text.as_deref(),
        _ => None,
    });
    let marks = turns
        .iter()
        .filter_map(|record| match record {
            Record::Receipt(receipt) => receipt.marked_question.as_deref(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(marks.len(), usize::from(case.has_mark()));
    if let Some(mark) = marks.first() {
        let mark = serde_json::to_value(mark)?;
        assert_eq!(mark["capture"]["record"], json!(capture.id));
        assert_eq!(mark["interpreted_turn"]["record"], json!(capture.turn));
        assert_eq!(mark["source"]["document"], json!(document));
        assert_eq!(mark["source"]["page"], json!(page));
        assert_eq!(mark["region"], json!([130, 35, 200, 50]));
        assert_eq!(mark["overview"]["sha256"], capture.images[0].media.sha256);
    }

    if case.question_only() {
        assert!(assistant.is_none());
        assert_eq!(*rendered_text.borrow(), [format!("Q: {MARKED}\n")]);
        assert_eq!(
            events
                .borrow()
                .iter()
                .filter(|event| **event == "insert")
                .count(),
            1
        );
    } else if refused {
        assert!(assistant.is_none());
        assert!(!events.borrow().contains(&"insert"));
        if case == Case::Unavailable {
            assert!(turns.iter().any(|record| matches!(record, Record::Turn(t)
                if t.role == Role::User && t.outcome == Outcome::Failed && t.text.as_deref() == Some(question))));
        }
    } else {
        let answer = assistant.context("Expected the stored answer")?;
        assert!(answer.starts_with(NOTEBOOK));
        assert_eq!(
            answer.contains("https://example.invalid/compass"),
            case == Case::Required
        );
    }
    drop((ledger, reader, store));
    std::fs::remove_dir_all(base)?;
    Ok(())
}

fn seed_memory(
    store: &Arc<Store>,
    profile: ConversationId,
    document: ConversationId,
) -> Result<()> {
    let memory = Arc::new(memory::LocalMemory::new_local(store.clone(), profile)?);
    for (scope, claim, value) in [
        (
            memory::Scope::Document { document },
            "compass choice",
            NOTEBOOK,
        ),
        (
            memory::Scope::Document {
                document: ConversationId::new_v4(),
            },
            "compass choice",
            OTHER,
        ),
        (memory::Scope::Global, "compass check", GLOBAL),
    ] {
        memory.put(memory::Mutation {
            operation: ConversationId::new_v4(),
            expected: Default::default(),
            fact: memory::Fact {
                key: memory::Key {
                    profile,
                    scope,
                    fact: ConversationId::new_v4(),
                },
                claim: claim.into(),
                subject: "compass".into(),
                value: value.into(),
                tags: vec![],
                provenance: memory::Provenance {
                    basis: memory::Basis::UserConfirmed,
                    description: "Synthetic notebook choice".into(),
                    conversation: None,
                    turn: None,
                    source: None,
                },
                confidence_basis_points: 1000,
                observed_ms: 1,
                expires_ms: None,
            },
        })?;
    }
    Ok(())
}

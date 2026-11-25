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
};

const QUESTION: &str = "Which compass should I use?";
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
        Ok(match calls.len() {
            1 => format!(
                "QUESTION: {}\nQUESTION_BOX: 130,35,200,50\nSELECTION_CENTER: 385,230\nSELECTION_SUBJECT: compass\nREQUEST_WEB_JSON: {}\n---\nANSWER: Deferred",
                self.question, self.intent
            ),
            2 => json!({"version":1, "transcription":self.question, "request_web":self.intent}).to_string(),
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
    Ok(())
}

fn check_captured_page(case: Case) -> Result<()> {
    use crate::conversation::Role;
    let mode = "reconnect-source-handoff";
    let (mut workflow, store, base, events, _notify) =
        workflow(mode, vec![released(Instant::now())], Duration::ZERO);
    let document = ConversationId::new_v4();
    let owner = workflow.resident_source.as_mut().unwrap();
    owner.selection = InstalledReaderSelection::fixture_with_installed_output();
    owner.factory = Box::new(OwnedFactory {
        source: Some((document, ConversationId::new_v4())),
        events,
        mode,
        delay: Duration::ZERO,
    });

    let profile = ConversationId::new_v4();
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
    drop(memory);
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
        (QUESTION, json!({"version":1, "intent":"no-restriction"}))
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
    let search = peer
        .map(|(_, worker)| worker.join().expect("search fixture thread panicked"))
        .transpose();
    let refused = matches!(case, Case::Unavailable | Case::StaleRequest);
    assert_eq!(fixture.constructions(), usize::from(case == Case::Required));
    if refused {
        let error = result.expect_err("Expected refusal before answering");
        if case == Case::Unavailable {
            assert!(error.is::<crate::conversation::receiving::memory::research::ResearchRefusal>());
        } else {
            assert!(
                error
                    .to_string()
                    .contains("changed before source preparation"),
                "{error:#}"
            );
        }
    } else {
        result?;
    }
    let calls = calls.borrow();
    assert_eq!(calls.len(), if refused { 2 } else { 3 });
    for (parts, _) in &calls[..2] {
        assert!(parts.iter().all(|part| ![NOTEBOOK, OTHER, GLOBAL]
            .iter()
            .any(|fact| part.contains(*fact))));
    }
    assert_eq!(calls[0].1, calls[1].1);
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
    if let Some(search) = search? {
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
    let generated = turns.iter().find_map(|record| match record {
        Record::Turn(t) if t.role == Role::Assistant && t.outcome == Outcome::Generated => {
            t.text.as_deref()
        }
        _ => None,
    });
    if refused {
        assert!(generated.is_none());
        if case == Case::Unavailable {
            assert!(turns.iter().any(|record| matches!(record, Record::Turn(t)
                if t.role == Role::User && t.outcome == Outcome::Failed && t.text.as_deref() == Some(question))));
        }
    } else {
        let answer = generated.context("Expected the generated answer")?;
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

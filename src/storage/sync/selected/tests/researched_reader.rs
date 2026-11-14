//! Composed host evidence, driven with the exact retained normal binary.
//! These cases are mandatory in the hosted composed-reader step, not provider
//! or installed/tablet/model-fidelity qualification.
#![cfg(all(target_os = "linux", feature = "development-resident-bootstrap"))]
use super::corrections::CorrectionFixture;
use super::*;
use crate::{
    conversation::{
        fixture_defer_pending_for_worker, resident_capture::tests::capture_with_pixels,
        selected_reader_prepare::ResidentPrepared, Ledger, Outcome, Record,
        SelectedDomainProjection, SelectedReaderText,
    },
    llm::ModelHandoff,
    storage::selection::SelectedReadBudget,
    workflow::{
        orchestrator::{researched_fixture, Orchestrator},
        Workflow,
    },
    LLMEngine,
};
use anyhow::ensure;
use lopdf::{
    content::{Content, Operation},
    dictionary, Document, Object, Stream,
};
use remarkable_open_sdk::{
    native_adapter::{HistoricalCapture, ReaderFacts},
    PageKey, Uuid as SdkUuid,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        Mutex,
    },
};

const QUESTION: &str = "What is alpha acceleration?";
const OPT_OUT: &str = "What is alpha acceleration? Don't use the internet.";
const QUOTED: &str =
    "What does the quoted phrase \"Don't use the internet\" mean for alpha acceleration?";
const MEMORY: &str = "ORIGINAL_ALPHA_MEMORY_CONVENTION";
type Events = Arc<Mutex<Vec<&'static str>>>;

fn retained_command() -> PathBuf {
    let path = PathBuf::from(
        std::env::var_os("BUDDY_RESEARCH_FIXTURE_COMMAND")
            .expect("mandatory composed-reader driver must supply the retained normal command"),
    );
    let expected = std::env::var("BUDDY_RESEARCH_FIXTURE_COMMAND_SHA256")
        .expect("mandatory composed-reader driver must supply the command digest");
    assert!(path.is_absolute() && crate::storage::valid_digest(&expected));
    let metadata = fs::symlink_metadata(&path).unwrap();
    assert!(metadata.file_type().is_file() && metadata.permissions().mode() & 0o111 != 0);
    let mut file = fs::File::open(&path).unwrap();
    let mut hash = Sha256::new();
    let mut bytes = [0; 65536];
    loop {
        let n = file.read(&mut bytes).unwrap();
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    assert_eq!(
        format!("{:x}", hash.finalize()),
        expected,
        "retained command bytes changed"
    );
    path
}

fn children() -> BTreeSet<u32> {
    fs::read_to_string("/proc/thread-self/children")
        .unwrap()
        .split_whitespace()
        .map(|pid| pid.parse().unwrap())
        .collect()
}

fn pdf_bytes(second: &str) -> Vec<u8> {
    let mut pdf = Document::with_version("1.5");
    let root = pdf.new_object_id();
    let font = pdf.add_object(
        dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
    );
    let pages = ["unrelated first page", second]
        .into_iter()
        .map(|text| {
            let content = Content {
                operations: vec![
                    Operation::new("BT", vec![]),
                    Operation::new("Tf", vec!["F1".into(), 12.into()]),
                    Operation::new("Td", vec![20.into(), 700.into()]),
                    Operation::new("Tj", vec![Object::string_literal(text)]),
                    Operation::new("ET", vec![]),
                ],
            }
            .encode()
            .unwrap();
            let stream = pdf.add_object(Stream::new(dictionary! {}, content));
            pdf.add_object(
                dictionary! {"Type"=>"Page", "Parent"=>root, "Contents"=>stream,
                "MediaBox"=>vec![0.into(),0.into(),600.into(),800.into()]},
            )
        })
        .collect::<Vec<_>>();
    pdf.objects.insert(
        root,
        dictionary! { "Type"=>"Pages", "Count"=>2,
        "Kids"=>pages, "Resources"=>dictionary! {"Font"=>dictionary! {"F1"=>font}} }
        .into(),
    );
    let catalog = pdf.add_object(dictionary! {"Type"=>"Catalog", "Pages"=>root});
    pdf.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    bytes
}

#[derive(Clone, Copy)]
enum MemoryMode {
    Off,
    Empty,
    Document,
    Global,
}

struct Setup {
    f: CorrectionFixture,
    identity: catalog::Identity,
    reader: Arc<SelectedAdmission>,
    scope: SelectionScope,
    path: PathBuf,
    native: PathBuf,
    decoy: Uuid,
    mode: MemoryMode,
    memory_fact: Option<memory::Fact>,
}
impl Setup {
    fn new(mode: MemoryMode, disabled: bool, search: bool) -> Self {
        let f = CorrectionFixture::new();
        let identity = catalog::Identity {
            group: f.binding.group,
            profile: f.owner.profile,
            binding_sha256: f.binding.digest.clone(),
        };
        let owner = catalog::Owner::Document {
            document: f.document,
        };
        let scope = identity.aggregate_scope(&owner).unwrap();
        let mut budget = 32;
        let genesis = f
            .engine(&f.a)
            .refresh_root_catalog(&mut budget)
            .unwrap()
            .unwrap()
            .root_sha;
        // Reuse the established real selected conversation seed without publishing
        // a pending native intent. Registration names its actual original genesis.
        let _ = fixture_defer_pending_for_worker(
            f.a.clone(),
            scope.clone(),
            f.document,
            genesis_base(&genesis, &scope.key_sha256),
        );
        super::recovery::register_document(f.a.clone(), &identity, owner, &genesis);
        let provider = selected_memory::SelectedMemory::new(f.a.clone(), identity.clone()).unwrap();
        let memory_fact = match mode {
            MemoryMode::Document | MemoryMode::Global => {
                let mut value = fact(identity.profile);
                value.claim = "alpha acceleration convention".into();
                value.subject = "alpha acceleration".into();
                value.value = MEMORY.into();
                if matches!(mode, MemoryMode::Document) {
                    value.key.scope = memory::Scope::Document {
                        document: f.document,
                    };
                    provider
                        .put(memory::Mutation {
                            operation: Uuid::new_v4(),
                            expected: BTreeSet::new(),
                            fact: value.clone(),
                        })
                        .unwrap();
                } else {
                    let registration = provider
                        .initialize_aggregate(selected_memory::InitializeAggregate {
                            operation: Uuid::new_v4(),
                            accepted_base_sha256: digest(b"independent alpha fixture memory"),
                            fact: value.clone(),
                        })
                        .unwrap()
                        .registration
                        .unwrap();
                    provider
                        .register_selection(selected_memory::RegisterSelection {
                            operation: Uuid::new_v4(),
                            registration,
                        })
                        .unwrap();
                }
                Some(value)
            }
            _ => None,
        };
        let path = f.storage.0.join("researched-reader.json");
        files::atomic(
            &f.a.paths.credentials.join("fixture_model_key"),
            b"PRIVATE_RESEARCH_FIXTURE",
        )
        .unwrap();
        files::atomic_json(&path, &json!({"paths":f.a.paths,
            "model":"gpt-6.1-terra", "base_url":"https://api.openai.com", "model_credential":"fixture_model_key",
            "reader":{"selection":scope,"researched_pdf":{"research_model":if search {Some("gpt-5.6-terra")} else {None}}},
            "product_settings":{"reader_enabled":true},
            "research_policy":if disabled {"disabled"} else {"required"},
            "memory":{"enabled":!matches!(mode,MemoryMode::Off),"profile":identity.profile,"selection":identity},
            "personalization":{"enabled":true,"text":"Use concise fixture answers."}})).unwrap();
        preference_command::set_fixture_preference(
            f.a.clone(),
            &path,
            f.binding.clone(),
            &f.wire,
            json!({"product_settings":{"reader_enabled":true,"writer_enabled":false}}),
        )
        .unwrap();
        let reader = Arc::new(SelectedAdmission::new(f.a.clone(), scope.clone()).unwrap());
        let mixed =
            f.a.selected_snapshot(&scope, MAX_ITEMS)
                .unwrap()
                .unwrap()
                .transaction
                .selected
                .record_namespaces
                .values()
                .any(|namespace| *namespace == Namespace::SubjectMemory);
        assert_eq!(mixed, matches!(mode, MemoryMode::Document));
        let native = f.storage.0.join("native-source");
        fs::DirBuilder::new().mode(0o700).create(&native).unwrap();
        fs::write(
            native.join(format!("{}.pdf", f.document)),
            pdf_bytes("alpha acceleration original PDF evidence. Printed source text: Don't use the internet."),
        )
        .unwrap();
        let decoy = Uuid::new_v4();
        fs::write(
            native.join(format!("{decoy}.pdf")),
            pdf_bytes("alpha acceleration DECOY_DOCUMENT_MUST_NOT_BE_QUERIED"),
        )
        .unwrap();
        files::atomic_json(&native.join(format!("{decoy}.content")), &json!({"fileType":"pdf",
            "cPages":{"original":{"value":2},"pages":[{"id":Uuid::new_v4(),"idx":{"value":"a"},"redir":{"value":1}}]}})).unwrap();
        Self {
            f,
            identity,
            reader,
            scope,
            path,
            native,
            decoy,
            mode,
            memory_fact,
        }
    }
    fn provider(&self) -> selected_memory::SelectedMemory {
        selected_memory::SelectedMemory::new(self.f.a.clone(), self.identity.clone()).unwrap()
    }
    fn mutate_memory(&self) {
        let provider = self.provider();
        let mut value = self.memory_fact.clone().unwrap();
        let before = self.snapshot().token;
        let expected = provider
            .inspect(&value.key)
            .unwrap()
            .into_iter()
            .map(|e| e.revision_id)
            .collect();
        value.value = "CHANGED_ALPHA_MEMORY_CONVENTION".into();
        provider
            .put(memory::Mutation {
                operation: Uuid::new_v4(),
                expected,
                fact: value,
            })
            .unwrap();
        if matches!(self.mode, MemoryMode::Global) {
            assert_eq!(before, self.snapshot().token);
        }
    }
    fn snapshot(&self) -> crate::storage::selection::SelectedSnapshot {
        self.f
            .a
            .selected_snapshot(&self.scope, MAX_ITEMS)
            .unwrap()
            .unwrap()
    }
    fn records(&self, root: Uuid) -> Vec<Record> {
        SelectedDomainProjection::from_snapshot(&self.snapshot())
            .unwrap()
            .inspect(root)
            .unwrap()
    }
    fn generated(&self, root: Uuid) -> Vec<crate::conversation::Turn> {
        self.records(root)
            .into_iter()
            .filter_map(|r| match r {
                Record::Turn(turn) if turn.outcome == Outcome::Generated => Some(turn),
                _ => None,
            })
            .collect()
    }
    fn mutate_source(&self) {
        fs::OpenOptions::new()
            .append(true)
            .open(self.native.join(format!("{}.pdf", self.f.document)))
            .unwrap()
            .write_all(b"changed source")
            .unwrap();
    }
    fn mutate_preference(&self) {
        let mut config: Value = files::json(&self.path, MAX_METADATA as u64).unwrap();
        config["personalization"]["text"] = json!("changed local preference");
        files::atomic_json(&self.path, &config).unwrap();
    }
    fn run(&self, model: Scripted<'_>) -> Run {
        let before_children = children();
        let command = retained_command();
        let (policy, token) = ReaderPreferences::from_binding(
            self.f.a.clone(),
            self.path.clone(),
            &self.reader,
            Instant::now(),
            self.f.binding.clone(),
            |_| panic!("established selected preferences opened transport"),
        )
        .unwrap();
        // Freeze the non-secret answer destination before any original-PDF read,
        // as installed acquisition does. The protected read remains gated after
        // Prepare and source-pin admission; neither step grants model egress.
        let search = policy
            .config()
            .reader
            .researched_pdf
            .as_ref()
            .and_then(|v| v.research_model.as_deref());
        let selection =
            crate::config::researched_reader::Selection::admit(policy.config(), search).unwrap();
        let pixels = image::RgbaImage::from_pixel(12, 20, image::Rgba([17, 29, 41, 255]));
        let capture = capture_with_pixels(self.f.document, pixels.clone());
        let mut png = Cursor::new(Vec::new());
        pixels.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let png = png.into_inner();
        let original_hash = digest(&png);
        let original_digest = Sha256::digest(&png).into();
        let id = |value: Uuid| SdkUuid::parse(&value.to_string()).unwrap();
        let page = capture.page();
        let facts = ReaderFacts::new(
            PageKey {
                document: id(self.f.document),
                page: id(page),
            },
            vec![id(page)],
        )
        .unwrap();
        let historical = HistoricalCapture::new(
            facts.clone(),
            facts.clone(),
            [12, 20],
            1.0,
            png,
            original_digest,
        )
        .unwrap();
        fs::write(self.native.join(format!("{}.content", self.f.document)), serde_json::to_vec(&json!({"fileType":"pdf",
            "cPages":{"original":{"value":2},"pages":[{"id":page,"idx":{"value":"a"},"redir":{"value":1}}]}})).unwrap()).unwrap();
        let mut pdf = crate::workflow::development_original_pdf(
            &self.native,
            &command,
            self.f.document,
            &facts,
            &historical,
            policy.deadline(),
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(pdf.pin.original_page(), 2);
        let index = pdf.index.key().clone();
        assert_eq!(
            children().difference(&before_children).count(),
            1,
            "actual owned PDF worker must be waiting for its one query"
        );
        let prepared =
            ResidentPrepared::new(&self.f.a, &token, &capture, Uuid::new_v4(), 2).unwrap();
        let (token, acknowledgment) = {
            let mut reads = policy.reads(ReaderPhase::Acquisition).unwrap();
            let [catalog, preferences] = policy.gates();
            SelectedAdmission::with_gates(&[&self.reader, catalog, preferences], |store| {
                policy.verify_locked(store, &self.reader, &token, &mut reads)?;
                policy.require_acquisition_time()?;
                pdf.pin.check(policy.deadline())?;
                prepared.publish_locked(store, &token, &mut reads)
            })
            .unwrap()
        };
        let root = acknowledgment.root.record_id;
        let capture_id = acknowledgment.capture.record_id;
        let models = {
            let mut reads = policy.reads(ReaderPhase::Acquisition).unwrap();
            let [catalog, preferences] = policy.gates();
            SelectedAdmission::with_gates(&[&self.reader, catalog, preferences], |store| {
                policy.verify_locked(store, &self.reader, &token, &mut reads)?;
                pdf.pin.check(policy.deadline())?;
                selection.resolve()
            })
            .unwrap()
        };
        let workflow = Workflow::selected_owned_reader_fixture(self.reader.clone());
        let mut receiver = Orchestrator::new(workflow, model, Ledger::new(self.f.a.clone()));
        receiver.set_personalization(policy.config().personalization_snapshot().unwrap());
        if !matches!(self.mode, MemoryMode::Off) {
            receiver.set_selected_subject_memory(Arc::new(self.provider()));
        }
        let result = receiver.receive_researched_resident(
            token.clone(),
            acknowledgment,
            policy,
            pdf,
            models,
        );
        drop(receiver);
        assert_eq!(
            children(),
            before_children,
            "owned PDF query/refusal must reap its original child"
        );
        let crate::conversation::SelectedCaptureImages::Resident(stored) = self
            .reader
            .capture_images(&self.snapshot().token, capture_id)
            .unwrap()
        else {
            panic!("selected request must retain its resident original");
        };
        assert_eq!(digest(&stored.original_png), original_hash);
        assert!(
            self.f
                .a
                .heads(Namespace::Conversation, root)
                .unwrap()
                .is_empty(),
            "no ordinary conversation publication"
        );
        assert!(self.records(root).iter().all(|r| !matches!(r, Record::Turn(t) if !matches!(t.outcome,Outcome::Prepared|Outcome::Interpreted|Outcome::Generated))));
        Run {
            result,
            root,
            prepared: token,
            index,
        }
    }
}

struct Run {
    result: Result<()>,
    root: Uuid,
    prepared: SelectionToken,
    index: crate::retrieval::IndexKey,
}

#[derive(Clone)]
struct Frame {
    images: Vec<String>,
    texts: Vec<String>,
}
#[derive(Clone, Copy)]
enum Reading {
    Clear,
    Quoted,
    Deny,
    Unclear,
    Disagree,
    Malformed,
    Missing,
    Duplicate,
    MissingAnalysis,
    DuplicateAnalysis,
}
struct Scripted<'a> {
    reading: Reading,
    required: bool,
    has_fact: bool,
    frames: Arc<Mutex<Vec<Frame>>>,
    events: Events,
    content: Frame,
    hook: Option<Box<dyn FnMut(usize, bool) + 'a>>,
}
impl<'a> Scripted<'a> {
    fn new(reading: Reading, required: bool, has_fact: bool, events: Events) -> Self {
        Self {
            reading,
            required,
            has_fact,
            frames: Arc::new(Mutex::new(vec![])),
            events,
            content: Frame {
                images: vec![],
                texts: vec![],
            },
            hook: None,
        }
    }
    fn question(&self) -> &'static str {
        if matches!(self.reading, Reading::Deny | Reading::Disagree) {
            OPT_OUT
        } else if matches!(self.reading, Reading::Quoted) {
            QUOTED
        } else {
            QUESTION
        }
    }
}
impl LLMEngine for Scripted<'_> {
    fn add_text_content(&mut self, text: &str) {
        self.content.texts.push(text.into());
    }
    fn add_image_content(&mut self, image: &str) {
        self.content.images.push(image.into());
    }
    fn clear_content(&mut self) {
        self.content.images.clear();
        self.content.texts.clear();
    }
    fn execute(&mut self) -> Result<String> {
        panic!("researched fixture used unguarded model");
    }
    fn execute_with_deadline(
        &mut self,
        progress: &mut dyn FnMut() -> Result<()>,
        handoff: &mut ModelHandoff<'_>,
        deadline: Instant,
    ) -> Result<String> {
        ensure!(Instant::now() < deadline, "fixture deadline");
        progress()?;
        let stage = self.frames.lock().unwrap().len();
        if let Some(hook) = &mut self.hook {
            hook(stage, true);
        }
        let mut submitted = false;
        handoff(&mut || {
            ensure!(!submitted, "duplicate fixture submit");
            submitted = true;
            self.frames.lock().unwrap().push(self.content.clone());
            self.events.lock().unwrap().push(match stage {
                0 => "analysis",
                1 => "verification",
                2 => "answer",
                _ => panic!("extra model stage"),
            });
            Ok(())
        })?;
        ensure!(submitted, "model returned without actual joined handoff");
        if let Some(hook) = &mut self.hook {
            hook(stage, false);
        }
        let question = self.question();
        let deny = matches!(self.reading, Reading::Deny | Reading::Disagree);
        let intent = if deny {
            json!({"version":1,"intent":"no-internet","quote":"Don't use the internet"})
        } else {
            json!({"version":1,"intent":"no-restriction"})
        };
        Ok(match stage {
            0 => {
                let metadata = match self.reading {
                    Reading::MissingAnalysis => String::new(),
                    Reading::DuplicateAnalysis => {
                        format!("REQUEST_WEB_JSON: {intent}\nREQUEST_WEB_JSON: {intent}\n")
                    }
                    _ => format!("REQUEST_WEB_JSON: {intent}\n"),
                };
                format!("QUESTION: {question}\nQUESTION_BOX: 10,10,100,100\nSELECTION_CENTER: 100,100\n{metadata}---\nANSWER: ANALYSIS_PROPOSAL_MUST_NOT_PUBLISH")
            }
            1 => {
                if matches!(self.reading, Reading::Malformed) {
                    "TRANSCRIPTION: legacy metadata is not admitted".into()
                } else if matches!(self.reading, Reading::Missing) {
                    json!({"version":1,"transcription":question}).to_string()
                } else if matches!(self.reading, Reading::Duplicate) {
                    format!("{{\"version\":1,\"transcription\":{},\"request_web\":{{\"version\":1,\"intent\":\"no-restriction\",\"intent\":\"no-internet\"}}}}",json!(question))
                } else {
                    json!({"version":1,"transcription":question,"request_web":match self.reading {
                    Reading::Unclear=>json!({"version":1,"intent":"unclear"}), Reading::Disagree=>json!({"version":1,"intent":"no-restriction"}), _=>intent }}).to_string()
                }
            }
            2 => {
                let mut citations = vec!["S1"];
                if self.has_fact {
                    citations.push("M1");
                }
                if self.required {
                    citations.push("R1");
                }
                json!({"status":"answered","text":format!("Final original-PDF answer {}",citations.iter().map(|id|format!("[{id}]")).collect::<Vec<_>>().join(" ")),"citations":citations}).to_string()
            }
            _ => panic!("extra researched call"),
        })
    }
}

fn assert_success(
    setup: &Setup,
    run: &Run,
    frames: &[Frame],
    question: &str,
    has_fact: bool,
) -> Value {
    assert!(
        run.result.is_ok(),
        "researched receiver failed: {:?}",
        run.result.as_ref().err()
    );
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0].images.len(), 4);
    assert_eq!(frames[0].images, frames[1].images);
    assert!(frames[2].images.is_empty());
    assert!(
        frames[1]
            .texts
            .iter()
            .all(|text| !text.contains(question)
                && !text.contains("ANALYSIS_PROPOSAL_MUST_NOT_PUBLISH")),
        "verifier must remain independent"
    );
    let context: Value = serde_json::from_str(&frames[2].texts[1]).unwrap();
    let request = &context["context"]["request_context"];
    assert_eq!(request["scope"], "selected-historical");
    assert_eq!(request["request"]["text"], question);
    assert_eq!(request["history"], json!([]));
    assert_eq!(request["retrieval"]["source"], json!(run.index));
    assert_eq!(run.index.document, setup.f.document);
    assert_ne!(run.index.document, setup.decoy);
    let actual_pdf = fs::read(setup.native.join(format!("{}.pdf", setup.f.document))).unwrap();
    assert_eq!(run.index.source_sha256, digest(&actual_pdf));
    assert_eq!(request["retrieval"]["passages"][0]["page"], 2);
    assert!(request["retrieval"]["passages"][0]["text"]
        .as_str()
        .unwrap()
        .contains("alpha acceleration"));
    assert!(!context
        .to_string()
        .contains("DECOY_DOCUMENT_MUST_NOT_BE_QUERIED"));
    let facts = context["context"]["memory"]["context"]["facts"]
        .as_array()
        .map_or(0, Vec::len);
    assert_eq!(facts, usize::from(has_fact));
    if has_fact {
        assert!(context.to_string().contains(MEMORY));
        assert_eq!(context["context"]["memory"]["query"]["text"], question);
    }
    let drafts = setup.generated(run.root);
    assert_eq!(drafts.len(), 1);
    assert!(drafts[0]
        .text
        .as_ref()
        .unwrap()
        .contains("References:\n[S1] Document"));
    assert!(drafts[0].text.as_ref().unwrap().contains("page 2"));
    assert!(drafts[0]
        .text
        .as_ref()
        .unwrap()
        .contains(&format!("[S1] Document {}, page 2", setup.f.document)));
    assert!(!drafts[0]
        .text
        .as_ref()
        .unwrap()
        .contains(&setup.decoy.to_string()));
    assert!(!drafts[0]
        .text
        .as_ref()
        .unwrap()
        .contains("ANALYSIS_PROPOSAL_MUST_NOT_PUBLISH"));
    assert!(setup.records(run.root).iter().any(|r| matches!(r,Record::Turn(t) if t.outcome==Outcome::Interpreted&&t.text.as_deref()==Some(question))));
    context
}

struct SearchPeer {
    endpoint: String,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<Value>>>,
    worker: Option<thread::JoinHandle<()>>,
}
impl SearchPeer {
    fn new(events: Events, mut change: Option<Box<dyn FnMut() + Send>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let ended = stop.clone();
        let requests = Arc::new(Mutex::new(vec![]));
        let captured = requests.clone();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                if ended.load(AtomicOrdering::Relaxed) {
                    break;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        let value = read_search(&mut stream);
                        captured.lock().unwrap().push(value);
                        events.lock().unwrap().push("research");
                        if let Some(change) = &mut change {
                            change();
                        }
                        let body=serde_json::to_vec(&json!({"status":"completed","error":null,"incomplete_details":null,
                        "output":[{"type":"web_search_call","status":"completed","action":{"type":"search","queries":["alpha acceleration"],"sources":[{"type":"url","url":"https://example.invalid/alpha"}]}},
                        {"type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":json!({"sources":[{"url":"https://example.invalid/alpha","title":"Fixture research","excerpt":"Provider fixture summary, unverified"}]}).to_string()}]}]})).unwrap();
                        write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .unwrap();
                        stream.write_all(&body).unwrap();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "fixture listener deadline");
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(e) => panic!("fixture listener: {e}"),
                }
            }
        });
        Self {
            endpoint,
            stop,
            requests,
            worker: Some(worker),
        }
    }
}
impl Drop for SearchPeer {
    fn drop(&mut self) {
        self.stop.store(true, AtomicOrdering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}
fn read_search(stream: &mut TcpStream) -> Value {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut header = vec![];
    while !header.ends_with(b"\r\n\r\n") {
        assert!(header.len() < 16384);
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        header.push(byte[0]);
    }
    let header = String::from_utf8(header).unwrap();
    assert!(header.starts_with("POST /v1/responses HTTP/1.1"));
    let size = header
        .lines()
        .find_map(|line| {
            let (k, v) = line.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().unwrap())
        })
        .unwrap();
    assert!(size <= 1024 * 1024);
    let mut body = vec![0; size];
    stream.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[test]
#[ignore = "requires the retained normal binary; mandatory hosted composed-reader step"]
fn researched_pdf_opt_out_skips_transport() {
    for mode in [
        MemoryMode::Off,
        MemoryMode::Document,
        MemoryMode::Global,
        MemoryMode::Empty,
    ] {
        let setup = Setup::new(mode, false, false);
        let scope = researched_fixture::Scope::new(None, false);
        let events = Events::default();
        let model = Scripted::new(
            Reading::Deny,
            false,
            matches!(mode, MemoryMode::Document | MemoryMode::Global),
            events.clone(),
        );
        let frames = model.frames.clone();
        let run = setup.run(model);
        let context = assert_success(
            &setup,
            &run,
            &frames.lock().unwrap(),
            OPT_OUT,
            matches!(mode, MemoryMode::Document | MemoryMode::Global),
        );
        assert_eq!(
            context["research"]["disabled_reason"],
            "request-no-internet"
        );
        assert_eq!(scope.constructions(), 0);
        if matches!(mode, MemoryMode::Empty) {
            assert_eq!(
                context["context"]["memory"]["status"],
                json!(memory::Status::CompleteEmpty)
            );
        }
        assert_eq!(
            *events.lock().unwrap(),
            vec!["analysis", "verification", "answer"]
        );
    }
}
#[test]
#[ignore = "requires the retained normal binary; mandatory hosted composed-reader step"]
fn researched_pdf_disabled_skips_transport() {
    let setup = Setup::new(MemoryMode::Off, true, false);
    let scope = researched_fixture::Scope::new(None, false);
    let events = Events::default();
    let model = Scripted::new(Reading::Clear, false, false, events.clone());
    let frames = model.frames.clone();
    let run = setup.run(model);
    let context = assert_success(&setup, &run, &frames.lock().unwrap(), QUESTION, false);
    assert_eq!(
        context["research"]["disabled_reason"],
        "configured-disabled"
    );
    assert_eq!(scope.constructions(), 0);
    assert_eq!(
        *events.lock().unwrap(),
        vec!["analysis", "verification", "answer"]
    );
}
#[test]
#[ignore = "requires the retained normal binary; mandatory hosted composed-reader step"]
fn researched_pdf_required_uses_guarded_search() {
    // Both a printed denial in the original PDF and a denial quoted as data in
    // the question remain non-instructions when independent readings agree.
    // Scripted classifications do not qualify real-model handwriting fidelity.
    for reading in [Reading::Clear, Reading::Quoted] {
        let setup = Setup::new(MemoryMode::Document, false, true);
        let events = Events::default();
        let peer = SearchPeer::new(events.clone(), None);
        let scope = researched_fixture::Scope::new(Some(peer.endpoint.clone()), false);
        let model = Scripted::new(reading, true, true, events.clone());
        let question = model.question();
        let frames = model.frames.clone();
        let run = setup.run(model);
        let context = assert_success(&setup, &run, &frames.lock().unwrap(), question, true);
        assert_eq!(scope.constructions(), 1);
        assert_eq!(
            *events.lock().unwrap(),
            vec!["analysis", "verification", "research", "answer"]
        );
        let requests = peer.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["model"], "gpt-5.6-terra");
        assert_eq!(requests[0]["tool_choice"], "required");
        let exact_search = requests[0]["input"][0]["content"][0]["text"]
            .as_str()
            .unwrap();
        let search: Value = serde_json::from_str(exact_search).unwrap();
        assert_eq!(
            context["research"]["identity"]["context_sha256"],
            digest(exact_search.as_bytes())
        );
        assert_eq!(
            context["research"]["identity"]["request_turn"],
            context["context"]["request_context"]["request"]["id"]
        );
        assert_eq!(
            context["research"]["identity"]["conversation"],
            json!(run.root)
        );
        assert_eq!(
            context["research"]["queries"],
            json!(["alpha acceleration"])
        );
        assert_eq!(
            context["research"]["sources"][0]["url"],
            "https://example.invalid/alpha"
        );
        assert_eq!(
            search["request_context"],
            context["context"]["request_context"]
        );
        assert_eq!(search["memory"], context["context"]["memory"]);
        assert!(search.to_string().contains(MEMORY));
        assert!(setup.generated(run.root)[0]
            .text
            .as_ref()
            .unwrap()
            .contains(
            "[R1] Fixture research - https://example.invalid/alpha (provider reported, unverified)"
        ));
    }
    // Search eligibility is intentionally late: it cannot reject independently
    // admitted recognition, but Required cannot construct a transport without
    // the exact qualified profile after fresh Interpret and local retrieval.
    for profile in [None, Some("unqualified-search-model")] {
        let setup = Setup::new(MemoryMode::Off, false, false);
        let mut config: Value = files::json(&setup.path, MAX_METADATA as u64).unwrap();
        config["reader"]["researched_pdf"]["research_model"] = json!(profile);
        files::atomic_json(&setup.path, &config).unwrap();
        let scope = researched_fixture::Scope::new(None, false);
        let events = Events::default();
        let model = Scripted::new(Reading::Clear, true, false, events.clone());
        let frames = model.frames.clone();
        let run = setup.run(model);
        assert!(run.result.is_err());
        assert_eq!(frames.lock().unwrap().len(), 2);
        assert_eq!(*events.lock().unwrap(), vec!["analysis", "verification"]);
        assert_eq!(scope.constructions(), 0);
        assert!(setup.generated(run.root).is_empty());
        assert!(setup.records(run.root).iter().any(|record| matches!(record, Record::Turn(turn) if turn.outcome==Outcome::Interpreted && turn.text.as_deref()==Some(QUESTION))));
        assert!(scope.generation().is_none());
    }
}
#[test]
#[ignore = "requires the retained normal binary; mandatory hosted composed-reader step"]
fn researched_pdf_selected_memory_stays_pinned() {
    for after in [false, true] {
        let setup = Setup::new(MemoryMode::Global, false, false);
        let scope = researched_fixture::Scope::new(None, false);
        let events = Events::default();
        let mut model = Scripted::new(Reading::Deny, false, true, events);
        let frames = model.frames.clone();
        model.hook = Some(Box::new(|stage, before| {
            if stage == 2 && before != after {
                setup.mutate_memory();
            }
        }));
        let run = setup.run(model);
        assert!(run.result.is_err());
        assert!(setup.generated(run.root).is_empty());
        assert_eq!(scope.constructions(), 0);
        assert_eq!(frames.lock().unwrap().len(), if after { 3 } else { 2 });
        assert_eq!(
            setup
                .records(run.root)
                .iter()
                .filter(|r| matches!(r,Record::Turn(t) if t.outcome==Outcome::Interpreted))
                .count(),
            1
        );
    }
    // The view was captured before research. An independent global edit during
    // the real response wait must prevent the next answer handoff.
    let setup = Setup::new(MemoryMode::Global, false, true);
    let provider = setup.provider();
    let original = setup.memory_fact.clone().unwrap();
    let store = setup.f.a.clone();
    let doc = setup.scope.clone();
    let events = Events::default();
    let peer = SearchPeer::new(
        events.clone(),
        Some(Box::new(move || {
            let before = store
                .selected_snapshot(&doc, MAX_ITEMS)
                .unwrap()
                .unwrap()
                .token;
            let expected = provider
                .inspect(&original.key)
                .unwrap()
                .into_iter()
                .map(|e| e.revision_id)
                .collect();
            let mut changed = original.clone();
            changed.value = "CHANGED_DURING_RESEARCH".into();
            provider
                .put(memory::Mutation {
                    operation: Uuid::new_v4(),
                    expected,
                    fact: changed,
                })
                .unwrap();
            assert_eq!(
                store
                    .selected_snapshot(&doc, MAX_ITEMS)
                    .unwrap()
                    .unwrap()
                    .token,
                before
            );
        })),
    );
    let scope = researched_fixture::Scope::new(Some(peer.endpoint.clone()), false);
    let model = Scripted::new(Reading::Clear, true, true, events.clone());
    let frames = model.frames.clone();
    let run = setup.run(model);
    assert!(run.result.is_err());
    assert!(setup.generated(run.root).is_empty());
    assert_eq!(frames.lock().unwrap().len(), 2);
    assert_eq!(scope.constructions(), 1);
    assert_eq!(
        *events.lock().unwrap(),
        vec!["analysis", "verification", "research"]
    );
}
#[test]
#[ignore = "requires the retained normal binary; mandatory hosted composed-reader step"]
fn researched_pdf_unclear_intent_stops_before_interpret() {
    for reading in [
        Reading::Unclear,
        Reading::Disagree,
        Reading::Malformed,
        Reading::Missing,
        Reading::Duplicate,
        Reading::MissingAnalysis,
        Reading::DuplicateAnalysis,
    ] {
        let setup = Setup::new(MemoryMode::Off, false, false);
        let scope = researched_fixture::Scope::new(None, false);
        let model = Scripted::new(reading, false, false, Events::default());
        let frames = model.frames.clone();
        let run = setup.run(model);
        assert!(run.result.is_err());
        assert_eq!(setup.snapshot().token, run.prepared);
        assert!(setup.generated(run.root).is_empty());
        assert!(!setup
            .records(run.root)
            .iter()
            .any(|r| matches!(r,Record::Turn(t) if t.outcome==Outcome::Interpreted)));
        assert_eq!(
            frames.lock().unwrap().len(),
            if matches!(
                reading,
                Reading::MissingAnalysis | Reading::DuplicateAnalysis
            ) {
                1
            } else {
                2
            }
        );
        assert_eq!(scope.constructions(), 0);
        assert!(scope.generation().is_none());
    }
}
#[test]
#[ignore = "requires the retained normal binary; mandatory hosted composed-reader step"]
fn researched_pdf_drift_refuses_handoff_and_generation() {
    for kind in 0..3 {
        for after in [false, true] {
            let setup = Setup::new(MemoryMode::Document, false, false);
            let scope = researched_fixture::Scope::new(None, false);
            let mut model = Scripted::new(Reading::Deny, false, true, Events::default());
            let frames = model.frames.clone();
            model.hook = Some(Box::new(|stage, before| {
                if stage == 2 && before != after {
                    match kind {
                        0 => setup.mutate_source(),
                        1 => setup.mutate_preference(),
                        _ => setup.mutate_memory(),
                    }
                }
            }));
            let run = setup.run(model);
            assert!(run.result.is_err());
            assert!(setup.generated(run.root).is_empty());
            assert_eq!(scope.constructions(), 0);
            assert_eq!(frames.lock().unwrap().len(), if after { 3 } else { 2 });
            assert_eq!(
                setup
                    .records(run.root)
                    .iter()
                    .filter(|r| matches!(r,Record::Turn(t) if t.outcome==Outcome::Interpreted))
                    .count(),
                1
            );
        }
    }
}
#[test]
#[ignore = "requires the retained normal binary; mandatory hosted composed-reader step"]
fn researched_pdf_lost_ack_recovers_original_without_replay() {
    let setup = Setup::new(MemoryMode::Document, false, false);
    let scope = researched_fixture::Scope::new(None, true);
    let events = Events::default();
    let model = Scripted::new(Reading::Deny, false, true, events.clone());
    let frames = model.frames.clone();
    let run = setup.run(model);
    assert!(run.result.is_err());
    assert_eq!(setup.generated(run.root).len(), 1);
    let witness = scope
        .generation()
        .expect("actual frozen Generated request witness");
    assert!(matches!(
        witness.request.kind,
        SelectedReaderText::Generate { .. }
    ));
    assert_eq!(
        setup.generated(run.root)[0].text.as_deref(),
        Some(witness.request.text.as_str())
    );
    let current = setup.snapshot().token;
    let before = events.lock().unwrap().clone();
    // Break fresh source/config admission. Historical receipt recovery must not
    // consult them, mint a new Attempt or replay any of the completed stages.
    setup.mutate_source();
    setup.mutate_preference();
    let mut reads = SelectedReadBudget::new(64, MAX_ITEMS as u64, MAX_METADATA as u64, MAX_MEDIA);
    assert!(setup
        .reader
        .recover_reader_text(&witness.accepted, &witness.request, &mut reads)
        .unwrap());
    let mut changed = witness.request.clone();
    changed.text.push_str("changed original");
    assert!(setup
        .reader
        .recover_reader_text(&witness.accepted, &changed, &mut reads)
        .is_err());
    assert_eq!(setup.snapshot().token, current);
    assert_eq!(setup.generated(run.root).len(), 1);
    assert_eq!(*events.lock().unwrap(), before);
    assert_eq!(frames.lock().unwrap().len(), 3);
    assert_eq!(scope.constructions(), 0);
}

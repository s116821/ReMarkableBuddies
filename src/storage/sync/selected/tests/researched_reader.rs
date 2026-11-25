//! Composed host evidence, driven with the exact retained normal binary.
//! These cases are mandatory in the hosted composed-reader step, not provider
//! or installed/tablet/model-fidelity qualification.
#![cfg(all(target_os = "linux", feature = "development-resident-bootstrap"))]
#[cfg(feature = "installed-resident-output")]
mod ordinary_epub;
use super::corrections::CorrectionFixture;
use super::*;
use crate::{
    conversation::{
        fixture_defer_pending_for_worker, resident_capture::tests::capture_with_pixels,
        selected_reader_prepare::ResidentPrepared, Ledger, Outcome, Record,
        SelectedDomainProjection, SelectedReaderText,
    },
    llm::ModelHandoff,
    storage::{selection::SelectedReadBudget, Manifest, Scope},
    workflow::{
        orchestrator::{researched_fixture, Orchestrator},
        resident_supervisor::health::ServiceHealth,
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
    collections::BTreeMap,
    io::{Cursor, Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    os::unix::{
        fs::{DirBuilderExt, PermissionsExt},
        net::UnixDatagram,
    },
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        mpsc, Mutex,
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
    pdf_pages(["unrelated first page", second])
}

fn pdf_pages<'a>(text_pages: impl IntoIterator<Item = &'a str>) -> Vec<u8> {
    let mut pdf = Document::with_version("1.5");
    let root = pdf.new_object_id();
    let font = pdf.add_object(
        dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" },
    );
    let pages = text_pages
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
            Object::Reference(pdf.add_object(
                dictionary! {"Type"=>"Page", "Parent"=>root, "Contents"=>stream,
                "MediaBox"=>vec![0.into(),0.into(),600.into(),800.into()]},
            ))
        })
        .collect::<Vec<_>>();
    let pages_count = pages.len() as i64;
    pdf.objects.insert(
        root,
        dictionary! { "Type"=>"Pages", "Count"=>pages_count,
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

#[derive(Default)]
struct RunOptions {
    health: Option<ServiceHealth>,
    parent_window: Option<Duration>,
    exhaust_submission: bool,
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
    fn ordinary_manifests(&self) -> BTreeMap<Uuid, Manifest> {
        self.f
            .a
            .manifests()
            .unwrap()
            .into_iter()
            .filter(|manifest| manifest.scope == Scope::LocalTransaction)
            .map(|manifest| (manifest.transaction_id, manifest))
            .collect()
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
        self.run_with(model, RunOptions::default())
    }
    fn run_with(&self, model: Scripted<'_>, options: RunOptions) -> Run {
        let ordinary_before = self.ordinary_manifests();
        let before_children = children();
        let command = retained_command();
        let (mut policy, token) = ReaderPreferences::from_binding(
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
        if let Some(window) = options.parent_window {
            // Shorten the same admitted request only after acquisition setup.
            policy.cap_fixture_windows(window, window);
        }
        if options.exhaust_submission {
            let mut reads = policy.reads(ReaderPhase::Submission).unwrap();
            let error = reads.charge(u64::MAX, 0, 0, 0).unwrap_err();
            assert!(read_budget_exhausted(&error) && reads.exhausted());
        }
        let deadline = policy.deadline();
        let mut workflow = Workflow::selected_owned_reader_fixture(self.reader.clone());
        if let Some(health) = options.health {
            workflow.enable_service_health(health).unwrap();
        }
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
        let returned = Instant::now();
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
        // Ordinary and selected publications share the record-head index.
        // Compare complete ordinary manifests so success and refusal paths
        // cannot silently add or replace an ordinary committed transaction.
        assert_eq!(
            self.ordinary_manifests(),
            ordinary_before,
            "researched Reader must not change ordinary committed storage"
        );
        assert!(self.records(root).iter().all(|r| !matches!(r, Record::Turn(t) if !matches!(t.outcome,Outcome::Prepared|Outcome::Interpreted|Outcome::Generated))));
        Run {
            result,
            root,
            prepared: token,
            index,
            deadline,
            returned,
        }
    }
}

struct Run {
    result: Result<()>,
    root: Uuid,
    prepared: SelectionToken,
    index: crate::retrieval::IndexKey,
    deadline: Instant,
    returned: Instant,
}

fn read_budget_exhausted(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string() == "selected read budget exhausted")
}

fn currentness_lost(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string() == "Runtime health or input lost; further activity stopped")
}

#[derive(Default)]
struct ProgressExhaustion {
    calls: usize,
    budget_cause: bool,
    sticky_budget_cause: bool,
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
    exhaust_answer_progress: Option<Arc<Mutex<ProgressExhaustion>>>,
    currentness_refusal: Option<Arc<AtomicBool>>,
    answer_refused: bool,
    ordinary_pdf_questions: Vec<&'static str>,
    mark_ordinary_pdf_question: bool,
    captured_page_only: bool,
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
            exhaust_answer_progress: None,
            currentness_refusal: None,
            answer_refused: false,
            ordinary_pdf_questions: Vec::new(),
            mark_ordinary_pdf_question: false,
            captured_page_only: false,
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
        let ordinal = self.frames.lock().unwrap().len();
        let (stage, question) = if self.ordinary_pdf_questions.is_empty() {
            (ordinal, self.question())
        } else {
            (ordinal % 3, self.ordinary_pdf_questions[ordinal / 3])
        };
        if let Some(hook) = &mut self.hook {
            hook(stage, true);
        }
        if stage == 2 {
            if let Some(witness) = &self.exhaust_answer_progress {
                // Exercise the callback lent by the real Attempt/Workflow. An
                // over-polling model cannot replace its cumulative allowance.
                for calls in 1..=4096 {
                    if let Err(error) = progress() {
                        let budget_cause = read_budget_exhausted(&error);
                        let sticky_budget_cause = progress()
                            .err()
                            .is_some_and(|again| read_budget_exhausted(&again));
                        *witness.lock().unwrap() = ProgressExhaustion {
                            calls,
                            budget_cause,
                            sticky_budget_cause,
                        };
                        return Err(error);
                    }
                }
                anyhow::bail!("fixture did not exhaust the original progress allowance");
            }
        }
        let mut submitted = false;
        let outcome = handoff(&mut || {
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
        });
        if let Err(error) = outcome {
            if let Some(witness) = &self.currentness_refusal {
                witness.store(currentness_lost(&error), AtomicOrdering::Release);
            }
            return Err(error);
        }
        ensure!(submitted, "model returned without actual joined handoff");
        if let Some(hook) = &mut self.hook {
            hook(stage, false);
        }
        if stage == 2 {
            if let Some(witness) = &self.currentness_refusal {
                // Observe the real progress callback's underlying guard error
                // before the answering consumer sanitizes provider failures.
                if let Err(error) = progress() {
                    witness.store(currentness_lost(&error), AtomicOrdering::Release);
                    return Err(error);
                }
            }
        }
        if stage == 2 && self.answer_refused {
            return Err(crate::llm::ProviderRefused.into());
        }
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
                let mark = if self.mark_ordinary_pdf_question {
                    assert!(self.content.texts[0].contains("HANDWRITTEN_QUESTION_MARK:"));
                    "HANDWRITTEN_QUESTION_MARK: present\n"
                } else {
                    ""
                };
                format!("QUESTION: {question}\nQUESTION_BOX: 10,10,100,100\nSELECTION_CENTER: 100,100\n{metadata}{mark}---\nANSWER: ANALYSIS_PROPOSAL_MUST_NOT_PUBLISH")
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
            2 if self.captured_page_only => {
                json!({"status":"answered","text":"Captured page answer.","citations":[]})
                    .to_string()
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

type ActiveSocket = Arc<Mutex<Option<TcpStream>>>;
struct HeldSearchObservation {
    request: Value,
    request_at: Instant,
    partial_body_at: Option<Instant>,
    closed_at: Instant,
}
struct HeldSearchPeer {
    endpoint: String,
    stop: Arc<AtomicBool>,
    active: ActiveSocket,
    done: mpsc::Receiver<Result<HeldSearchObservation>>,
    worker: Option<thread::JoinHandle<()>>,
}
impl HeldSearchPeer {
    fn new(events: Events, partial_body: bool, change: Option<Box<dyn FnOnce() + Send>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let active = ActiveSocket::default();
        let (sent, done) = mpsc::channel();
        let ended = stop.clone();
        let connection = active.clone();
        let worker = thread::spawn(move || {
            let observation =
                held_search(listener, ended, connection, events, partial_body, change);
            let _ = sent.send(observation);
        });
        Self {
            endpoint,
            stop,
            active,
            done,
            worker: Some(worker),
        }
    }
    fn stop_and_join(&mut self) -> Result<()> {
        self.stop.store(true, AtomicOrdering::Release);
        if let Some(stream) = self
            .active
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take()
        {
            // Interrupt an accepted peer even if a setup/assertion failed while
            // it was reading the request, not only in the response-hold loop.
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(worker) = self.worker.take() {
            ensure!(worker.join().is_ok(), "held research peer panicked");
        }
        Ok(())
    }
    fn finish(&mut self) -> Result<HeldSearchObservation> {
        // Success must arrive before forced cleanup. A socket closed by Drop
        // must never count as evidence that the owned client canceled itself.
        let observation = self
            .done
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| anyhow::anyhow!("held research peer lacks a client-closure witness"));
        let joined = self.stop_and_join();
        joined?;
        observation?
    }
}
impl Drop for HeldSearchPeer {
    fn drop(&mut self) {
        // Join on all paths, including assertion unwinding. Explicit finish
        // reports worker errors; Drop must not cause a second unwind.
        let _ = self.stop_and_join();
    }
}
fn held_search(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    active: ActiveSocket,
    events: Events,
    partial_body: bool,
    change: Option<Box<dyn FnOnce() + Send>>,
) -> Result<HeldSearchObservation> {
    let bound = Instant::now() + Duration::from_secs(30);
    let mut stream = loop {
        ensure!(
            !stop.load(AtomicOrdering::Acquire),
            "held peer cleanup before request"
        );
        ensure!(Instant::now() < bound, "held peer accept deadline");
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => return Err(error.into()),
        }
    };
    stream.set_nonblocking(true)?;
    let control = stream.try_clone()?;
    *active.lock().unwrap() = Some(control);
    // Closes the race where cleanup preceded publication of the accepted socket.
    ensure!(
        !stop.load(AtomicOrdering::Acquire),
        "held peer cleanup after accept"
    );
    let request = read_held_search(&mut stream, &stop, bound)?;
    let request_at = Instant::now();
    events.lock().unwrap().push("research");
    let partial_body_at = if partial_body {
        let mut remaining = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n{\"status\":\"completed\"".as_slice();
        while !remaining.is_empty() {
            held_peer_current(&stop, bound)?;
            match stream.write(remaining) {
                Ok(0) => anyhow::bail!("held peer partial response write stopped"),
                Ok(written) => remaining = &remaining[written..],
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2))
                }
                Err(error) => return Err(error.into()),
            }
        }
        Some(Instant::now())
    } else {
        None
    };
    if let Some(change) = change {
        change();
    }
    let mut byte = [0];
    loop {
        ensure!(
            !stop.load(AtomicOrdering::Acquire),
            "forced cleanup is not client closure"
        );
        ensure!(Instant::now() < bound, "held peer closure deadline");
        match stream.read(&mut byte) {
            Ok(0) => break,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
                ) =>
            {
                break
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                thread::sleep(Duration::from_millis(2))
            }
            Ok(_) => anyhow::bail!("unexpected request bytes while response withheld"),
            Err(error) => return Err(error.into()),
        }
    }
    ensure!(
        !stop.load(AtomicOrdering::Acquire),
        "forced cleanup is not client closure"
    );
    let closed_at = Instant::now();
    ensure!(
        matches!(listener.accept(), Err(error) if error.kind()==std::io::ErrorKind::WouldBlock),
        "extra research connection after closure"
    );
    Ok(HeldSearchObservation {
        request,
        request_at,
        partial_body_at,
        closed_at,
    })
}

fn held_peer_current(stop: &AtomicBool, bound: Instant) -> Result<()> {
    ensure!(
        !stop.load(AtomicOrdering::Acquire),
        "held peer cleanup interrupted I/O"
    );
    ensure!(Instant::now() < bound, "held peer I/O deadline");
    Ok(())
}
fn held_read_exact(
    stream: &mut TcpStream,
    bytes: &mut [u8],
    stop: &AtomicBool,
    bound: Instant,
) -> Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        held_peer_current(stop, bound)?;
        match stream.read(&mut bytes[offset..]) {
            Ok(0) => anyhow::bail!("held peer request ended early"),
            Ok(read) => offset += read,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2))
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
fn read_held_search(stream: &mut TcpStream, stop: &AtomicBool, bound: Instant) -> Result<Value> {
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        ensure!(header.len() < 16384, "held peer request header limit");
        let mut byte = [0];
        held_read_exact(stream, &mut byte, stop, bound)?;
        header.push(byte[0]);
    }
    let header = String::from_utf8(header)?;
    ensure!(
        header.starts_with("POST /v1/responses HTTP/1.1"),
        "held peer request target differs"
    );
    let (_, length) = header
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .ok_or_else(|| anyhow::anyhow!("held peer request length absent"))?;
    let length: usize = length.trim().parse()?;
    ensure!(length <= 1024 * 1024, "held peer request body limit");
    let mut body = vec![0; length];
    held_read_exact(stream, &mut body, stop, bound)?;
    Ok(serde_json::from_slice(&body)?)
}

fn fixture_health() -> (ServiceHealth, Arc<AtomicBool>, UnixDatagram) {
    let (socket, notifications) = UnixDatagram::pair().unwrap();
    let health = ServiceHealth::fixture(socket);
    let failed = health.failure_witness();
    (health, failed, notifications)
}

fn assert_interpreted_refusal(setup: &Setup, run: &Run, question: &str) {
    assert!(
        run.result.is_err(),
        "refused composition unexpectedly succeeded"
    );
    assert!(setup.generated(run.root).is_empty());
    let records = setup.records(run.root);
    let interpreted: Vec<_> = records
        .iter()
        .filter_map(|record| match record {
            Record::Turn(turn) if turn.outcome == Outcome::Interpreted => Some(turn),
            _ => None,
        })
        .collect();
    assert_eq!(interpreted.len(), 1);
    assert_eq!(interpreted[0].text.as_deref(), Some(question));
}

fn assert_held_search(observed: &HeldSearchObservation, run: &Run, partial_body: bool) {
    assert_eq!(observed.request["model"], "gpt-5.6-terra");
    assert_eq!(observed.request["tool_choice"], "required");
    let exact = observed.request["input"][0]["content"][0]["text"]
        .as_str()
        .unwrap();
    let context: Value = serde_json::from_str(exact).unwrap();
    let request = &context["request_context"];
    assert_eq!(request["conversation"], json!(run.root));
    assert_eq!(request["request"]["text"], QUESTION);
    assert_eq!(request["history"], json!([]));
    assert_eq!(request["retrieval"]["source"], json!(run.index));
    assert!(context.to_string().contains(MEMORY));
    assert_eq!(observed.partial_body_at.is_some(), partial_body);
    if let Some(written) = observed.partial_body_at {
        assert!(written >= observed.request_at && written <= observed.closed_at);
    }
    assert!(observed.closed_at >= observed.request_at);
}

#[cfg(feature = "installed-resident-output")]
#[test]
fn ordinary_installed_reader_leaves_mapped_files_out_of_the_request() -> Result<()> {
    use crate::workflow::OrdinaryPdfFixtureRounds;
    use anyhow::Context;

    const OTHER_PAGE: &str = "OTHER_PAGE_TEXT_MUST_NOT_BE_SENT";
    for (format, bytes, original_pages, redir) in [
        ("pdf", pdf_bytes(OTHER_PAGE), 2, 0),
        ("epub", ordinary_epub::book(OTHER_PAGE), 600, 310),
    ] {
        let fixture = Fixture::new();
        fs::DirBuilder::new().mode(0o700).create(&fixture.0)?;
        let document = Uuid::new_v4();
        let page = Uuid::new_v4();
        let path = fixture.0.join(format!("{document}.{format}"));
        fs::write(&path, &bytes)?;
        files::atomic_json(
            &fixture.0.join(format!("{document}.content")),
            &json!({"fileType":format, "cPages":{"original":{"value":original_pages},"pages":[
                {"id":page,"idx":{"value":"a"},"redir":{"value":redir}}
            ]}}),
        )?;
        let qualification = researched_fixture::Scope::new(None, false);
        let mut model = Scripted::new(Reading::Deny, false, false, Events::default());
        model.captured_page_only = true;
        let frames = model.frames.clone();
        let (mut results, records) = crate::workflow::ordinary_pdf_receiver_fixture(
            document,
            page,
            fixture.0.clone(),
            None,
            OrdinaryPdfFixtureRounds {
                epub: format == "epub",
                rendered: None,
                source_targets: false,
                bound_successor_for_book_edit: None,
                count: 1,
                before_round: None,
            },
            |_, _, _| model,
            |_, _| Ok(()),
        )?;
        results
            .remove(0)
            .with_context(|| format!("Ordinary {format} request"))?;
        assert!(qualification.ordinary_pdf_workers().is_empty());
        assert_eq!(qualification.constructions(), 0);
        let frame = records
            .iter()
            .find_map(|record| match record {
                Record::RequestEvidence(evidence) => Some(&evidence.frame_json),
                _ => None,
            })
            .context("ordinary request evidence absent")?;
        let frame: Value = serde_json::from_str(frame)?;
        assert_eq!(frame["source"]["kind"], "captured-page");
        assert_eq!(
            frame["source"]["source_record"]["record"]["before"]["page"],
            json!(page)
        );
        assert!(frame["retrieval"].is_null());
        let frames = frames.lock().unwrap();
        assert_eq!(frames.len(), 3);
        assert!(!frames[2].images.is_empty());
        assert!(frames[2]
            .images
            .iter()
            .all(|image| frames[1].images.contains(image)));
        assert!(frames[2]
            .texts
            .iter()
            .all(|text| !text.contains(OTHER_PAGE)));
        assert_eq!(fs::read(path)?, bytes);
    }
    Ok(())
}

#[cfg(feature = "installed-resident-output")]
fn assert_pdf_reuse_between_service_questions(
    executable: &std::path::Path,
    oversized: &[u8],
) -> Result<()> {
    use crate::conversation::Role;
    use crate::workflow::OrdinaryPdfFixtureRounds;
    use anyhow::Context;
    use std::{cell::Cell, rc::Rc};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Service {
        Running,
        Restarted,
        Drifted,
        Oversized,
    }

    const SECOND_QUESTION: &str = "Explain unrelated first page. Don't use the internet.";
    let fixture = Fixture::new();
    fs::DirBuilder::new().mode(0o700).create(&fixture.0)?;
    let document = Uuid::new_v4();
    let page = Uuid::new_v4();
    let inserted_page = Uuid::new_v4();
    let pdf = fixture.0.join(format!("{document}.pdf"));
    let table = fixture.0.join(format!("{document}.content"));
    let initial = json!({"fileType":"pdf", "cPages":{"original":{"value":2}, "pages":[
        {"id":page, "idx":{"value":"a"}, "redir":{"value":0}}
    ]}});
    let mut after_insertion = initial.clone();
    after_insertion["cPages"]["pages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":inserted_page, "idx":{"value":"b"}, "redir":null}));
    let bytes = pdf_bytes("alpha acceleration original PDF evidence");
    fs::write(&pdf, &bytes)?;
    files::atomic_json(&table, &initial)?;

    for service in [
        Service::Running,
        Service::Restarted,
        Service::Drifted,
        Service::Oversized,
    ] {
        let (questions, expected_reuse) = match service {
            Service::Running => (
                &[(OPT_OUT, Some(2)), (SECOND_QUESTION, Some(1))][..],
                &[false, true][..],
            ),
            Service::Restarted => (&[(SECOND_QUESTION, Some(1))][..], &[false][..]),
            Service::Drifted => (
                &[
                    (OPT_OUT, Some(2)),
                    ("Describe alpha acceleration. Don't use the internet.", None),
                    (
                        "Locate alpha acceleration. Don't use the internet.",
                        Some(2),
                    ),
                ][..],
                &[false, true, false][..],
            ),
            Service::Oversized => (
                &[
                    ("Where is velocity marker? Don't use the internet.", Some(7)),
                    (
                        "Explain the velocity marker. Don't use the internet.",
                        Some(7),
                    ),
                ][..],
                &[false, false][..],
            ),
        };
        if service == Service::Oversized {
            let mut large_table = initial.clone();
            large_table["cPages"]["original"]["value"] = json!(7);
            files::atomic_json(&table, &large_table)?;
            fs::write(&pdf, oversized)?;
        }
        let current_round = Rc::new(Cell::new(0));
        let mut update_order = |round, order: &std::path::Path| -> Result<()> {
            current_round.set(round);
            if service == Service::Running && round == 1 {
                // Model the new on-disk table after Buddy-page insertion.
                files::atomic_json(&table, &after_insertion)?;
            }
            if matches!(service, Service::Restarted | Service::Drifted)
                || service == Service::Running && round != 0
            {
                // Restore the same valid order before the request following drift.
                fs::write(order, inserted_page.to_string())?;
            }
            Ok(())
        };
        let scope = researched_fixture::Scope::new(None, false);
        let mut model = Scripted::new(Reading::Deny, false, false, Events::default());
        model.ordinary_pdf_questions = questions.iter().map(|&(question, _)| question).collect();
        let frames = model.frames.clone();
        let (results, records) = crate::workflow::ordinary_pdf_receiver_fixture(
            document,
            page,
            fixture.0.clone(),
            Some(executable.into()),
            OrdinaryPdfFixtureRounds {
                epub: false,
                rendered: None,
                source_targets: false,
                bound_successor_for_book_edit: None,
                count: questions.len(),
                before_round: Some(&mut update_order),
            },
            |_, order_change, _| {
                let round = current_round.clone();
                model.hook = Some(Box::new(move |stage, pending| {
                    if service == Service::Drifted && round.get() == 1 && stage == 2 && !pending {
                        fs::write(&order_change, Uuid::new_v4().to_string()).unwrap();
                    }
                }));
                model
            },
            |_, _| Ok(()),
        )
        .with_context(|| format!("Run ordinary PDF service case {service:?}"))?;
        assert_eq!(results.len(), questions.len());
        assert_eq!(
            frames.lock().unwrap().len(),
            questions.len() * 3,
            "{service:?}: {results:?}"
        );
        assert_eq!(scope.constructions(), 0);
        let workers = scope.ordinary_pdf_workers();
        assert_eq!(
            workers
                .iter()
                .map(|&(_, reused)| reused)
                .collect::<Vec<_>>(),
            expected_reuse,
            "{service:?}"
        );
        assert_eq!(
            workers
                .iter()
                .map(|&(request, _)| request)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            workers.len()
        );
        let expected_file = if service == Service::Oversized {
            oversized
        } else {
            &bytes
        };
        assert_eq!(fs::read(&pdf)?, expected_file);
        for (&(question, expected_page), result) in questions.iter().zip(results) {
            let user = records
                .iter()
                .find_map(|record| match record {
                    Record::Turn(turn)
                        if turn.role == Role::User && turn.text.as_deref() == Some(question) =>
                    {
                        Some(turn)
                    }
                    _ => None,
                })
                .context("Verified service question absent")?;
            let evidence = records
                .iter()
                .find_map(|record| match record {
                    Record::RequestEvidence(evidence) if evidence.request_turn == user.id => {
                        Some(evidence)
                    }
                    _ => None,
                })
                .context("Exact service request evidence absent")?;
            let frame: Value = serde_json::from_str(&evidence.frame_json)?;
            assert_eq!(frame["request"]["text"], question);
            let answer = records.iter().find_map(|record| match record {
                Record::Turn(turn)
                    if turn.role == Role::Assistant && turn.exchange == user.exchange =>
                {
                    Some(turn)
                }
                _ => None,
            });
            if let Some(page) = expected_page {
                result
                    .with_context(|| format!("Complete ordinary PDF service case {service:?}"))?;
                assert_eq!(frame["retrieval"]["passages"][0]["page"], page);
                let text = answer
                    .and_then(|turn| turn.text.as_deref())
                    .context("Service answer absent")?;
                assert!(text.contains(&format!("[S1] Document {document}, page {page}")));
            } else {
                let error = result.expect_err("Warm answer survived native order drift");
                assert!(format!("{error:#}").contains("Reader source or page order changed"));
                assert!(answer.is_none());
            }
        }
    }
    Ok(())
}

#[cfg(feature = "installed-resident-output")]
#[test]
#[ignore = "requires the retained normal binary; mandatory hosted composed-reader step"]
fn ordinary_installed_pdf_uses_exact_passages_and_fences_source_drift() -> Result<()> {
    use crate::conversation::{AttemptReason, Role};
    use anyhow::Context;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Case {
        Offline,
        Required,
        FileBeforeAnswer,
        FileDuringAnswerObserve,
        FileDuringGeneratedObserve,
        FileDuringRefusalObserve,
        OrderBeforeVerification,
        OrderAfterAnswer,
        ConversationBeforeVerification,
        IdenticalTurnSuccessor,
        IdenticalTurnSuccessorWithTargets,
        ProviderRefusal,
        ProviderRefusalWithTargets,
        ProviderRefusalAfterNativeDrift,
        ProviderRefusalAfterConversationDrift,
    }

    let executable = retained_command();
    let large_page = ".".repeat(400 * 1024);
    ordinary_epub::check_open_book(&executable).context(
        "Exercise ordinary EPUB question and section citations with the retained binary",
    )?;
    let oversized = pdf_pages(
        [large_page.as_str(); 6]
            .into_iter()
            .chain(["velocity marker"]),
    );
    assert_pdf_reuse_between_service_questions(&executable, &oversized)
        .context("Exercise ordinary PDF service reuse, drift, restart and oversized input")?;
    crate::workflow::assert_ordinary_pdf_reuse(
        &executable,
        &pdf_bytes("alpha acceleration original source"),
        &pdf_bytes("beta velocity revised source"),
    )
    .context("Exercise ordinary PDF source reuse with the retained binary")?;
    let fixture = Fixture::new();
    let root = fixture.0.join("original-pdf");
    let mut directories = fs::DirBuilder::new();
    directories.mode(0o700);
    directories
        .create(&fixture.0)
        .context("Create private ordinary PDF fixture parent")?;
    directories
        .create(&root)
        .context("Create ordinary PDF fixture source directory")?;
    let document = Uuid::new_v4();
    let page = Uuid::new_v4();
    let decoy = Uuid::new_v4();
    let path = root.join(format!("{document}.pdf"));
    let bytes = pdf_bytes("alpha acceleration original PDF evidence");
    files::atomic_json(
        &root.join(format!("{document}.content")),
        &json!({"fileType":"pdf", "cPages":{"original":{"value":2}, "pages":[
            {"id":page, "idx":{"value":"a"}, "redir":{"value":0}}
        ]}}),
    )
    .context("Write ordinary PDF fixture page table")?;
    fs::write(
        root.join(format!("{decoy}.pdf")),
        pdf_bytes("alpha acceleration DECOY_DOCUMENT_MUST_NOT_BE_QUERIED"),
    )
    .context("Write ordinary PDF fixture decoy")?;

    for case in [
        Case::Offline,
        Case::Required,
        Case::FileBeforeAnswer,
        Case::FileDuringAnswerObserve,
        Case::FileDuringGeneratedObserve,
        Case::FileDuringRefusalObserve,
        Case::OrderBeforeVerification,
        Case::OrderAfterAnswer,
        Case::ConversationBeforeVerification,
        Case::IdenticalTurnSuccessor,
        Case::IdenticalTurnSuccessorWithTargets,
        Case::ProviderRefusal,
        Case::ProviderRefusalWithTargets,
        Case::ProviderRefusalAfterNativeDrift,
        Case::ProviderRefusalAfterConversationDrift,
    ] {
        fs::write(&path, &bytes)
            .with_context(|| format!("Write original PDF for ordinary fixture case {case:?}"))?;
        let events = Arc::new(Mutex::new(vec![]));
        let required = case == Case::Required;
        let peer = required.then(|| SearchPeer::new(events.clone(), None));
        let scope =
            researched_fixture::Scope::new(peer.as_ref().map(|peer| peer.endpoint.clone()), false);
        let replace_interpreted_turn = matches!(
            case,
            Case::IdenticalTurnSuccessor | Case::IdenticalTurnSuccessorWithTargets
        );
        if replace_interpreted_turn {
            scope.replace_interpreted_pdf_turn();
        }
        let mut script = Scripted::new(
            if required {
                Reading::Clear
            } else {
                Reading::Deny
            },
            required,
            false,
            events.clone(),
        );
        let stable_refusal = matches!(
            case,
            Case::ProviderRefusal | Case::ProviderRefusalWithTargets
        );
        script.answer_refused = stable_refusal
            || matches!(
                case,
                Case::ProviderRefusalAfterNativeDrift
                    | Case::ProviderRefusalAfterConversationDrift
                    | Case::FileDuringRefusalObserve
            );
        let frames = script.frames.clone();
        let path = &path;
        let (mut results, records) = crate::workflow::ordinary_pdf_receiver_fixture(
            document,
            page,
            root.clone(),
            Some(executable.clone()),
            crate::workflow::OrdinaryPdfFixtureRounds {
                epub: false,
                rendered: None,
                source_targets: matches!(
                    case,
                    Case::ProviderRefusalWithTargets | Case::IdenticalTurnSuccessorWithTargets
                ),
                bound_successor_for_book_edit: None,
                count: 1,
                before_round: None,
            },
            |ledger, order_change, change_file_on_observe| {
                let ledger = Ledger::new(ledger.store().clone());
                script.hook = Some(Box::new(move |stage, pending| {
                    match (case, stage, pending) {
                        (Case::FileBeforeAnswer, 2, true) => {
                            fs::write(path, b"changed before answer handoff").unwrap();
                        }
                        (Case::FileDuringAnswerObserve, 2, true)
                        | (
                            Case::FileDuringGeneratedObserve | Case::FileDuringRefusalObserve,
                            2,
                            false,
                        ) => {
                            change_file_on_observe.set(true);
                        }
                        (Case::OrderBeforeVerification, 1, true)
                        | (
                            Case::OrderAfterAnswer | Case::ProviderRefusalAfterNativeDrift,
                            2,
                            false,
                        ) => {
                            fs::write(&order_change, Uuid::new_v4().to_string()).unwrap();
                        }
                        (Case::ConversationBeforeVerification, 1, true)
                        | (Case::ProviderRefusalAfterConversationDrift, 2, false) => {
                            let user = ledger
                                .store()
                                .snapshot_heads_matching(
                                    &[Namespace::Conversation],
                                    crate::storage::MAX_ITEMS,
                                    |_| true,
                                )
                                .unwrap()
                                .iter()
                                .find_map(|envelope| match Ledger::decode(envelope).unwrap() {
                                    Record::Turn(turn) if turn.role == Role::User => Some(turn),
                                    _ => None,
                                })
                                .unwrap();
                            let root = ledger
                                .expected(Namespace::Conversation, user.conversation)
                                .unwrap();
                            ledger
                                .append(
                                    Uuid::new_v4(),
                                    root,
                                    crate::conversation::Turn {
                                        id: Uuid::new_v4(),
                                        exchange: Uuid::new_v4(),
                                        sequence: 0,
                                        outcome: Outcome::Interpreted,
                                        sources: vec![],
                                        text: Some("Concurrent Reader question".into()),
                                        ..user
                                    },
                                    vec![],
                                )
                                .unwrap();
                        }
                        _ => {}
                    }
                }));
                script
            },
            |ledger, records| {
                let failures = records
                    .iter()
                    .filter_map(|record| match record {
                        Record::OutcomeFact(fact)
                            if fact.reason == AttemptReason::ProviderFailure =>
                        {
                            Some(fact)
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if stable_refusal {
                    let [fact] = failures.as_slice() else {
                        anyhow::bail!("Expected exactly one provider failure");
                    };
                    let description = crate::conversation::feedback::describe_stored(
                        ledger,
                        fact.conversation,
                        fact.id,
                    )?;
                    assert_eq!(description.turn, fact.turn);
                    assert_eq!(description.code, "RMB-RESPONSE-UNAVAILABLE");
                    assert_eq!(
                        description.text,
                        "The response service could not complete this request."
                    );
                    let user = records
                        .iter()
                        .find_map(|record| match record {
                            Record::Turn(user) if user.id == fact.turn => Some(user),
                            _ => None,
                        })
                        .unwrap();
                    assert_eq!(user.outcome, Outcome::Failed);
                    assert_eq!(user.text.as_deref(), Some(OPT_OUT));
                    let retained = ledger
                        .request_evidence(
                            fact.conversation,
                            fact.turn,
                            crate::writer::stage(user.exchange, "reader-pdf-evidence"),
                        )?
                        .ok_or_else(|| anyhow::anyhow!("Missing original PDF request evidence"))?;
                    assert_eq!(retained.record.request_turn, user.id);
                    assert!(!user.sources.is_empty());
                    assert!(user.sources.iter().all(|id| retained
                        .record
                        .sources
                        .iter()
                        .any(|source| source.record == *id)));
                } else {
                    assert!(failures.is_empty(), "{case:?}");
                }
                Ok(())
            },
        )
        .with_context(|| format!("Run ordinary PDF fixture case {case:?}"))?;
        let result = results
            .pop()
            .context("Ordinary PDF fixture returned no result")?;
        let frames = frames.lock().unwrap();
        let assistants = records
            .iter()
            .filter_map(|record| match record {
                Record::Turn(turn) if turn.role == Role::Assistant => Some(turn),
                _ => None,
            })
            .collect::<Vec<_>>();
        let question = if required { QUESTION } else { OPT_OUT };
        let interpreted = !matches!(
            case,
            Case::OrderBeforeVerification | Case::ConversationBeforeVerification
        );
        let question_outcome = if stable_refusal {
            Outcome::Failed
        } else {
            Outcome::Interpreted
        };
        assert_eq!(
            records.iter().any(|record| matches!(
                record, Record::Turn(turn)
                    if turn.outcome == question_outcome && turn.text.as_deref() == Some(question)
            )),
            interpreted,
            "{case:?}"
        );
        assert_eq!(
            records
                .iter()
                .filter(|r| matches!(r, Record::RequestEvidence(_)))
                .count(),
            usize::from(interpreted && !replace_interpreted_turn),
            "{case:?}",
        );
        assert_eq!(scope.constructions(), usize::from(required));
        let expected_calls = match case {
            Case::OrderBeforeVerification | Case::ConversationBeforeVerification => 1,
            Case::FileBeforeAnswer
            | Case::FileDuringAnswerObserve
            | Case::IdenticalTurnSuccessor
            | Case::IdenticalTurnSuccessorWithTargets => 2,
            _ => 3,
        };
        assert_eq!(frames.len(), expected_calls, "{case:?}: {result:?}");
        if !matches!(case, Case::Offline | Case::Required) {
            let error = result.expect_err("Changed request reached publication");
            let cause = format!("{error:#}");
            if matches!(
                case,
                Case::OrderBeforeVerification
                    | Case::OrderAfterAnswer
                    | Case::ProviderRefusalAfterNativeDrift
            ) {
                assert!(
                    cause.contains("Reader source or page order changed"),
                    "{cause}"
                );
            }
            if matches!(
                case,
                Case::FileDuringAnswerObserve
                    | Case::FileDuringGeneratedObserve
                    | Case::FileDuringRefusalObserve
            ) {
                assert_eq!(fs::read(path)?, b"changed during native Observe");
                assert!(cause.contains("Original document file changed"), "{cause}");
            }
            if replace_interpreted_turn {
                assert!(
                    cause.contains("Reader follow-up changed before source preparation"),
                    "{cause}"
                );
            }
            if stable_refusal {
                assert!(error.is::<crate::llm::ProviderRefused>(), "{cause}");
            }
            assert!(assistants.is_empty(), "{case:?}");
            continue;
        }

        result.with_context(|| format!("Complete ordinary PDF Reader case {case:?}"))?;
        let expected_events = if required {
            vec!["analysis", "verification", "research", "answer"]
        } else {
            vec!["analysis", "verification", "answer"]
        };
        assert_eq!(*events.lock().unwrap(), expected_events);
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].images, frames[1].images);
        assert!(!frames[2].images.is_empty());
        let context: Value = serde_json::from_str(&frames[2].texts[1])?;
        let request = &context["context"]["request_context"];
        assert_eq!(request["request"]["text"], question);
        if let Some(peer) = &peer {
            let requests = peer.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0]["model"], "gpt-5.6-terra");
            assert_eq!(requests[0]["tool_choice"], "required");
            let encoded = requests[0]["input"][0]["content"][0]["text"]
                .as_str()
                .unwrap();
            let search: Value = serde_json::from_str(encoded)?;
            assert_eq!(&search["request_context"], request);
            assert_eq!(
                context["research"]["identity"]["context_sha256"],
                digest(encoded.as_bytes())
            );
        } else {
            assert_eq!(
                context["research"]["disabled_reason"],
                "request-no-internet"
            );
        }
        let retrieval = &request["retrieval"];
        assert_eq!(retrieval["source"]["document"], json!(document));
        assert_eq!(retrieval["source"]["source_sha256"], digest(&bytes));
        assert_eq!(retrieval["passages"][0]["page"], 2);
        assert!(retrieval["passages"][0]["text"]
            .as_str()
            .unwrap()
            .contains("alpha acceleration"));
        assert!(!context
            .to_string()
            .contains("DECOY_DOCUMENT_MUST_NOT_BE_QUERIED"));
        let [draft] = assistants.as_slice() else {
            anyhow::bail!("Expected one persisted Reader answer");
        };
        let answer = draft.text.as_deref().unwrap();
        assert!(answer.contains(&format!("[S1] Document {document}, page 2")));
        if required {
            assert!(answer.contains("[R1] Fixture research - https://example.invalid/alpha"));
        }
        assert!(!answer.contains("ANALYSIS_PROPOSAL_MUST_NOT_PUBLISH"));
    }
    Ok(())
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

#[test]
#[ignore = "requires the retained normal binary; mandatory hosted composed-reader step"]
fn researched_pdf_currentness_loss_stops_later_work() {
    // Inject modeled health loss into the actual Workflow owner. This is not
    // physical-contact or native input-device qualification.
    for after in [false, true] {
        let setup = Setup::new(MemoryMode::Document, false, false);
        let scope = researched_fixture::Scope::new(None, false);
        let (health, failed, _notifications) = fixture_health();
        let events = Events::default();
        let mut model = Scripted::new(Reading::Deny, false, true, events.clone());
        let frames = model.frames.clone();
        let refused = Arc::new(AtomicBool::new(false));
        model.currentness_refusal = Some(refused.clone());
        let change = failed.clone();
        model.hook = Some(Box::new(move |stage, before| {
            if stage == 2 && before != after {
                change.store(true, AtomicOrdering::Release);
            }
        }));
        let run = setup.run_with(
            model,
            RunOptions {
                health: Some(health),
                ..RunOptions::default()
            },
        );
        assert_interpreted_refusal(&setup, &run, OPT_OUT);
        assert!(failed.load(AtomicOrdering::Acquire));
        assert!(
            refused.load(AtomicOrdering::Acquire),
            "real handoff/progress must witness Workflow currentness loss"
        );
        assert_eq!(frames.lock().unwrap().len(), if after { 3 } else { 2 });
        let expected = if after {
            vec!["analysis", "verification", "answer"]
        } else {
            vec!["analysis", "verification"]
        };
        assert_eq!(*events.lock().unwrap(), expected);
        assert_eq!(scope.constructions(), 0);
        assert!(scope.generation().is_none());
    }
    let setup = Setup::new(MemoryMode::Document, false, true);
    let (health, failed, _notifications) = fixture_health();
    let events = Events::default();
    let change = failed.clone();
    let mut peer = HeldSearchPeer::new(
        events.clone(),
        false,
        Some(Box::new(move || {
            change.store(true, AtomicOrdering::Release);
        })),
    );
    let scope = researched_fixture::Scope::new(Some(peer.endpoint.clone()), false);
    let model = Scripted::new(Reading::Clear, true, true, events.clone());
    let frames = model.frames.clone();
    let run = setup.run_with(
        model,
        RunOptions {
            health: Some(health),
            ..RunOptions::default()
        },
    );
    let observed = peer.finish().unwrap(); // joins before any outcome assertion
    assert_interpreted_refusal(&setup, &run, QUESTION);
    assert_held_search(&observed, &run, false);
    assert!(failed.load(AtomicOrdering::Acquire));
    // Distinguish the actual health-progress refusal from the peer's 30-second
    // cleanup bound and the untouched 67-second provider/420-second parent cap.
    assert!(run.returned >= observed.request_at);
    assert!(run.returned.duration_since(observed.request_at) < Duration::from_secs(3));
    assert!(run.returned < run.deadline);
    assert_eq!(frames.lock().unwrap().len(), 2);
    assert_eq!(
        *events.lock().unwrap(),
        vec!["analysis", "verification", "research"]
    );
    assert_eq!(scope.constructions(), 1);
    assert!(scope.generation().is_none());
}

#[test]
#[ignore = "requires the retained normal binary; mandatory hosted composed-reader step"]
fn researched_pdf_withheld_search_expires_original_deadline() {
    for partial_body in [false, true] {
        let setup = Setup::new(MemoryMode::Document, false, true);
        let events = Events::default();
        let mut peer = HeldSearchPeer::new(events.clone(), partial_body, None);
        let scope = researched_fixture::Scope::new(Some(peer.endpoint.clone()), false);
        let model = Scripted::new(Reading::Clear, true, true, events.clone());
        let frames = model.frames.clone();
        let run = setup.run_with(
            model,
            RunOptions {
                parent_window: Some(Duration::from_secs(6)),
                ..RunOptions::default()
            },
        );
        let observed = peer.finish().unwrap(); // no remainder or forced-close success
        assert_interpreted_refusal(&setup, &run, QUESTION);
        assert_held_search(&observed, &run, partial_body);
        assert!(
            observed.request_at < run.deadline,
            "request must be in flight before the original deadline"
        );
        assert!(
            run.returned >= run.deadline,
            "must exercise actual remaining-deadline expiry"
        );
        assert_eq!(frames.lock().unwrap().len(), 2);
        assert_eq!(
            *events.lock().unwrap(),
            vec!["analysis", "verification", "research"]
        );
        assert_eq!(scope.constructions(), 1);
        assert!(scope.generation().is_none());
    }
}

#[test]
#[ignore = "requires the retained normal binary; mandatory hosted composed-reader step"]
fn researched_pdf_original_read_allowance_cannot_be_renewed() {
    {
        let setup = Setup::new(MemoryMode::Document, false, false);
        let scope = researched_fixture::Scope::new(None, false);
        let events = Events::default();
        let model = Scripted::new(Reading::Deny, false, true, events.clone());
        let frames = model.frames.clone();
        let run = setup.run_with(
            model,
            RunOptions {
                exhaust_submission: true,
                ..RunOptions::default()
            },
        );
        assert!(
            run.result.as_ref().err().is_some_and(read_budget_exhausted),
            "must refuse the original exhausted submission allowance"
        );
        assert_eq!(setup.snapshot().token, run.prepared);
        assert!(frames.lock().unwrap().is_empty());
        assert!(events.lock().unwrap().is_empty());
        assert!(setup.generated(run.root).is_empty());
        assert!(!setup.records(run.root).iter().any(
            |record| matches!(record, Record::Turn(turn) if turn.outcome == Outcome::Interpreted)
        ));
        assert_eq!(scope.constructions(), 0);
        assert!(scope.generation().is_none());
    }
    let setup = Setup::new(MemoryMode::Document, false, false);
    let scope = researched_fixture::Scope::new(None, false);
    let events = Events::default();
    let mut model = Scripted::new(Reading::Deny, false, true, events.clone());
    let frames = model.frames.clone();
    let exhausted = Arc::new(Mutex::new(ProgressExhaustion::default()));
    model.exhaust_answer_progress = Some(exhausted.clone());
    let run = setup.run(model);
    assert_interpreted_refusal(&setup, &run, OPT_OUT);
    let observed = exhausted.lock().unwrap();
    assert!((1..=4096).contains(&observed.calls));
    assert!(
        observed.budget_cause && observed.sticky_budget_cause,
        "actual progress must fail and remain failed with selected read budget exhausted"
    );
    assert!(
        run.returned < run.deadline,
        "budget exhaustion must precede parent expiry"
    );
    assert_eq!(frames.lock().unwrap().len(), 2);
    assert_eq!(*events.lock().unwrap(), vec!["analysis", "verification"]);
    assert_eq!(scope.constructions(), 0);
    assert!(scope.generation().is_none());
}

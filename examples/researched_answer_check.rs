//! Explicit public authored answering fixture; research is synthetic, no tablet or web acquisition.
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use clap::Parser;
use remarkable_reader_buddy::{
    conversation::{
        self as c,
        receiving::{
            answer as a,
            memory::{self as cm, research as rr},
            ReceivingBudget, RequestSource,
        },
    },
    llm::{
        openai::{OpenAI, DEFAULT_MODEL},
        LLMEngine,
    },
    memory as m,
    retrieval::{Index, NormalizedDocument, SourceFormat, SourceUnit},
    storage::{digest, Media, Namespace, Store, StorePaths, Uuid},
};
use serde_json::{json, Value};
use std::{
    cell::Cell,
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
#[derive(Parser)]
struct Args {
    #[arg(long, required = true)]
    live: bool,
    #[arg(long)]
    output: PathBuf,
    #[arg(long,default_value=DEFAULT_MODEL)]
    model: String,
    /// Explicit host-only base; existing OpenAI appends /v1/chat/completions.
    #[arg(long)]
    base_url: Option<String>,
    #[arg(long, default_value_t = 90)]
    timeout_seconds: u64,
    /// This example counts UTF8 descriptor bytes, not model-window tokens.
    #[arg(long, default_value_t = 1048576)]
    max_counted_bytes: usize,
    #[arg(long, default_value_t = 16384)]
    max_answer_bytes: usize,
}
struct TemporaryRoot(PathBuf);
impl TemporaryRoot {
    fn create() -> Result<Self> {
        let p = std::env::temp_dir().join(format!("buddy-answer-check-{}", Uuid::new_v4()));
        fs::create_dir(&p)?;
        Ok(Self(p))
    }
}
impl Drop for TemporaryRoot {
    fn drop(&mut self) {
        if let (Ok(p), Ok(parent)) = (self.0.canonicalize(), std::env::temp_dir().canonicalize()) {
            if p.parent() == Some(parent.as_path()) && p.file_name() == self.0.file_name() {
                let _ = fs::remove_dir_all(p);
            }
        }
    }
}
fn turn(conversation: Uuid, text: &str) -> c::Turn {
    c::Turn {
        id: Uuid::new_v4(),
        conversation,
        exchange: Uuid::new_v4(),
        sequence: 0,
        role: c::Role::User,
        mode: c::Mode::Reader,
        outcome: c::Outcome::Interpreted,
        text: Some(text.into()),
        sources: vec![],
        correction_of: None,
        created_ms: 1,
        updated_ms: 1,
        completion: None,
    }
}
struct SyntheticResearch;
impl rr::Provider for SyntheticResearch {
    fn research(&self, r: &rr::ResearchRequest<'_>) -> Result<rr::ResearchResult> {
        Ok(rr::ResearchResult{identity:r.identity.clone(),status:rr::Status::Complete,basis:rr::EvidenceBasis::Synthetic,diagnostic:None,queries:vec!["authored fixture mole definition".into()],sources:vec![rr::Citation{url:"https://www.bipm.org/en/si-base-units/mole".into(),title:"Authored synthetic BIPM attribution, not a fetched page".into(),excerpt:"Authored synthetic research fixture: the mole contains exactly 6.02214076 x 10^23 specified elementary entities. This attribution is not a fetched quotation.".into(),published_ms:None}]})
    }
}
fn frozen(root: &TemporaryRoot) -> Result<rr::FrozenResearchContext> {
    let store = Arc::new(Store::open(StorePaths {
        data: root.0.join("data"),
        cache: root.0.join("cache"),
        credentials: root.0.join("credentials"),
    })?);
    let ledger = c::Ledger::new(store.clone());
    let conversation = Uuid::new_v4();
    ledger.create(conversation, Uuid::new_v4(), 1)?;
    let history = turn(
        conversation,
        " Earlier exact history: compare evidence carefully; café.\n",
    );
    ledger.append(
        Uuid::new_v4(),
        ledger.expected(Namespace::Conversation, conversation)?,
        history,
        vec![],
    )?;
    let mut request=turn(conversation,"  What exact Avogadro value defines one mole? Compare the authored source, my older associative note, and supplied synthetic research. Explicitly explain the note's discrepancy and cite all three captured entries.\n");
    let document = Uuid::new_v4();
    let mut image = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        2,
        3,
        image::Rgb([200, 200, 200]),
    ))
    .write_to(&mut image, image::ImageFormat::Png)?;
    let bytes = image.into_inner();
    let media = Media {
        sha256: digest(&bytes),
        bytes: bytes.len() as u64,
        media_type: "image/png".into(),
    };
    ledger.stage_image(&media, bytes.as_slice())?;
    let source = c::ImageUse {
        id: Uuid::new_v4(),
        conversation,
        turn: request.id,
        image: media,
        observation: c::SourceObservation {
            document,
            page: Uuid::new_v4(),
            session: "authored-example".into(),
            visit: "authored-example".into(),
            content_revision: Some("authored-not-native".into()),
            order_revision: None,
            capability_revision: "authored".into(),
            evidence_procedure: "synthetic-marker-not-handwriting".into(),
        },
        width: 2,
        height: 3,
        purpose: "authored marker only, not handwriting or native evidence".into(),
        ordinal: 0,
        parent: None,
        parent_dimensions: None,
        crop: None,
        transform: [1., 0., 0., 1., 0., 0.],
        target: None,
        captured_ms: 1,
    };
    request.sources = vec![source.id];
    let request_id = request.id;
    ledger.append(
        Uuid::new_v4(),
        ledger.expected(Namespace::Conversation, conversation)?,
        request,
        vec![source.clone()],
    )?;
    let text="Authored source fixture: the SI mole contains exactly 6.02214076 x 10^23 specified elementary entities; this fixed number is the numerical Avogadro constant in mol^-1.";
    let index = Index::build(NormalizedDocument {
        document,
        format: SourceFormat::Pdf,
        source_sha256: digest(text.as_bytes()),
        extractor_revision: "authored-answer-fixture-v1".into(),
        units: vec![SourceUnit {
            page: Some(1),
            section: Some("Authored SI fixture".into()),
            text: text.into(),
        }],
    })?;
    let profile = Uuid::new_v4();
    let memory = m::LocalMemory::new_local(store, profile)?;
    m::Provider::put(&memory,m::Mutation{operation:Uuid::new_v4(),expected:BTreeSet::new(),fact:m::Fact{key:m::Key{profile,scope:m::Scope::Document{document},fact:Uuid::new_v4()},claim:"older mole note".into(),value:"Older authored associative note: Avogadro value for a mole is 6.02214076 x 10^22. This note is deliberately stale/wrong for the fixture.".into(),subject:"mole Avogadro".into(),tags:vec![],provenance:m::Provenance{basis:m::Basis::UserConfirmed,description:"Authored older note; confirmation is not source verification".into(),conversation:None,turn:None,source:None},confidence_basis_points:9000,observed_ms:10,expires_ms:None}})?;
    let budget = cm::CombinedBudget {
        max_encoded_bytes: 524288,
        max_tokens: 1_000_000,
    };
    let prior = ledger
        .source_request_context(
            conversation,
            request_id,
            None,
            RequestSource::Original {
                origin_turn: request_id,
                source_record: source.id,
                expected: index.key().clone(),
            },
            ReceivingBudget {
                context: c::ContextBudget {
                    max_text_bytes: 65536,
                    max_turns: 64,
                    provider_token_limit: Some(1_000_000),
                },
                max_encoded_bytes: 524288,
                max_passages: 2,
                max_passage_bytes: 16384,
                neighbors: false,
            },
            Some(&index),
            |s| Ok(s.len()),
        )?
        .with_memory(
            cm::MemoryInput::Query {
                provider: &memory,
                profile,
                options: cm::MemoryOptions {
                    now_ms: 100,
                    current_claims: vec![],
                    max_matches: 8,
                    max_context_bytes: 16384,
                },
            },
            budget.clone(),
            |s| Ok(s.len()),
        )?;
    ensure!(
        prior.memory().context().is_some_and(|c| c.facts.len() == 1),
        "authored memory fixture missing"
    );
    prior.with_research(
        rr::ResearchInput {
            policy: rr::ResearchPolicy::Required,
            directive: rr::ResearchDirective::UsePolicy,
            provider: Some(&SyntheticResearch),
            options: rr::ResearchOptions {
                now_ms: 100,
                max_queries: 2,
                max_sources: 2,
                max_evidence_bytes: 16384,
            },
        },
        budget,
        |s| Ok(s.len()),
    )
}
struct Observed<'a> {
    inner: OpenAI,
    current: Vec<Value>,
    sent: Option<Vec<Value>>,
    returned: Option<String>,
    response_digest: Option<String>,
    response_bytes: Option<usize>,
    calls: &'a Cell<usize>,
    clears: usize,
}
impl LLMEngine for Observed<'_> {
    fn add_text_content(&mut self, s: &str) {
        self.current.push(json!({"type":"text","text":s}));
        self.inner.add_text_content(s);
    }
    fn add_image_content(&mut self, s: &str) {
        self.current.push(json!({"type":"image_url","image_url":s}));
        self.inner.add_image_content(s);
    }
    fn clear_content(&mut self) {
        self.clears += 1;
        self.current.clear();
        self.inner.clear_content();
    }
    fn execute(&mut self) -> Result<String> {
        self.execute_with_progress(&mut || Ok(()))
    }
    fn execute_with_progress(&mut self, p: &mut dyn FnMut() -> Result<()>) -> Result<String> {
        ensure!(
            self.calls.replace(self.calls.get() + 1) == 0,
            "one answering invocation maximum"
        );
        self.sent = Some(self.current.clone());
        let text = self.inner.execute_with_progress(p)?;
        self.response_digest = Some(digest(text.as_bytes()));
        self.response_bytes = Some(text.len());
        if text.len() <= 65536 {
            self.returned = Some(text.clone());
        }
        Ok(text)
    }
}
fn selection(args: &Args) -> Result<(String, String)> {
    ensure!(
        (1..=90).contains(&args.timeout_seconds)
            && !args.model.trim().is_empty()
            && args.model.len() <= 128
            && !args.model.chars().any(char::is_control),
        "invalid answering development selection"
    );
    if Path::new(".env").exists() {
        dotenv::from_path(".env").map_err(|_| anyhow::anyhow!("local credential setup failed"))?;
    }
    let base = match &args.base_url {
        Some(base) => base.clone(),
        None => {
            ensure!(
                std::env::var("OPENAI_BASE_URL")
                    .is_ok_and(|s| s.trim_end_matches('/') == "https://api.openai.com")
                    || std::env::var_os("OPENAI_BASE_URL").is_none(),
                "explicit answering base required for configured provider"
            );
            "https://api.openai.com".into()
        }
    };
    ensure!(
        base.len() <= 2048 && !base.chars().any(|c| c.is_control() || c.is_whitespace()),
        "invalid answering development selection"
    );
    let url = reqwest::Url::parse(&base)
        .map_err(|_| anyhow::anyhow!("invalid answering development selection"))?;
    let local = url
        .host_str()
        .and_then(|h| {
            h.trim_matches(&['[', ']'][..])
                .parse::<std::net::IpAddr>()
                .ok()
        })
        .is_some_and(|ip| ip.is_loopback());
    ensure!(
        (url.scheme() == "https" || (url.scheme() == "http" && local))
            && url.host_str().is_some()
            && matches!(url.path(), "" | "/")
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "invalid answering development selection"
    );
    let key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY unavailable"))?;
    ensure!(
        !key.is_empty()
            && key.len() <= 8192
            && key.is_ascii()
            && !key.chars().any(|c| c.is_control() || c.is_whitespace()),
        "invalid answering development selection"
    );
    Ok((url.as_str().trim_end_matches('/').into(), key))
}
fn run(args: &Args, calls: &Cell<usize>) -> Result<Value> {
    let (base, key) = selection(args)?;
    let root = TemporaryRoot::create()?;
    let context = frozen(&root)?;
    let context_sha = digest(context.encoded_json().as_bytes());
    let mut engine = Observed {
        inner: OpenAI::new(args.model.clone(), key, Some(base))
            .with_timeout(Duration::from_secs(args.timeout_seconds)),
        current: vec![],
        sent: None,
        returned: None,
        response_digest: None,
        response_bytes: None,
        calls,
        clears: 0,
    };
    let started = Instant::now();
    let result = a::answer(
        &context,
        &mut engine,
        a::AnswerOptions {
            max_tokens: args.max_counted_bytes,
            max_text_bytes: args.max_answer_bytes,
            ..Default::default()
        },
        |s| Ok(s.len()),
        &mut |_| Ok(()),
    );
    let prompt = engine
        .sent
        .as_ref()
        .map(|parts| serde_json::to_vec(parts).expect("bounded observed text parts"));
    let cleanup = engine.current.is_empty() && engine.clears == 2;
    let (mut draft, mut error, mut checks) = (Value::Null, Value::Null, Value::Null);
    let mut passed = false;
    match result {
        Ok(answer) => {
            let decoded: Value = serde_json::from_str(answer.encoded_json())?;
            let compact = answer
                .text()
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
                .to_ascii_lowercase();
            let ids: Vec<_> = decoded["citations"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|v| v["id"].as_str())
                .collect();
            let value = compact.contains("6.02214076");
            let exponent = ["10^23", "10^{23}", "10²³", "e23", "e+23", "10**23"]
                .iter()
                .any(|s| compact.contains(s));
            let note = compact.contains("note") || compact.contains("memory");
            let refs = ["S1", "M1", "R1"].iter().all(|id| ids.contains(id));
            let correlated = answer.context_sha256() == context_sha
                && prompt
                    .as_ref()
                    .is_some_and(|p| digest(p) == answer.prompt_sha256());
            checks = json!({"fixed_value":value,"exponent_23":exponent,"mentions_note":note,"all_three_captured_references":refs,"hash_correlation":correlated,"engine_content_cleared":cleanup});
            passed = answer.status() == a::Status::Answered
                && value
                && exponent
                && note
                && refs
                && correlated
                && cleanup;
            draft = decoded;
            if !passed {
                error = json!("representative answering assertions failed");
            }
        }
        Err(e) => {
            error = json!(e.to_string());
        }
    }
    Ok(
        json!({"status":if passed{"ok"}else{"failed"},"engine_invocations":calls.get(),"model":args.model,"elapsed_ms":started.elapsed().as_millis(),"counter_basis":"declared-utf8-byte-count-not-model-tokens","context_sha256":context_sha,"context_json":context.encoded_json(),"prompt_sha256":prompt.as_ref().map(|p|digest(p)),"response_sha256":engine.response_digest,"response_bytes":engine.response_bytes,"raw_model_response_base64":engine.returned.as_ref().map(|s|STANDARD.encode(s.as_bytes())),"engine_content_cleared":cleanup,"draft":draft,"fixture_assertions":checks,"error":error,"research_basis":"synthetic-authored-fixture-not-hosted-acquisition"}),
    )
}
fn private_file(path: &Path) -> Result<File> {
    ensure!(path.is_absolute(), "private output must be absolute");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|_| anyhow::anyhow!("new private evidence file unavailable"))
}
fn main() -> Result<()> {
    let args = Args::parse();
    let mut file = private_file(&args.output)?;
    let calls = Cell::new(0);
    let record = run(&args, &calls).unwrap_or_else(|e| {
        let error = match e.to_string().as_str() {
            "OPENAI_API_KEY unavailable" => "OPENAI_API_KEY unavailable",
            "explicit answering base required for configured provider" => {
                "explicit answering base required for configured provider"
            }
            "invalid answering development selection" => "invalid answering development selection",
            "local credential setup failed" => "local credential setup failed",
            _ => "development admission failed",
        };
        json!({"status":"failed","engine_invocations":calls.get(),"error":error,"draft":null})
    });
    serde_json::to_writer_pretty(&mut file, &record)?;
    file.write_all(b"\n")?;
    file.flush()?;
    let ok = record["status"] == "ok";
    println!(
        "Answer development check {}; private evidence saved",
        if ok { "passed" } else { "failed" }
    );
    ensure!(ok, "answer development check failed");
    Ok(())
}

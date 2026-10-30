//! Explicit one-POST hosted acquisition check using public authored context, never tablet access.
use anyhow::{ensure, Result};
use base64::Engine;
use clap::Parser;
use remarkable_reader_buddy::{
    conversation::{
        self as c,
        receiving::{
            memory::{self as cm, research as rr},
            ReceivingBudget, RequestSource,
        },
    },
    llm::openai::DEFAULT_MODEL,
    storage::{digest, Namespace, Store, StorePaths, Uuid},
};
use rr::openai::{OpenAIWebResearch, TransportOptions};
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
#[derive(Parser)]
struct Args {
    /// Explicitly enables a paid hosted request; omitted means no credential lookup or request.
    #[arg(long, required = true)]
    live: bool,
    /// New absolute private evidence file; existing files are never overwritten.
    #[arg(long)]
    output: PathBuf,
    #[arg(long,default_value=DEFAULT_MODEL)]
    model: String,
    #[arg(long)]
    endpoint: Option<String>,
    #[arg(long, default_value_t = 90)]
    timeout_seconds: u64,
    /// Final research-context counter is explicitly UTF8 bytes, not a model-window token claim.
    #[arg(long, default_value_t = 524288)]
    max_counted_bytes: usize,
}
struct TemporaryRoot(PathBuf);
impl TemporaryRoot {
    fn create() -> Result<Self> {
        let p = std::env::temp_dir().join(format!("buddy-web-check-{}", Uuid::new_v4()));
        fs::create_dir(&p)?;
        Ok(Self(p))
    }
}
impl Drop for TemporaryRoot {
    fn drop(&mut self) {
        if let Ok(p) = self.0.canonicalize() {
            if let Ok(parent) = std::env::temp_dir().canonicalize() {
                if p.parent() == Some(parent.as_path()) && p.file_name() == self.0.file_name() {
                    let _ = fs::remove_dir_all(p);
                }
            }
        }
    }
}
fn frozen(root: &TemporaryRoot) -> Result<cm::FrozenSourceMemoryContext> {
    let store = Arc::new(Store::open(StorePaths {
        data: root.0.join("data"),
        cache: root.0.join("cache"),
        credentials: root.0.join("credentials"),
    })?);
    let ledger = c::Ledger::new(store);
    let conversation = Uuid::new_v4();
    ledger.create(conversation, Uuid::new_v4(), 1)?;
    let turn=c::Turn{id:Uuid::new_v4(),conversation,exchange:Uuid::new_v4(),sequence:0,role:c::Role::User,mode:c::Mode::Reader,outcome:c::Outcome::Interpreted,
  text:Some("Verify the SI definition of the mole against official BIPM or NIST documentation. Summarize the definition and cite the consulted official source. This is an authored public development check.".into()),sources:vec![],correction_of:None,created_ms:1,updated_ms:1,completion:None};
    let turn_id = turn.id;
    ledger.append(
        Uuid::new_v4(),
        ledger.expected(Namespace::Conversation, conversation)?,
        turn,
        vec![],
    )?;
    let receiving = ReceivingBudget {
        context: c::ContextBudget {
            max_text_bytes: 65536,
            max_turns: 64,
            provider_token_limit: Some(1_000_000),
        },
        max_encoded_bytes: 524288,
        max_passages: 2,
        max_passage_bytes: 16384,
        neighbors: false,
    };
    let context = ledger.source_request_context(
        conversation,
        turn_id,
        None,
        RequestSource::NoDocument,
        receiving,
        None,
        |text| Ok(text.len()),
    )?;
    context.with_memory(
        cm::MemoryInput::Disabled,
        cm::CombinedBudget {
            max_encoded_bytes: 524288,
            max_tokens: 1_000_000,
        },
        |text| Ok(text.len()),
    )
}
struct OneAttempt<'a> {
    inner: &'a dyn rr::Provider,
    calls: Cell<usize>,
    error: RefCell<Option<String>>,
}
impl rr::Provider for OneAttempt<'_> {
    fn research(&self, r: &rr::ResearchRequest<'_>) -> Result<rr::ResearchResult> {
        ensure!(
            self.calls.replace(self.calls.get() + 1) == 0,
            "development one-attempt limit"
        );
        self.inner.research(r).inspect_err(|e| {
            *self.error.borrow_mut() = Some(e.to_string());
        })
    }
}
fn run(args: &Args) -> Result<serde_json::Value> {
    ensure!(
        args.live && (1..=90).contains(&args.timeout_seconds) && args.max_counted_bytes > 0,
        "invalid development bounds"
    );
    // Explicit current-directory protected setup only; never search parent projects for credentials.
    if std::path::Path::new(".env").exists() {
        dotenv::from_path(".env").map_err(|_| anyhow::anyhow!("local credential setup failed"))?;
    }
    let endpoint = match &args.endpoint {
        Some(endpoint) => endpoint.clone(),
        None => {
            ensure!(
                std::env::var("OPENAI_BASE_URL")
                    .is_ok_and(|base| base.trim_end_matches('/') == "https://api.openai.com")
                    || std::env::var_os("OPENAI_BASE_URL").is_none(),
                "explicit research endpoint required for configured provider"
            );
            "https://api.openai.com/v1/responses".to_owned()
        }
    };
    let key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY unavailable"))?;
    let root = TemporaryRoot::create()?;
    let prior = frozen(&root)?;
    let prior_sha = digest(prior.encoded_json().as_bytes());
    let mut progress = |_| Ok(());
    let backend = OpenAIWebResearch::new(
        args.model.clone(),
        endpoint,
        key,
        TransportOptions {
            timeout: Duration::from_secs(args.timeout_seconds),
            ..Default::default()
        },
        &mut progress,
    )?
    .with_exchange_observation();
    let observed = OneAttempt {
        inner: &backend,
        calls: Cell::new(0),
        error: RefCell::new(None),
    };
    let now_ms = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
    let start = Instant::now();
    let result = prior.with_research(
        rr::ResearchInput {
            policy: rr::ResearchPolicy::Required,
            directive: rr::ResearchDirective::UsePolicy,
            provider: Some(&observed),
            options: rr::ResearchOptions {
                now_ms,
                max_queries: 4,
                max_sources: 8,
                max_evidence_bytes: 16384,
            },
        },
        cm::CombinedBudget {
            max_encoded_bytes: 524288,
            max_tokens: args.max_counted_bytes,
        },
        |text| Ok(text.len()),
    );
    match result {
        Ok(context) => {
            ensure!(
                digest(context.source_memory_context().encoded_json().as_bytes()) == prior_sha,
                "captured context changed"
            );
            let exchange = backend
                .take_exchange_observation()
                .ok_or_else(|| anyhow::anyhow!("successful adapter observation absent"))?;
            Ok(
                json!({"status":"ok","provider_calls":observed.calls.get(),"elapsed_ms":start.elapsed().as_millis(),"model":args.model,"counter_basis":"declared-utf8-byte-count-not-model-tokens","context_sha256":prior_sha,"composition_sha256":digest(context.encoded_json().as_bytes()),"research":context.research(),"exchange":{"request_sha256":exchange.request_sha256(),"response_sha256":exchange.response_sha256(),"identity":exchange.identity(),"response_base64":base64::engine::general_purpose::STANDARD.encode(exchange.response_bytes())}}),
            )
        }
        Err(error) => {
            // Adapter success is not complete composition success: discard even a valid transient body.
            drop(backend.take_exchange_observation());
            Ok(
                json!({"status":"failed","provider_calls":observed.calls.get(),"elapsed_ms":start.elapsed().as_millis(),"error":observed.error.borrow().as_deref().unwrap_or("final composition refused"),"composition_error":error.to_string(),"exchange":null}),
            )
        }
    }
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
    let record = run(&args).unwrap_or_else(|error| {
        let message = match error.to_string().as_str() {
            "OPENAI_API_KEY unavailable" => "OPENAI_API_KEY unavailable",
            "explicit research endpoint required for configured provider" => "explicit research endpoint required for configured provider",
            "local credential setup failed" => "local credential setup failed",
            "invalid web research credential" => "invalid web research credential",
            "invalid web research selection" => "invalid web research selection",
            _ => "development admission failed",
        };
        let before_dispatch = message != "development admission failed";
        json!({"status":"failed","error":message,"provider_calls":before_dispatch.then_some(0usize),"exchange":null})
    });
    serde_json::to_writer_pretty(&mut file, &record)?;
    file.write_all(b"\n")?;
    file.flush()?;
    let ok = record["status"] == "ok";
    println!(
        "Research development check {}; private evidence saved",
        if ok { "passed" } else { "failed" }
    );
    ensure!(ok, "research development check failed");
    Ok(())
}

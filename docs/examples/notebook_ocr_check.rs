//! Explicit historical PNG -> unverified OCR -> Index -> owned cache reopen/query.
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use clap::Parser;
use remarkable_reader_buddy::{
    llm::{
        openai::{OpenAI, DEFAULT_MODEL},
        LLMEngine,
    },
    retrieval::{
        disk::DiskCache,
        notebook::{self, Options, PageImage},
        Index, Provider,
    },
    storage::{digest, Store, StorePaths, Uuid},
};
use serde_json::{json, Value};
use std::{
    cell::Cell,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Parser)]
struct Args {
    #[arg(long, required = true)]
    live: bool,
    #[arg(long)]
    output: PathBuf,
    #[arg(long, required = true)]
    image: Vec<PathBuf>,
    #[arg(long, required = true)]
    page: Vec<u32>,
    #[arg(long, required = true)]
    query: Vec<String>,
    #[arg(long,default_value=DEFAULT_MODEL)]
    model: String,
    #[arg(long)]
    base_url: Option<String>,
    #[arg(long, default_value_t = 90)]
    timeout_seconds: u64,
    #[arg(long, default_value_t = 25165824)]
    max_input_bytes: usize,
    #[arg(long, default_value_t = 32768)]
    max_page_text_bytes: usize,
    /// Development cancellation control; never enabled implicitly.
    #[arg(long)]
    cancel_after_ms: Option<u64>,
}
struct TemporaryRoot(PathBuf);
impl TemporaryRoot {
    fn create() -> Result<Self> {
        let p = std::env::temp_dir().join(format!("buddy-notebook-check-{}", Uuid::new_v4()));
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
struct Observed<'a> {
    inner: OpenAI,
    current: Vec<Value>,
    prompt_sha: Option<String>,
    prompt_bytes: Option<usize>,
    returned: Option<String>,
    response_sha: Option<String>,
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
            "one OCR invocation maximum"
        );
        let descriptor = serde_json::to_vec(&self.current)?;
        self.prompt_sha = Some(digest(&descriptor));
        self.prompt_bytes = Some(descriptor.len());
        let returned = self.inner.execute_with_progress(p)?;
        self.response_sha = Some(digest(returned.as_bytes()));
        if returned.len() <= 128 * 1024 {
            self.returned = Some(returned.clone());
        }
        Ok(returned)
    }
}
fn selection(args: &Args) -> Result<(String, String)> {
    ensure!(
        (1..=90).contains(&args.timeout_seconds)
            && !args.model.trim().is_empty()
            && args.model.len() <= 128
            && !args.model.chars().any(char::is_control)
            && args.cancel_after_ms.is_none_or(|n| n <= 90_000),
        "invalid development selection"
    );
    if Path::new(".env").exists() {
        dotenv::from_path(".env").map_err(|_| anyhow::anyhow!("credential setup refused"))?;
    }
    let base = match &args.base_url {
        Some(b) => b.clone(),
        None => {
            ensure!(
                std::env::var_os("OPENAI_BASE_URL").is_none()
                    || std::env::var("OPENAI_BASE_URL")
                        .is_ok_and(|b| b.trim_end_matches('/') == "https://api.openai.com"),
                "explicit base required for configured provider"
            );
            "https://api.openai.com".into()
        }
    };
    ensure!(
        base.len() <= 2048 && !base.chars().any(|c| c.is_control() || c.is_whitespace()),
        "invalid development selection"
    );
    let url =
        reqwest::Url::parse(&base).map_err(|_| anyhow::anyhow!("invalid development selection"))?;
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
        "invalid development selection"
    );
    let key =
        std::env::var("OPENAI_API_KEY").map_err(|_| anyhow::anyhow!("credential unavailable"))?;
    ensure!(
        !key.is_empty()
            && key.len() <= 8192
            && key.is_ascii()
            && !key.chars().any(|c| c.is_control() || c.is_whitespace()),
        "invalid development selection"
    );
    Ok((url.as_str().trim_end_matches('/').into(), key))
}
fn run(args: &Args, calls: &Cell<usize>) -> Result<Value> {
    ensure!(
        !args.image.is_empty()
            && args.image.len() <= 4
            && args.image.len() == args.page.len()
            && !args.query.is_empty()
            && args.query.len() <= 8
            && args.query.iter().all(|q| !q.trim().is_empty()
                && q.len() <= 512
                && !q.chars().any(char::is_control)),
        "invalid development inputs"
    );
    let (base, key) = selection(args)?;
    let mut bytes = Vec::new();
    let mut total = 0;
    for path in &args.image {
        let mut b = Vec::new();
        File::open(path)?
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut b)?;
        total += b.len();
        ensure!(
            b.len() <= 8 * 1024 * 1024 && total <= 16 * 1024 * 1024,
            "image read exceeds bound"
        );
        bytes.push(b);
    }
    let pages = args
        .page
        .iter()
        .zip(&bytes)
        .map(|(page, png)| PageImage { page: *page, png })
        .collect::<Vec<_>>();
    let mut engine = Observed {
        inner: OpenAI::new(args.model.clone(), key, Some(base))
            .with_timeout(Duration::from_secs(args.timeout_seconds)),
        current: vec![],
        prompt_sha: None,
        prompt_bytes: None,
        returned: None,
        response_sha: None,
        calls,
        clears: 0,
    };
    let started = Instant::now();
    let result = notebook::normalize(
        Uuid::new_v4(),
        &pages,
        &args.model,
        &mut engine,
        Options {
            max_input_bytes: args.max_input_bytes,
            max_page_text_bytes: args.max_page_text_bytes,
            ..Default::default()
        },
        &mut |_| {
            ensure!(
                args.cancel_after_ms
                    .is_none_or(|ms| started.elapsed() < Duration::from_millis(ms)),
                "development cancellation"
            );
            Ok(())
        },
    );
    let cleanup = engine.current.is_empty() && engine.clears == 2;
    let pipeline=result.and_then(|normalized|{
        let index=Index::build(normalized.document.clone())?;let expected=index.key().clone();drop(index);
        let root=TemporaryRoot::create()?;let paths=StorePaths{data:root.0.join("data"),cache:root.0.join("cache"),credentials:root.0.join("credentials")};
        {let store=Store::open(paths.clone())?;let cache=DiskCache::open(&store)?;ensure!(cache.install(normalized.document.clone())?==expected,"cache key mismatch");}
        let store=Store::open(paths)?;let cache=DiskCache::open(&store)?;
        let mut retrieved=Vec::new();
        for query in &args.query {let r=cache.query(&expected,query,4,16384,false)?;ensure!(r.source==expected,"query key mismatch");
            for p in &r.passages {let unit=&normalized.document.units[p.unit];ensure!(p.page==unit.page && p.text==unit.text[p.range[0]..p.range[1]],"query range mismatch");}
            retrieved.push(json!({"query":query,"retrieved":r}));
        }
        let prompt_matches=engine.prompt_sha.as_deref()==Some(normalized.prompt_sha256.as_str()) && engine.prompt_bytes==Some(normalized.prompt_bytes);
        ensure!(cleanup && prompt_matches && calls.get()==1,"OCR observation mismatch");
        Ok(json!({"normalization":normalized,"expected_key":expected,"queries":retrieved,"cache_reopened":true,"prompt_matches":prompt_matches}))
    });
    let (status, data) = match pipeline {
        Ok(value) => ("ok", value),
        Err(_) => ("failed", Value::Null),
    };
    Ok(
        json!({"status":status,"model":args.model,"engine_invocations":calls.get(),"elapsed_ms":started.elapsed().as_millis(),"engine_content_cleared":cleanup,"prompt_sha256":engine.prompt_sha,"prompt_bytes":engine.prompt_bytes,"response_sha256":engine.response_sha,"raw_model_response_base64":engine.returned.as_ref().map(|s|STANDARD.encode(s.as_bytes())),"pipeline":data,"authority":"unverified-historical-page-image-ocr; no-native-or-runtime-qualification"}),
    )
}
fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(args.output.is_absolute(), "private output must be absolute");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&args.output)
        .map_err(|_| anyhow::anyhow!("new private output unavailable"))?;
    let calls = Cell::new(0);
    let record=run(&args,&calls).unwrap_or_else(|_|json!({"status":"failed","engine_invocations":calls.get(),"pipeline":null,"error":"development admission failed"}));
    let encoded = serde_json::to_vec_pretty(&record)?;
    ensure!(
        encoded.len() <= 2 * 1024 * 1024,
        "private evidence exceeds bound"
    );
    file.write_all(&encoded)?;
    file.write_all(b"\n")?;
    file.flush()?;
    let ok = record["status"] == "ok";
    println!(
        "Notebook development check {}; private evidence saved",
        if ok { "passed" } else { "failed" }
    );
    ensure!(ok, "notebook development check failed");
    Ok(())
}

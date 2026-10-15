use super::LLMEngine;
use anyhow::{Context, Result};
use log::{debug, info};
use serde_json::json;
use serde_json::Value as JsonValue;

pub const DEFAULT_MODEL: &str = "gpt-5.6-terra";

pub struct OpenAI {
    timeout: std::time::Duration,
    model: String,
    base_url: String,
    api_key: String,
    content: Vec<JsonValue>,
}

impl OpenAI {
    pub fn new(model: String, api_key: String, base_url: Option<String>) -> Self {
        let base_url = base_url.unwrap_or_else(|| "https://api.openai.com".to_string());

        Self {
            timeout: std::time::Duration::from_secs(90),
            model,
            base_url,
            api_key,
            content: Vec::new(),
        }
    }

    pub fn from_env(model: Option<String>) -> Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY environment variable not set"))?;
        anyhow::ensure!(!api_key.trim().is_empty(), "OPENAI_API_KEY is empty");
        let base_url = std::env::var("OPENAI_BASE_URL").ok();
        let model = model.unwrap_or_else(|| DEFAULT_MODEL.to_string());

        Ok(Self::new(model, api_key, base_url))
    }

    /// Bound explicitly requested development calls without changing tablet defaults.
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn add_content(&mut self, content: JsonValue) {
        self.content.push(content);
    }
}

impl LLMEngine for OpenAI {
    fn add_text_content(&mut self, text: &str) {
        self.add_content(json!({
            "type": "text",
            "text": text,
        }));
    }

    fn add_image_content(&mut self, base64_image: &str) {
        self.add_content(json!({
            "type": "image_url",
            "image_url": {
                "url": format!("data:image/png;base64,{}", base64_image),
                "detail": "high"
            }
        }));
    }

    fn clear_content(&mut self) {
        self.content.clear();
    }

    fn execute(&mut self) -> Result<String> {
        self.execute_with_progress(&mut || Ok(()))
    }

    fn execute_with_progress(
        &mut self,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<String> {
        // Refuse before dispatch if the caller has already canceled/lost admission.
        let preflight = std::time::Instant::now();
        progress()?;
        let run = crate::measurement::context();
        let _timing = crate::measurement::Span::new("provider.wait_with_progress");
        let body = json!({
            "model": self.model,
            "messages": [{"role": "user", "content": self.content}],
            "max_completion_tokens": 4000
        });
        debug!(
            "Request: model={} content_parts={}",
            self.model,
            self.content.len()
        );
        let endpoint = format!("{}/v1/chat/completions", self.base_url);
        let api_key = self.api_key.clone();
        let timeout = self.timeout;
        let (cancel, mut canceled) = tokio::sync::watch::channel(false);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::Builder::new().name("buddy-provider-request".into()).spawn(move || {
            let _run = crate::measurement::Run::enter(run);
            let result = (|| -> Result<String> {
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()
                    .context("Create provider runtime")?;
                runtime.block_on(async {
                    let request = async {
                        let _timing = crate::measurement::Span::new("provider.request");
                        let client = reqwest::Client::builder().timeout(timeout).hickory_dns(true).build()
                            .context("Create asynchronous provider transport")?;
                        let mut response = client.post(endpoint).bearer_auth(api_key).json(&body).send().await
                            .map_err(provider_error)?
                            .error_for_status().map_err(|error| error.without_url()).context("Model response status")?;
                        // Preserve the previous bounded response behavior, including unknown lengths.
                        const MAX_RESPONSE: usize = 10 * 1024 * 1024;
                        anyhow::ensure!(response.content_length().is_none_or(|size| size <= MAX_RESPONSE as u64), "Model response exceeds bound");
                        let mut bytes = Vec::new();
                        while let Some(chunk) = response.chunk().await.map_err(provider_error)? {
                            anyhow::ensure!(chunk.len() <= MAX_RESPONSE.saturating_sub(bytes.len()), "Model response exceeds bound");
                            bytes.extend_from_slice(&chunk);
                        }
                        let text = String::from_utf8(bytes).context("Decode model response text")?;
                        parse_response(&text)
                    };
                    tokio::pin!(request);
                    tokio::select! {
                        biased;
                        _ = canceled.changed() => Err(anyhow::anyhow!("Model request canceled")),
                        result = &mut request => result,
                    }
                })
                // Dropping this runtime stops async I/O; no blocking HTTP/DNS task
                // is spawned or leaked via a shutdown timeout/background escape.
            })();
            let _ = sender.send(result);
        }).context("Start provider request")?;
        let mut owned = OwnedRequest {
            cancel,
            worker: Some(worker),
        };
        let mut due = preflight
            + crate::workflow::indicator::next_deadline(
                std::time::Duration::ZERO,
                preflight.elapsed(),
            );
        let result = loop {
            match receiver.recv_timeout(due.saturating_duration_since(std::time::Instant::now())) {
                Ok(result) => break result,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    break Err(anyhow::anyhow!("Model worker disconnected"))
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    let started = std::time::Instant::now();
                    if let Err(error) = progress() {
                        break Err(error);
                    }
                    due = started
                        + crate::workflow::indicator::next_deadline(
                            std::time::Duration::ZERO,
                            started.elapsed(),
                        );
                }
            }
        };
        owned.finish()?;
        result
    }
}

/// Always cancel and join owned request work, including callback unwinding.
struct OwnedRequest {
    cancel: tokio::sync::watch::Sender<bool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl OwnedRequest {
    fn finish(&mut self) -> Result<()> {
        let _ = self.cancel.send(true);
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| anyhow::anyhow!("Model worker panicked"))?;
        }
        Ok(())
    }
}
impl Drop for OwnedRequest {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

fn provider_error(error: reqwest::Error) -> anyhow::Error {
    if error.is_timeout() {
        anyhow::anyhow!("Model request timeout")
    } else {
        anyhow::Error::from(error.without_url()).context("Model request")
    }
}

fn parse_response(body_text: &str) -> Result<String> {
    let json: JsonValue = serde_json::from_str(body_text).context("Decode model response JSON")?;
    info!(
        "API usage: prompt_tokens={:?} completion_tokens={:?} total_tokens={:?}",
        json["usage"]["prompt_tokens"].as_u64(),
        json["usage"]["completion_tokens"].as_u64(),
        json["usage"]["total_tokens"].as_u64()
    );
    debug!("Provider response decoded");

    // Extract the response text
    let response_text = json["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No response content found"))?
        .to_string();

    Ok(response_text)
}

#[cfg(test)]
mod tests {
    use super::parse_response;

    #[test]
    fn response_errors_do_not_panic_or_echo_response_content() {
        for body in [
            "private invalid body",
            "{}",
            r#"{"choices":[{"message":{"content":null}}]}"#,
        ] {
            let error = parse_response(body).unwrap_err().to_string();
            assert!(!error.contains("private invalid body"));
        }
        assert_eq!(
            parse_response(r#"{"choices":[{"message":{"content":"NONE"}}]}"#).unwrap(),
            "NONE"
        );
    }
}

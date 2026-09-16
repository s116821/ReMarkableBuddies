use super::LLMEngine;
use anyhow::{Context, Result};
use log::{debug, info};
use serde_json::json;
use serde_json::Value as JsonValue;

pub const DEFAULT_MODEL: &str = "gpt-5.6-terra";

pub struct OpenAI {
    model: String,
    base_url: String,
    api_key: String,
    content: Vec<JsonValue>,
}

impl OpenAI {
    pub fn new(model: String, api_key: String, base_url: Option<String>) -> Self {
        let base_url = base_url.unwrap_or_else(|| "https://api.openai.com".to_string());

        Self {
            model,
            base_url,
            api_key,
            content: Vec::new(),
        }
    }

    pub fn from_env(model: Option<String>) -> Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY environment variable not set"))?;
        let base_url = std::env::var("OPENAI_BASE_URL").ok();
        let model = model.unwrap_or_else(|| DEFAULT_MODEL.to_string());

        Ok(Self::new(model, api_key, base_url))
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
        let body = json!({
            "model": self.model,
            "messages": [{
                "role": "user",
                "content": self.content
            }],
            "max_completion_tokens": 4000
        });

        debug!(
            "Request: model={} content_parts={}",
            self.model,
            self.content.len()
        );
        let raw_response = ureq::post(format!("{}/v1/chat/completions", self.base_url).as_str())
            .header("Authorization", &format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .send_json(&body);

        let mut response = match raw_response {
            Ok(response) => response,
            Err(err) => {
                info!("API Error: {}", err);
                return Err(anyhow::anyhow!("API ERROR: {}", err));
            }
        };

        // Read response body as string
        let body_text = response
            .body_mut()
            .read_to_string()
            .context("Read model response")?;
        parse_response(&body_text)
    }
}

fn parse_response(body_text: &str) -> Result<String> {
    let json: JsonValue = serde_json::from_str(body_text).context("Decode model response JSON")?;
    info!("API usage: model={} usage={}", json["model"], json["usage"]);
    debug!("Response: {}", json);

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

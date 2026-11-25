//! Ollama client (feature `ollama`): `POST /api/chat` with a JSON-Schema
//! `format`, temperature 0, non-streaming. Only loopback base URLs are allowed
//! so meeting content can never leave the machine through this client.

use std::net::IpAddr;
use std::time::Duration;

use reqwest::Url;
use serde_json::{Value, json};

use crate::prompt::REPAIR_MESSAGE;
use crate::schema::EnhanceOutput;
use crate::{EnhanceError, ExtractClient};

pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434";
pub const DEFAULT_MODEL: &str = "gemma4:26b-mlx";

#[derive(Debug, Clone)]
pub struct OllamaClient {
    chat_url: Url,
    model: String,
    num_ctx: usize,
    keep_alive: String,
    http: reqwest::Client,
}

/// Accept only `http(s)://localhost`, `127.0.0.0/8` or `[::1]`.
pub fn parse_loopback_base_url(base_url: &str) -> Result<Url, EnhanceError> {
    let mut url = Url::parse(base_url.trim())
        .map_err(|e| EnhanceError::Client(format!("invalid Ollama URL: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(EnhanceError::Client(
            "Ollama URL must be http or https".into(),
        ));
    }
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_matches(['[', ']'])
        .to_owned();
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
    if !loopback {
        return Err(EnhanceError::Client(format!(
            "refusing non-loopback Ollama host `{host}`: notes stay on this machine"
        )));
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

impl OllamaClient {
    pub fn new(base_url: &str, model: &str, num_ctx: usize) -> Result<Self, EnhanceError> {
        let base = parse_loopback_base_url(base_url)?;
        let chat_url = base
            .join("api/chat")
            .map_err(|e| EnhanceError::Client(e.to_string()))?;
        let http = reqwest::Client::builder()
            // Never route local model traffic through an HTTP(S) proxy.
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15 * 60))
            .build()
            .map_err(|e| EnhanceError::Client(e.to_string()))?;
        Ok(Self {
            chat_url,
            model: model.to_owned(),
            num_ctx,
            keep_alive: "10m".into(),
            http,
        })
    }

    pub fn with_keep_alive(mut self, keep_alive: &str) -> Self {
        self.keep_alive = keep_alive.to_owned();
        self
    }

    fn body(&self, messages: &[Value], schema: &Value) -> Value {
        json!({
            "model": self.model,
            "messages": messages,
            "format": schema,
            "stream": false,
            "think": false,
            "options": {"temperature": 0, "num_ctx": self.num_ctx},
            "keep_alive": self.keep_alive,
        })
    }

    async fn chat(&self, messages: &[Value], schema: &Value) -> Result<String, EnhanceError> {
        let response = self
            .http
            .post(self.chat_url.clone())
            .json(&self.body(messages, schema))
            .send()
            .await
            .map_err(|e| EnhanceError::Client(format!("could not reach Ollama: {e}")))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| EnhanceError::Client(e.to_string()))?;
        if !status.is_success() {
            let snippet: String = text.chars().take(300).collect();
            return Err(EnhanceError::Client(format!(
                "Ollama returned {status}: {snippet}"
            )));
        }
        let value: Value = serde_json::from_str(&text)
            .map_err(|e| EnhanceError::Client(format!("unreadable Ollama response: {e}")))?;
        Ok(value["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_owned())
    }
}

fn parse_output(content: &str) -> Result<EnhanceOutput, String> {
    let trimmed = content.trim();
    let trimmed = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|s| s.trim_end().trim_end_matches("```").trim())
        .unwrap_or(trimmed);
    serde_json::from_str(trimmed).map_err(|e| e.to_string())
}

impl ExtractClient for OllamaClient {
    fn model_name(&self) -> String {
        self.model.clone()
    }

    async fn extract(
        &self,
        system: &str,
        user: &str,
        schema: &Value,
    ) -> Result<EnhanceOutput, EnhanceError> {
        let mut messages = vec![
            json!({"role": "system", "content": system}),
            json!({"role": "user", "content": user}),
        ];
        let first = self.chat(&messages, schema).await?;
        let first_error = match parse_output(&first) {
            Ok(output) => return Ok(output),
            Err(e) => e,
        };
        // One repair attempt: show the model its own invalid answer.
        messages.push(json!({"role": "assistant", "content": first}));
        messages.push(json!({"role": "user", "content": REPAIR_MESSAGE}));
        let second = self.chat(&messages, schema).await?;
        parse_output(&second).map_err(|e| {
            EnhanceError::BadModelOutput(format!("invalid JSON twice ({first_error}; then {e})"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_urls_are_accepted() {
        for ok in [
            "http://127.0.0.1:11434",
            "http://localhost:11434/",
            "http://[::1]:11434",
            "http://127.0.0.2",
        ] {
            assert!(parse_loopback_base_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://192.168.1.5:11434",
            "https://api.example.com",
            "http://10.0.0.1",
            "http://localhost.evil.com",
            "file:///etc/passwd",
            "not a url",
        ] {
            assert!(parse_loopback_base_url(bad).is_err(), "{bad}");
        }
        assert!(OllamaClient::new("http://example.com", "m", 1024).is_err());
    }

    #[test]
    fn parses_fenced_json() {
        assert!(parse_output("```json\n{\"sections\":[]}\n```").is_ok());
        assert!(parse_output("nope").is_err());
    }
}

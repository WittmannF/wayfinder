//! The model that writes pages, behind one function so the priced pass is
//! testable without a network.

use std::time::Duration;
use wayfinder_core::{PageAnswer, PageRequest, PageWriter};

/// What an OpenAI-compatible endpoint needs to answer.
#[derive(Debug, Clone)]
pub struct Endpoint {
    /// Full URL of the chat-completions endpoint.
    pub url: String,
    /// Model name to request.
    pub model: String,
    /// Bearer token.
    pub key: String,
    /// Request timeout in seconds.
    pub timeout_seconds: u64,
}

impl Endpoint {
    /// Read an endpoint from flags and the environment.
    /// @param url - endpoint URL from `--endpoint`
    /// @param model - model from `--model`, falling back to `WAYFINDER_MODEL`
    /// @returns the endpoint, or `None` when no model is configured
    pub fn resolve(url: &str, model: Option<&str>) -> Option<Self> {
        let model = model
            .map(str::to_owned)
            .or_else(|| std::env::var("WAYFINDER_MODEL").ok())
            .filter(|value| !value.trim().is_empty())?;
        let key = std::env::var("WAYFINDER_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .or_else(|_| std::env::var("DEEPSEEK_API_KEY"))
            .unwrap_or_default();
        Some(Self {
            url: url.to_owned(),
            model,
            key,
            timeout_seconds: 240,
        })
    }
}

/// A page writer that calls one chat-completions endpoint.
pub struct HttpPageWriter {
    endpoint: Endpoint,
}

impl HttpPageWriter {
    /// Build a writer over one endpoint.
    /// @param endpoint - URL, model, and key
    pub fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }
}

impl PageWriter for HttpPageWriter {
    fn write(&self, request: &PageRequest) -> wayfinder_core::Result<PageAnswer> {
        let body = serde_json::json!({
            "model": self.endpoint.model,
            "temperature": 0.2,
            "messages": [
                {
                    "role": "system",
                    "content": "You write memory pages for a codebase. You state what the files show, \
        name the files you relied on, and never invent behavior you did not read.",
                },
                { "role": "user", "content": request.prompt },
            ],
        });
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(self.endpoint.timeout_seconds)))
            .build();
        let agent = ureq::Agent::new_with_config(config);
        let mut call = agent
            .post(&self.endpoint.url)
            .header("content-type", "application/json");
        if !self.endpoint.key.is_empty() {
            call = call.header("authorization", format!("Bearer {}", self.endpoint.key));
        }
        let mut response = call.send_json(body).map_err(|error| {
            wayfinder_core::Error::Invalid(format!("model call failed: {error}"))
        })?;
        let document: serde_json::Value = response.body_mut().read_json().map_err(|error| {
            wayfinder_core::Error::Invalid(format!("model answer was not JSON: {error}"))
        })?;
        let body = document["choices"][0]["message"]["content"]
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| {
                wayfinder_core::Error::Invalid(format!(
                    "model answer carried no content: {}",
                    document
                ))
            })?;
        let tokens = document["usage"]["total_tokens"].as_u64().unwrap_or(0);
        Ok(PageAnswer {
            body: strip_fences(body),
            tokens,
        })
    }
}

/// Drop a fence a model wrapped around the whole answer.
fn strip_fences(body: &str) -> String {
    let trimmed = body.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = rest.split_once('\n').map(|(_, rest)| rest).unwrap_or(rest);
        if let Some(inner) = rest.strip_suffix("```") {
            return inner.trim().to_owned();
        }
    }
    trimmed.to_owned()
}

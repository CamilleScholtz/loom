pub mod builders;
pub mod fake;
pub mod interpret;
pub mod openrouter;
pub mod prompts;
pub mod stream;

use anyhow::Result;

pub use fake::FakeClient;
pub use openrouter::OpenRouterClientImpl;
#[allow(unused_imports)]
pub use stream::{StreamChunk, StreamReceiver};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }
}

/// Schema definition for a forced tool call. When attached to a request, the
/// model is constrained to respond by calling `name` with arguments matching
/// `parameters_schema`. `force` makes the choice mandatory.
#[derive(Clone, Debug)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters_schema: serde_json::Value,
    pub force: bool,
}

#[derive(Clone, Debug)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    /// Optional forced tool call. When `Some`, the OpenRouter request adds
    /// the tool to the chat completion and (if `force` is set) constrains the
    /// model to invoke it. `None` is identical to plain prose completion.
    pub tool: Option<ToolDef>,
}

pub trait Client: Send + Sync {
    fn chat(&self, request: ChatRequest) -> Result<String>;
    fn chat_stream(&self, request: ChatRequest) -> Result<StreamReceiver>;
}

/// Drive a streaming chat to completion and return the concatenated text. For
/// forced tool calls this returns the accumulated tool-args JSON; for plain
/// completions it returns the accumulated content. The OpenRouter `chat()`
/// path surfaces only message content — tool-call arguments live behind
/// `partial_tool_calls` on the streaming path, so this helper is the simplest
/// way to get a blocking, all-at-once string from a forced-tool request.
pub fn chat_blocking_drain(client: &dyn Client, request: ChatRequest) -> Result<String> {
    let rx = client.chat_stream(request)?;
    let mut out = String::new();
    while let Ok(chunk) = rx.recv() {
        match chunk {
            StreamChunk::Token(t) => out.push_str(&t),
            StreamChunk::ToolArgs(a) => out.push_str(&a),
            StreamChunk::Done => break,
            StreamChunk::Error(e) => {
                return Err(anyhow::anyhow!("chat stream error: {}", e));
            }
        }
    }
    Ok(out)
}

#[derive(Clone, Debug)]
pub struct LlmConfig {
    pub dry_run: bool,
    pub api_key: Option<String>,
}

impl LlmConfig {
    pub fn from_env(dry_run: bool) -> Self {
        Self::resolve(dry_run, None)
    }

    /// Resolve the LLM config from the environment, falling back to
    /// `config_api_key` if `OPENROUTER_API_KEY` isn't set. The env var always
    /// wins so a one-off `set -x OPENROUTER_API_KEY ...` can override the
    /// config file without editing it.
    pub fn resolve(dry_run: bool, config_api_key: Option<&str>) -> Self {
        let env_key = std::env::var("OPENROUTER_API_KEY")
            .ok()
            .filter(|s| !s.is_empty());
        let api_key = env_key.or_else(|| {
            config_api_key
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
        });
        Self { dry_run, api_key }
    }
}

pub fn make_client(cfg: &LlmConfig) -> Box<dyn Client> {
    if cfg.dry_run {
        tracing::info!("llm: dry-run mode, using FakeClient");
        return Box::new(FakeClient::new());
    }
    match cfg.api_key.as_ref() {
        Some(key) => match OpenRouterClientImpl::new(key.clone()) {
            Ok(c) => {
                tracing::info!("llm: OpenRouter client ready");
                Box::new(c)
            }
            Err(e) => {
                tracing::warn!(error = %e, "llm: failed to start OpenRouter client, falling back to FakeClient");
                Box::new(FakeClient::new())
            }
        },
        None => {
            tracing::warn!("llm: OPENROUTER_API_KEY not set, falling back to FakeClient");
            Box::new(FakeClient::new())
        }
    }
}

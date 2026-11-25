//! AI provider client, Ask-mode subset.
//!
//! Plain-HTTP AI client (reqwest, no LLM SDK) for the Ask-mode assistant:
//! non-streaming `complete`, streaming `stream_chat`, model listing,
//! connection testing and provider presets. Tool calling, CLI providers,
//! reasoning-effort levels and image inputs are intentionally out of scope.

use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

/// Default number of automatic retries on transient API errors
/// (rate limits, timeouts, network blips). Users can adjust the limit in
/// Settings → AI. Set to 0 to disable automatic retries entirely.
pub const DEFAULT_MAX_RETRIES: u32 = 2;
pub const MAX_MAX_RETRIES: u32 = 10;

/// Clamp a user-provided max-retries value into the supported range.
pub fn clamp_max_retries(value: u32) -> u32 {
    value.clamp(0, MAX_MAX_RETRIES)
}

// ---------------------------------------------------------------------------
// Request cancellation
// ---------------------------------------------------------------------------

pub const AGENT_CANCELLED_ERROR: &str = "Agent loop cancelled";

/// Whether `err` is the cancellation sentinel returned by streaming functions.
pub fn is_cancelled_error(err: &str) -> bool {
    err == AGENT_CANCELLED_ERROR
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiProvider {
    #[serde(alias = "anthropic")]
    Claude,
    #[serde(rename = "anthropic-compatible")]
    AnthropicCompatible,
    Openai,
    Gemini,
    Deepseek,
    Qwen,
    MiniMax,
    Ollama,
    #[serde(rename = "openai-compatible")]
    OpenaiCompatible,
    Custom,
}

impl AiProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            AiProvider::Claude => "claude",
            AiProvider::AnthropicCompatible => "anthropic-compatible",
            AiProvider::Openai => "openai",
            AiProvider::Gemini => "gemini",
            AiProvider::Deepseek => "deepseek",
            AiProvider::Qwen => "qwen",
            AiProvider::MiniMax => "minimax",
            AiProvider::Ollama => "ollama",
            AiProvider::OpenaiCompatible => "openai-compatible",
            AiProvider::Custom => "custom",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiApiStyle {
    #[default]
    Completions,
    Responses,
    #[serde(rename = "anthropic-messages")]
    AnthropicMessages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AiAuthMethod {
    #[default]
    ApiKey,
    Bearer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfig {
    pub provider: AiProvider,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub auth_method: AiAuthMethod,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub api_style: AiApiStyle,
    #[serde(default)]
    pub proxy_enabled: bool,
    #[serde(default)]
    pub proxy_url: String,
    #[serde(default = "default_enable_thinking")]
    pub enable_thinking: bool,
    #[serde(default)]
    pub context_window: Option<u32>,
    /// Maximum number of automatic retries on transient API errors
    /// (rate limits, timeouts, network blips). Defaults to 2; set to 0 to
    /// disable automatic retries entirely.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

fn default_enable_thinking() -> bool {
    true
}

fn default_max_retries() -> u32 {
    DEFAULT_MAX_RETRIES
}

/// A plain-text chat message. `role` is one of `system` / `user` / `assistant`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiMessage {
    pub role: String,
    pub content: String,
}

// ---------------------------------------------------------------------------
// Provider presets
// ---------------------------------------------------------------------------

pub struct AiProviderPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub default_endpoint: &'static str,
    pub default_model: &'static str,
    pub default_api_style: AiApiStyle,
    pub default_auth_method: AiAuthMethod,
    pub requires_api_key: bool,
}

pub fn provider_presets() -> &'static [AiProviderPreset] {
    const PRESETS: &[AiProviderPreset] = &[
        AiProviderPreset {
            id: "claude",
            label: "Claude",
            default_endpoint: "https://api.anthropic.com/v1/messages",
            default_model: "claude-sonnet-4-20250514",
            default_api_style: AiApiStyle::Completions,
            default_auth_method: AiAuthMethod::ApiKey,
            requires_api_key: true,
        },
        AiProviderPreset {
            id: "openai",
            label: "OpenAI",
            default_endpoint: "https://api.openai.com/v1/chat/completions",
            default_model: "gpt-4o-mini",
            default_api_style: AiApiStyle::Completions,
            default_auth_method: AiAuthMethod::Bearer,
            requires_api_key: true,
        },
        AiProviderPreset {
            id: "gemini",
            label: "Gemini",
            default_endpoint: "https://generativelanguage.googleapis.com",
            default_model: "gemini-1.5-pro",
            default_api_style: AiApiStyle::Completions,
            default_auth_method: AiAuthMethod::ApiKey,
            requires_api_key: true,
        },
        AiProviderPreset {
            id: "deepseek",
            label: "DeepSeek",
            default_endpoint: "https://api.deepseek.com/v1",
            default_model: "deepseek-v4-flash",
            default_api_style: AiApiStyle::Completions,
            default_auth_method: AiAuthMethod::Bearer,
            requires_api_key: true,
        },
        AiProviderPreset {
            id: "qwen",
            label: "Qwen",
            default_endpoint: "https://dashscope.aliyuncs.com/compatible-mode/v1",
            default_model: "qwen-plus",
            default_api_style: AiApiStyle::Completions,
            default_auth_method: AiAuthMethod::Bearer,
            requires_api_key: true,
        },
        AiProviderPreset {
            id: "minimax",
            label: "MiniMax",
            default_endpoint: "https://api.minimax.io/v1",
            default_model: "MiniMax-M3",
            default_api_style: AiApiStyle::Completions,
            default_auth_method: AiAuthMethod::Bearer,
            requires_api_key: true,
        },
        AiProviderPreset {
            id: "ollama",
            label: "Ollama",
            default_endpoint: "http://localhost:11434/v1",
            default_model: "llama3.1",
            default_api_style: AiApiStyle::Completions,
            default_auth_method: AiAuthMethod::Bearer,
            requires_api_key: false,
        },
        AiProviderPreset {
            id: "anthropic-compatible",
            label: "Anthropic Compatible",
            default_endpoint: "",
            default_model: "",
            default_api_style: AiApiStyle::AnthropicMessages,
            default_auth_method: AiAuthMethod::Bearer,
            requires_api_key: false,
        },
        AiProviderPreset {
            id: "openai-compatible",
            label: "OpenAI Compatible",
            default_endpoint: "",
            default_model: "",
            default_api_style: AiApiStyle::Completions,
            default_auth_method: AiAuthMethod::Bearer,
            requires_api_key: false,
        },
        AiProviderPreset {
            id: "custom",
            label: "Custom",
            default_endpoint: "",
            default_model: "",
            default_api_style: AiApiStyle::Completions,
            default_auth_method: AiAuthMethod::Bearer,
            requires_api_key: false,
        },
    ];
    PRESETS
}

/// The preset entry for `provider`, if any.
pub fn preset_for(provider: AiProvider) -> Option<&'static AiProviderPreset> {
    provider_presets()
        .iter()
        .find(|preset| preset.id == provider.as_str())
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// Ensure the endpoint has an OpenAI API version prefix (`/v1`) when the user
/// entered a bare origin without any path.  This handles the common mistake of
/// entering a host like `https://api.example.com` without the `/v1` path that
/// most OpenAI-compatible providers require.
///
/// Strategy (mirrors CC-Switch):
/// 1. Already ends with `/v1` → return as-is.
/// 2. Pure origin (no path after host) → append `/v1`.
/// 3. Has a custom path → leave it alone (user probably knows what they're doing).
fn ensure_openai_version_prefix(endpoint: &str) -> String {
    let ep = endpoint.trim().trim_end_matches('/');
    if ep.ends_with("/v1") {
        return ep.to_string();
    }
    // Check whether the URL is a bare origin with no path segment.
    let origin_only = match ep.split_once("://") {
        Some((_scheme, rest)) => !rest.contains('/'),
        None => !ep.contains('/'),
    };
    if origin_only {
        format!("{ep}/v1")
    } else {
        ep.to_string()
    }
}

fn ensure_anthropic_version_prefix(endpoint: &str) -> String {
    let ep = endpoint.trim().trim_end_matches('/');
    if ep.ends_with("/v1") {
        ep.to_string()
    } else {
        format!("{ep}/v1")
    }
}

pub fn resolve_endpoint(config: &AiConfig) -> String {
    let ep = config.endpoint.trim().trim_end_matches('/');
    if matches!(config.provider, AiProvider::Gemini) {
        if ep.ends_with(":generateContent") || ep.ends_with(":streamGenerateContent") {
            return ep.to_string();
        }
        let base = ep.trim_end_matches("/v1beta");
        return format!("{base}/v1beta/models/{}:generateContent", config.model);
    }
    if matches!(config.provider, AiProvider::Openai) {
        let base = ep
            .strip_suffix("/chat/completions")
            .or_else(|| ep.strip_suffix("/responses"))
            .unwrap_or(ep)
            .trim_end_matches('/');
        let base = ensure_openai_version_prefix(base);
        return if config.api_style == AiApiStyle::Responses {
            format!("{base}/responses")
        } else {
            format!("{base}/chat/completions")
        };
    }
    if ep.ends_with("/chat/completions") || ep.ends_with("/responses") || ep.ends_with("/messages")
    {
        return ep.to_string();
    }
    if uses_anthropic_messages_api(config) {
        let base = ensure_anthropic_version_prefix(ep);
        return format!("{base}/messages");
    }
    match config.provider {
        AiProvider::Openai
        | AiProvider::Deepseek
        | AiProvider::Qwen
        | AiProvider::MiniMax
        | AiProvider::Ollama
        | AiProvider::OpenaiCompatible
        | AiProvider::Custom => {
            let base = ensure_openai_version_prefix(ep);
            if config.api_style == AiApiStyle::Responses {
                format!("{base}/responses")
            } else {
                format!("{base}/chat/completions")
            }
        }
        AiProvider::Claude | AiProvider::AnthropicCompatible | AiProvider::Gemini => unreachable!(),
    }
}

pub fn uses_anthropic_messages_api(config: &AiConfig) -> bool {
    matches!(
        config.provider,
        AiProvider::Claude | AiProvider::AnthropicCompatible
    ) || matches!(config.provider, AiProvider::Custom)
        && config.api_style == AiApiStyle::AnthropicMessages
}

fn resolve_gemini_stream_endpoint(config: &AiConfig) -> String {
    let endpoint = resolve_endpoint(config);
    if endpoint.ends_with(":streamGenerateContent") {
        endpoint
    } else {
        endpoint.replace(":generateContent", ":streamGenerateContent")
    }
}

pub fn resolve_model_list_endpoint(config: &AiConfig) -> Result<String, String> {
    if matches!(config.provider, AiProvider::Gemini) {
        let ep = config.endpoint.trim().trim_end_matches('/');
        if ep.is_empty() {
            return Err("Endpoint is required".to_string());
        }
        let base = ep
            .split("/v1beta/models/")
            .next()
            .unwrap_or(ep)
            .trim_end_matches("/v1beta")
            .trim_end_matches('/');
        return Ok(format!("{base}/v1beta/models"));
    }

    let ep = config.endpoint.trim().trim_end_matches('/');
    if ep.is_empty() {
        return Err("Endpoint is required".to_string());
    }
    if ep.ends_with("/models") {
        return Ok(ep.to_string());
    }

    let base = ep
        .strip_suffix("/chat/completions")
        .or_else(|| ep.strip_suffix("/responses"))
        .or_else(|| ep.strip_suffix("/messages"))
        .unwrap_or(ep)
        .trim_end_matches('/');

    if uses_anthropic_messages_api(config) {
        let base = ensure_anthropic_version_prefix(base);
        return Ok(format!("{base}/models"));
    }

    let base = ensure_openai_version_prefix(base);

    Ok(format!("{base}/models"))
}

// ---------------------------------------------------------------------------
// SSE parsing helpers
// ---------------------------------------------------------------------------

/// Extract the JSON payload from a single SSE line, if any.
///
/// Skips keep-alive comments (`: ...`), `event:` / `id:` lines and blanks.
/// Accepts both `data: {...}` prefixed lines and bare JSON lines (some
/// gateways drop the `data:` prefix).
pub fn stream_data_payload(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.is_empty()
        || line.starts_with(':')
        || line.starts_with("event:")
        || line.starts_with("id:")
    {
        return None;
    }
    if let Some(data) = line.strip_prefix("data:") {
        return Some(data.trim_start());
    }
    if line.starts_with('{') {
        return Some(line);
    }
    None
}

fn stream_event_name(line: &str) -> Option<&str> {
    line.trim()
        .strip_prefix("event:")
        .map(str::trim)
        .filter(|event| !event.is_empty())
}

/// Detect an Anthropic stream error event (event type `error` or a payload
/// with `type: "error"`) and format a categorized error message.
fn anthropic_stream_error(event_name: Option<&str>, event: &serde_json::Value) -> Option<String> {
    if event_name != Some("error") && event["type"].as_str() != Some("error") {
        return None;
    }

    let error = event.get("error").unwrap_or(event);
    let error_type = error["type"]
        .as_str()
        .filter(|value| !value.trim().is_empty());
    let message = error["message"]
        .as_str()
        .or_else(|| event["message"].as_str())
        .filter(|value| !value.trim().is_empty());
    let detail = match (error_type, message) {
        (Some(error_type), Some(message)) => format!("{error_type}: {message}"),
        (Some(error_type), None) => error_type.to_string(),
        (None, Some(message)) => message.to_string(),
        (None, None) => truncate_diagnostic(&event.to_string(), 500),
    };
    let category = classify_error(&detail);
    Some(format!("[{category}] Anthropic stream error ({detail})"))
}

/// Drain the next newline-terminated line from the receive buffer.
/// Returns `Ok(None)` when no complete line is available yet.
fn drain_next_stream_line(buffer: &mut Vec<u8>) -> Result<Option<String>, String> {
    let Some(pos) = buffer.iter().position(|byte| *byte == b'\n') else {
        return Ok(None);
    };
    let mut line = buffer.drain(..=pos).collect::<Vec<u8>>();
    if line.last() == Some(&b'\n') {
        line.pop();
    }
    String::from_utf8(line)
        .map(Some)
        .map_err(|e| format!("AI stream returned invalid UTF-8: {e}"))
}

/// Extract the text delta from an Anthropic `content_block_delta` event.
pub fn claude_stream_text(event: &serde_json::Value) -> Option<&str> {
    if event["type"] == "content_block_delta" {
        return event["delta"]["text"].as_str();
    }
    None
}

fn text_from_content_value(value: &serde_json::Value) -> Option<String> {
    if let Some(text) = value.as_str().filter(|text| !text.is_empty()) {
        return Some(text.to_string());
    }

    value.as_array().and_then(|parts| {
        let text = parts
            .iter()
            .filter_map(|part| {
                part["text"]
                    .as_str()
                    .or_else(|| part["content"].as_str())
                    .or_else(|| part["input_text"].as_str())
                    .or_else(|| part["output_text"].as_str())
            })
            .collect::<Vec<_>>()
            .join("");
        (!text.is_empty()).then_some(text)
    })
}

/// Extract the full text from a non-streaming OpenAI-style chat completion
/// response. Tolerates several shapes (message content, legacy `text`,
/// delta content, top-level content, and Responses-style output).
pub fn openai_response_text(data: &serde_json::Value) -> String {
    data["choices"]
        .get(0)
        .and_then(|choice| {
            text_from_content_value(&choice["message"]["content"])
                .or_else(|| text_from_content_value(&choice["text"]))
                .or_else(|| text_from_content_value(&choice["delta"]["content"]))
        })
        .or_else(|| text_from_content_value(&data["content"]))
        .or_else(|| {
            let text = responses_text(data);
            (!text.is_empty()).then_some(text)
        })
        .unwrap_or_default()
}

/// Extract the text delta from an OpenAI-style streaming chat completion event.
pub fn openai_stream_text(event: &serde_json::Value) -> Option<String> {
    event["choices"]
        .get(0)
        .and_then(|choice| {
            text_from_content_value(&choice["delta"]["content"])
                .or_else(|| text_from_content_value(&choice["message"]["content"]))
                .or_else(|| text_from_content_value(&choice["text"]))
        })
        .or_else(|| text_from_content_value(&event["content"]))
        .or_else(|| {
            event["delta"]
                .as_str()
                .filter(|text| !text.is_empty())
                .map(ToString::to_string)
        })
}

/// Extract the reasoning/thinking delta from an OpenAI-style streaming event
/// (DeepSeek `reasoning_content`, MiniMax `thinking`, etc.).
pub fn openai_stream_reasoning(event: &serde_json::Value) -> Option<&str> {
    event["choices"]
        .get(0)
        .and_then(|choice| {
            choice["delta"]["reasoning_content"]
                .as_str()
                .or_else(|| choice["delta"]["reasoning"].as_str())
                .or_else(|| choice["delta"]["thinking"].as_str())
        })
        .filter(|text| !text.is_empty())
}

// ---------------------------------------------------------------------------
// MiniMax streaming semantics
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
enum MiniMaxStreamSemantics {
    #[default]
    Auto,
    Incremental,
}

fn minimax_stream_semantics(config: &AiConfig) -> MiniMaxStreamSemantics {
    // The China platform currently emits ordinary fragments, while the global
    // SDK example documents cumulative snapshots. A fixed regional mode avoids
    // the inherently ambiguous case where two incremental fragments happen to
    // share a prefix; custom gateways retain the tolerant auto mode.
    let china_endpoint = reqwest::Url::parse(&config.endpoint)
        .ok()
        .and_then(|url| url.host_str().map(ToString::to_string))
        .is_some_and(|host| host.eq_ignore_ascii_case("api.minimaxi.com"));
    if china_endpoint {
        MiniMaxStreamSemantics::Incremental
    } else {
        MiniMaxStreamSemantics::Auto
    }
}

fn minimax_stream_reasoning_details(event: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    event["choices"]
        .get(0)
        .and_then(|choice| choice["delta"]["reasoning_details"].as_array())
        .filter(|details| !details.is_empty())
}

/// Converts cumulative text snapshots or ordinary incremental chunks into
/// incremental deltas while retaining the complete assembled text.
///
/// MiniMax deployments do not all use the same streaming semantics: the global
/// API examples use cumulative snapshots, while the China API currently emits
/// incremental fragments. Accepting both forms keeps rendering consistent
/// across the official endpoints and compatible gateways.
#[derive(Debug, Default)]
struct MiniMaxTextAccumulator {
    semantics: MiniMaxStreamSemantics,
    latest: String,
}

impl MiniMaxTextAccumulator {
    fn new(semantics: MiniMaxStreamSemantics) -> Self {
        Self {
            semantics,
            latest: String::new(),
        }
    }

    fn push(&mut self, value: &str) -> Option<String> {
        if value.is_empty() {
            return None;
        }

        if matches!(self.semantics, MiniMaxStreamSemantics::Incremental) {
            return Some(value.to_string());
        }

        if value == self.latest {
            return None;
        }

        if let Some(suffix) = value.strip_prefix(&self.latest) {
            let delta = suffix.to_string();
            self.latest = value.to_string();
            return (!delta.is_empty()).then_some(delta);
        }

        if self.latest.starts_with(value) {
            // Ignore an older/shorter cumulative snapshot that arrived late.
            return None;
        }

        self.latest = value.to_string();
        Some(value.to_string())
    }
}

#[derive(Debug)]
struct MiniMaxReasoningDetailState {
    position: usize,
    latest: serde_json::Value,
    text: MiniMaxTextAccumulator,
}

impl MiniMaxReasoningDetailState {
    fn matches(&self, detail: &serde_json::Value, position: usize) -> bool {
        let id = detail["id"].as_str().filter(|id| !id.is_empty());
        if let (Some(id), Some(latest_id)) =
            (id, self.latest["id"].as_str().filter(|id| !id.is_empty()))
        {
            return id == latest_id;
        }

        let index = detail["index"].as_u64();
        if let (Some(index), Some(latest_index)) = (index, self.latest["index"].as_u64()) {
            return index == latest_index;
        }

        self.position == position
    }

    fn process(&mut self, detail: &serde_json::Value) -> Option<String> {
        let delta = detail["text"]
            .as_str()
            .and_then(|text| self.text.push(text));
        self.latest = detail.clone();
        delta
    }
}

#[derive(Debug)]
struct MiniMaxStreamState {
    semantics: MiniMaxStreamSemantics,
    content: MiniMaxTextAccumulator,
    reasoning_fallback: MiniMaxTextAccumulator,
    reasoning_details: Vec<MiniMaxReasoningDetailState>,
}

#[derive(Debug, Default, PartialEq)]
struct MiniMaxStreamDelta {
    text: Option<String>,
    reasoning: Option<String>,
}

impl MiniMaxStreamState {
    fn new(semantics: MiniMaxStreamSemantics) -> Self {
        Self {
            semantics,
            content: MiniMaxTextAccumulator::new(semantics),
            reasoning_fallback: MiniMaxTextAccumulator::new(semantics),
            reasoning_details: Vec::new(),
        }
    }

    fn process(&mut self, event: &serde_json::Value) -> MiniMaxStreamDelta {
        let text = openai_stream_text(event).and_then(|text| self.content.push(&text));
        let reasoning = if let Some(details) = minimax_stream_reasoning_details(event) {
            let fallback_delta = openai_stream_reasoning(event)
                .and_then(|reasoning| self.reasoning_fallback.push(reasoning));
            let has_detail_text = details
                .iter()
                .any(|detail| detail["text"].as_str().is_some_and(|text| !text.is_empty()));
            let mut delta = String::new();
            for (position, detail) in details.iter().enumerate() {
                let state_index = self
                    .reasoning_details
                    .iter()
                    .position(|state| state.matches(detail, position))
                    .unwrap_or_else(|| {
                        self.reasoning_details.push(MiniMaxReasoningDetailState {
                            position,
                            latest: detail.clone(),
                            text: MiniMaxTextAccumulator::new(self.semantics),
                        });
                        self.reasoning_details.len() - 1
                    });
                if let Some(fragment) = self.reasoning_details[state_index].process(detail) {
                    delta.push_str(&fragment);
                }
            }
            if has_detail_text {
                (!delta.is_empty()).then_some(delta)
            } else {
                fallback_delta
            }
        } else {
            openai_stream_reasoning(event)
                .and_then(|reasoning| self.reasoning_fallback.push(reasoning))
        };
        MiniMaxStreamDelta { text, reasoning }
    }
}

fn openai_stream_has_finish_reason(event: &serde_json::Value) -> bool {
    event["choices"].as_array().is_some_and(|choices| {
        choices.iter().any(|choice| {
            choice["finish_reason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty())
        })
    })
}

/// Extract the text delta from an OpenAI Responses API streaming event.
/// Events carry a `type` field; only `response.output_text.delta` (or an
/// untyped event with a `delta`) carries text.
pub fn responses_stream_text(event: &serde_json::Value) -> Option<&str> {
    let event_type = event["type"].as_str().unwrap_or_default();
    if !event_type.is_empty() && event_type != "response.output_text.delta" {
        return None;
    }

    event["delta"].as_str().filter(|s| !s.is_empty())
}

fn responses_max_output_tokens(max_tokens: Option<u32>) -> u32 {
    max_tokens.unwrap_or(2048).max(16)
}

fn responses_text(data: &serde_json::Value) -> String {
    if let Some(text) = data["output_text"].as_str().filter(|text| !text.is_empty()) {
        return text.to_string();
    }

    data["output"]
        .as_array()
        .and_then(|items| {
            items.iter().find_map(|item| {
                item["content"]
                    .as_array()
                    .and_then(|parts| parts.iter().find_map(|p| p["text"].as_str()))
            })
        })
        .unwrap_or_default()
        .to_string()
}

/// Extract the full text from a non-streaming Gemini response.
pub fn gemini_text(data: &serde_json::Value) -> String {
    data["candidates"]
        .get(0)
        .and_then(|candidate| candidate["content"]["parts"].as_array())
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Chat-completion body decoration (token limits, thinking toggles)
// ---------------------------------------------------------------------------

fn is_openai_api_endpoint(endpoint: &str) -> bool {
    reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| host.eq_ignore_ascii_case("api.openai.com"))
        })
        .unwrap_or(false)
}

fn is_openai_api_config(config: &AiConfig) -> bool {
    // OpenAI provider can be routed through a custom proxy while still requiring OpenAI request semantics.
    matches!(config.provider, AiProvider::Openai) || is_openai_api_endpoint(&config.endpoint)
}

fn is_openai_reasoning_model(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    model.starts_with("gpt-5")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
}

/// OpenAI reasoning models (and MiniMax) reject `max_tokens` in chat
/// completions and require `max_completion_tokens` instead.
fn uses_chat_completion_max_completion_tokens(config: &AiConfig) -> bool {
    matches!(config.provider, AiProvider::MiniMax)
        || is_openai_api_config(config) && is_openai_reasoning_model(&config.model)
}

fn set_chat_completion_token_limit(
    body: &mut serde_json::Value,
    config: &AiConfig,
    max_tokens: u32,
) {
    if uses_chat_completion_max_completion_tokens(config) {
        body["max_completion_tokens"] = json!(max_tokens);
    } else {
        body["max_tokens"] = json!(max_tokens);
    }
}

fn apply_minimax_chat_completion_fields(body: &mut serde_json::Value, config: &AiConfig) {
    if matches!(config.provider, AiProvider::MiniMax) {
        body["reasoning_split"] = json!(true);
    }
}

fn decorate_chat_completion_body(body: &mut serde_json::Value, config: &AiConfig, max_tokens: u32) {
    set_chat_completion_token_limit(body, config, max_tokens);
    apply_minimax_chat_completion_fields(body, config);
    apply_chat_completion_thinking_toggle(body, config);
}

/// Kimi K2.5+ models (including K2.7-Code) handle thinking flags differently
/// and reject the OpenAI-compatible `extra_body.chat_template_kwargs` toggle.
///
/// Matches `kimi-k2.5`, `kimi-k2.6`, `kimi-k2.7-code`, K3+, and future versions,
/// while excluding older K2 variants (`kimi-k2`, `kimi-k2-thinking`, etc.).
/// Regex equivalent: /kimi-k(?:2\.[5-9]\d*|[3-9]\d*)/
fn is_kimi_model(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    if let Some(rest) = model.strip_prefix("kimi-k") {
        if rest.starts_with("2.") && rest.len() > 2 {
            // K2.x — the digit after "2." must be >= 5 (so K2.5+)
            rest[2..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit() && c >= '5')
        } else {
            // K3+ — first char must be digit >= 3
            rest.chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit() && c >= '3')
        }
    } else {
        false
    }
}

/// When thinking is disabled, emit the provider-specific toggle in the
/// chat-completion body.
fn apply_chat_completion_thinking_toggle(body: &mut serde_json::Value, config: &AiConfig) {
    if config.enable_thinking {
        return;
    }

    if is_openai_api_config(config) {
        // `extra_body.chat_template_kwargs` is a third-party compatibility extension,
        // not an OpenAI API parameter. OpenAI models use their native defaults here.
        return;
    }

    if matches!(config.provider, AiProvider::MiniMax) {
        body["thinking"] = json!({ "type": "disabled" });
    } else if matches!(config.provider, AiProvider::Ollama) {
        // Ollama's OpenAI-compatible API uses reasoning_effort instead of
        // forwarding provider-specific chat template arguments.
        body["reasoning_effort"] = json!("none");
    } else if !is_kimi_model(&config.model) {
        body["extra_body"] = json!({
            "chat_template_kwargs": { "enable_thinking": false }
        });
    }
}

fn runtime_thinking_enabled(config: &AiConfig) -> bool {
    config.enable_thinking
}

// ---------------------------------------------------------------------------
// Message mapping
// ---------------------------------------------------------------------------

fn claude_http_model(model: &str) -> &str {
    let model = model.trim();
    if model.to_ascii_lowercase().ends_with("[1m]") {
        model[..model.len() - "[1m]".len()].trim_end()
    } else {
        model
    }
}

/// Fallback system prompt for the Anthropic (Claude) API.
///
/// Anthropic rejects requests whose `system` field is an empty string with
/// `system: text content blocks must be non-empty`. When the caller has no
/// system prompt we send this minimal placeholder so the request stays valid.
const CLAUDE_DEFAULT_SYSTEM: &str = "You are a helpful assistant.";

/// Returns a non-empty system prompt for Claude requests, substituting a
/// default when the provided prompt is empty or whitespace-only.
fn claude_system_prompt(system_prompt: &str) -> &str {
    if system_prompt.trim().is_empty() {
        CLAUDE_DEFAULT_SYSTEM
    } else {
        system_prompt
    }
}

fn claude_messages(messages: &[AiMessage]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .map(|message| json!({ "role": message.role, "content": message.content }))
        .collect()
}

/// Extract an error message from a provider error JSON body.
pub fn extract_api_error(data: &serde_json::Value) -> Option<String> {
    data["error"]["message"]
        .as_str()
        .or_else(|| data["error"].as_str())
        .map(ToString::to_string)
}

/// Build the OpenAI Responses API `input` array. The system prompt becomes a
/// leading `developer`-role item.
pub fn build_responses_input(system_prompt: &str, messages: &[AiMessage]) -> serde_json::Value {
    let mut input = Vec::new();
    if !system_prompt.is_empty() {
        input.push(json!({
            "role": "developer",
            "content": system_prompt,
        }));
    }
    for m in messages {
        input.push(json!({
            "role": m.role,
            "content": m.content,
        }));
    }
    json!(input)
}

fn build_openai_chat_messages(
    system_prompt: &str,
    messages: &[AiMessage],
) -> Vec<serde_json::Value> {
    let mut output = vec![json!({ "role": "system", "content": system_prompt })];
    output.extend(
        messages
            .iter()
            .map(|message| json!({ "role": message.role, "content": message.content })),
    );
    output
}

fn gemini_message_parts(message: &AiMessage) -> Vec<serde_json::Value> {
    if message.content.is_empty() {
        Vec::new()
    } else {
        vec![json!({ "text": message.content })]
    }
}

/// Map chat messages onto Gemini `contents`. Assistant turns use the `model`
/// role; everything else is `user`.
fn build_gemini_contents(messages: &[AiMessage]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .map(|message| {
            let role = if message.role == "assistant" {
                "model"
            } else {
                "user"
            };
            json!({ "role": role, "parts": gemini_message_parts(message) })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Payload builders
// ---------------------------------------------------------------------------

fn build_claude_payload(
    config: &AiConfig,
    system_prompt: &str,
    messages: &[AiMessage],
    max_tokens: u32,
    stream: bool,
) -> serde_json::Value {
    let mut body = json!({
        "model": claude_http_model(&config.model),
        "max_tokens": max_tokens,
        "system": claude_system_prompt(system_prompt),
        "messages": claude_messages(messages),
    });
    if stream {
        body["stream"] = json!(true);
    }
    body
}

fn build_responses_payload(
    config: &AiConfig,
    system_prompt: &str,
    messages: &[AiMessage],
    max_tokens: u32,
    stream: bool,
) -> serde_json::Value {
    let mut body = json!({
        "model": config.model,
        "input": build_responses_input(system_prompt, messages),
        "max_output_tokens": responses_max_output_tokens(Some(max_tokens)),
    });
    if stream {
        body["stream"] = json!(true);
    }
    body
}

fn build_openai_payload(
    config: &AiConfig,
    system_prompt: &str,
    messages: &[AiMessage],
    max_tokens: u32,
    stream: bool,
) -> serde_json::Value {
    let chat_messages = build_openai_chat_messages(system_prompt, messages);
    let mut body = json!({ "model": config.model, "messages": chat_messages });
    if stream {
        body["stream"] = json!(true);
    }
    decorate_chat_completion_body(&mut body, config, max_tokens);
    body
}

fn build_gemini_payload(
    system_prompt: &str,
    messages: &[AiMessage],
    max_tokens: u32,
) -> serde_json::Value {
    json!({
        "systemInstruction": {
            "parts": [{ "text": system_prompt }],
        },
        "contents": build_gemini_contents(messages),
        "generationConfig": {
            "maxOutputTokens": max_tokens,
        },
    })
}

// ---------------------------------------------------------------------------
// Headers, auth, client
// ---------------------------------------------------------------------------

fn provider_requires_api_key(provider: &AiProvider) -> bool {
    preset_for(*provider).is_some_and(|preset| preset.requires_api_key)
}

fn normalized_api_key(config: &AiConfig) -> &str {
    config.api_key.trim()
}

pub(super) fn validate_endpoint_and_model(config: &AiConfig) -> Result<(), String> {
    if matches!(config.provider, AiProvider::MiniMax) && config.api_style != AiApiStyle::Completions
    {
        return Err("MiniMax currently supports the Chat Completions API style in Orvyn; select Completions and retry"
            .to_string());
    }
    validate_endpoint(config)?;
    if config.model.trim().is_empty() {
        return Err("Model is required".to_string());
    }
    Ok(())
}

pub(super) fn validate_endpoint(config: &AiConfig) -> Result<(), String> {
    let url = reqwest::Url::parse(config.endpoint.trim())
        .map_err(|_| "Enter a valid HTTP or HTTPS API endpoint".to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "Use an HTTP or HTTPS endpoint without credentials, query parameters, or a fragment"
                .into(),
        );
    }
    Ok(())
}

fn validate_config(config: &AiConfig) -> Result<(), String> {
    validate_endpoint_and_model(config)?;
    if provider_requires_api_key(&config.provider) && config.api_key.trim().is_empty() {
        return Err("API key is required".to_string());
    }
    Ok(())
}

fn validate_model_list_config(config: &AiConfig) -> Result<(), String> {
    if provider_requires_api_key(&config.provider) && config.api_key.trim().is_empty() {
        return Err("API key is required".to_string());
    }
    resolve_model_list_endpoint(config).map(|_| ())
}

/// Content-Type plus a Bearer token when an API key is configured (Ollama and
/// other keyless endpoints send just the content type).
pub fn maybe_bearer_headers(config: &AiConfig) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let api_key = normalized_api_key(config);
    if !api_key.is_empty() {
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|e| e.to_string())?,
        );
    }
    Ok(headers)
}

/// Anthropic headers: `x-api-key` (default) or `Authorization: Bearer` per the
/// configured auth method, plus the fixed `anthropic-version` header.
pub fn claude_headers(config: &AiConfig) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let api_key = normalized_api_key(config);
    if !api_key.is_empty() {
        match config.auth_method {
            AiAuthMethod::Bearer => {
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {api_key}"))
                        .map_err(|e| e.to_string())?,
                );
            }
            AiAuthMethod::ApiKey => {
                headers.insert(
                    "x-api-key",
                    HeaderValue::from_str(api_key).map_err(|e| e.to_string())?,
                );
            }
        }
    }
    headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    Ok(headers)
}

fn normalize_ai_proxy_url(proxy_url: &str) -> String {
    let proxy_url = proxy_url.trim();
    if proxy_url.contains("://") || proxy_url.is_empty() {
        proxy_url.to_string()
    } else {
        format!("http://{proxy_url}")
    }
}

/// Whether the resolved endpoint targets a local (loopback) server. Local
/// endpoints must bypass the proxy, or local Ollama/LM Studio instances become
/// unreachable through an outbound proxy.
fn is_local_endpoint(config: &AiConfig) -> bool {
    let endpoint = resolve_endpoint(config);
    let Ok(url) = reqwest::Url::parse(&endpoint) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .map(|addr| addr.is_loopback())
            .unwrap_or(false)
}

pub fn build_ai_http_client(
    config: &AiConfig,
    timeout_secs: u64,
) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .user_agent("Orvyn");
    if config.proxy_enabled && !config.proxy_url.trim().is_empty() && !is_local_endpoint(config) {
        let proxy_url = normalize_ai_proxy_url(&config.proxy_url);
        let proxy =
            reqwest::Proxy::all(&proxy_url).map_err(|e| format!("Invalid AI proxy URL: {e}"))?;
        builder = builder.proxy(proxy);
    }
    builder.build().map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Model listing
// ---------------------------------------------------------------------------

fn parse_model_list_response(data: &serde_json::Value) -> Result<Vec<String>, String> {
    let items = data["data"]
        .as_array()
        .ok_or_else(|| "Invalid model list response".to_string())?;
    let mut seen = HashSet::new();
    let mut models = Vec::new();

    for item in items {
        let Some(id) = item["id"].as_str().filter(|id| !id.trim().is_empty()) else {
            continue;
        };
        if seen.insert(id.to_string()) {
            models.push(id.to_string());
        }
    }

    Ok(models)
}

/// Gemini model entries that cannot be used for text chat (image/video/audio
/// generation, embeddings, live/TTS, robotics, ...) are filtered out.
fn is_gemini_non_assistant_model(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    const PREFIXES: &[&str] = &[
        "antigravity",
        "chirp",
        "imagen",
        "lyria",
        "nano-banana",
        "veo",
    ];

    PREFIXES.iter().any(|prefix| model.starts_with(prefix))
        || model.contains("computer-use")
        || model.contains("deep-research")
        || model.contains("embedding")
        || model.contains("-image")
        || model.contains("gemini-omni")
        || model.contains("image-generation")
        || model.contains("-live")
        || model.contains("live-")
        || model.contains("native-audio")
        || model.contains("robotics")
        || model.contains("-tts")
}

fn gemini_item_is_assistant_compatible(item: &serde_json::Value) -> bool {
    let Some(model_id) = item["name"].as_str().or_else(|| item["id"].as_str()) else {
        return false;
    };
    if is_gemini_non_assistant_model(model_id) {
        return false;
    }

    let Some(methods) = item["supportedGenerationMethods"].as_array() else {
        return true;
    };
    if methods.is_empty() {
        return true;
    }

    methods
        .iter()
        .filter_map(serde_json::Value::as_str)
        .any(|method| {
            method.eq_ignore_ascii_case("generateContent")
                || method.eq_ignore_ascii_case("streamGenerateContent")
        })
}

fn parse_gemini_model_list_response(data: &serde_json::Value) -> Result<Vec<String>, String> {
    let items = data["models"]
        .as_array()
        .ok_or_else(|| "Invalid Gemini model list response".to_string())?;
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for item in items {
        if !gemini_item_is_assistant_compatible(item) {
            continue;
        }
        let Some(name) = item["name"].as_str() else {
            continue;
        };
        let id = name.trim().trim_start_matches("models/");
        if id.is_empty() || !seen.insert(id.to_string()) {
            continue;
        }
        models.push(id.to_string());
    }
    Ok(models)
}

/// Anthropic-style model listing with cursor pagination (`after_id`).
async fn list_claude_models(
    client: &reqwest::Client,
    config: &AiConfig,
) -> Result<Vec<String>, String> {
    let endpoint = resolve_model_list_endpoint(config)?;
    let headers = claude_headers(config)?;
    let mut after_id: Option<String> = None;
    let mut seen_cursors = HashSet::new();
    let mut seen_models = HashSet::new();
    let mut models = Vec::new();

    loop {
        let mut request = client.get(&endpoint).headers(headers.clone());
        if let Some(after_id) = after_id.as_deref() {
            request = request.query(&[("after_id", after_id)]);
        }
        let res = request
            .send()
            .await
            .map_err(|e| format!("Claude model list request failed: {e}"))?;

        let status = res.status();
        if !status.is_success() {
            if matches!(
                config.provider,
                AiProvider::AnthropicCompatible | AiProvider::Custom
            ) && matches!(
                status,
                reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::METHOD_NOT_ALLOWED
            ) {
                return Err(format!(
                    "[modelDiscoveryUnsupported] The provider does not expose a model list at {endpoint}. Save the provider and enter a model ID manually."
                ));
            }
            return Err(categorized_http_error(res, "Claude model list", &config.api_key).await);
        }

        let data: serde_json::Value = res
            .json()
            .await
            .map_err(|e| format!("Claude model list returned invalid JSON (HTTP {status}): {e}"))?;
        for model in parse_model_list_response(&data)? {
            if seen_models.insert(model.clone()) {
                models.push(model);
            }
        }

        if data["has_more"].as_bool() != Some(true) {
            break;
        }
        let next_cursor = data["last_id"]
            .as_str()
            .filter(|cursor| !cursor.trim().is_empty())
            .ok_or_else(|| {
                "Claude model list response has_more=true but last_id is missing".to_string()
            })?;
        if !seen_cursors.insert(next_cursor.to_string()) {
            return Err(format!(
                "Claude model list returned repeated last_id cursor: {next_cursor}"
            ));
        }
        after_id = Some(next_cursor.to_string());
    }

    Ok(models)
}

/// Gemini model listing with page-token pagination.
async fn list_gemini_models(
    client: &reqwest::Client,
    config: &AiConfig,
) -> Result<Vec<String>, String> {
    let endpoint = resolve_model_list_endpoint(config)?;
    let mut page_token: Option<String> = None;
    let mut seen = HashSet::new();
    let mut models = Vec::new();

    loop {
        let mut request = client
            .get(&endpoint)
            .query(&[("pageSize", "1000")])
            .header("x-goog-api-key", normalized_api_key(config));
        if let Some(token) = page_token.as_deref() {
            request = request.query(&[("pageToken", token)]);
        }

        let res = request
            .send()
            .await
            .map_err(|e| format!("Gemini model list request failed: {e}"))?;
        let status = res.status();
        let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(extract_api_error(&data)
                .unwrap_or_else(|| format!("Gemini model list API error: {status}")));
        }

        for model in parse_gemini_model_list_response(&data)? {
            if seen.insert(model.clone()) {
                models.push(model);
            }
        }

        page_token = data["nextPageToken"]
            .as_str()
            .filter(|token| !token.trim().is_empty())
            .map(ToString::to_string);
        if page_token.is_none() {
            break;
        }
    }

    Ok(models)
}

/// OpenAI-style model listing (single `/models` page). Also used for Ollama's
/// OpenAI-compatible endpoint.
async fn list_openai_compatible_models(
    client: &reqwest::Client,
    config: &AiConfig,
) -> Result<Vec<String>, String> {
    let res = client
        .get(resolve_model_list_endpoint(config)?)
        .headers(maybe_bearer_headers(config)?)
        .send()
        .await
        .map_err(|e| format!("AI model list request failed: {e}"))?;

    let status = res.status();
    let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(
            extract_api_error(&data).unwrap_or_else(|| format!("Model list API error: {status}"))
        );
    }

    parse_model_list_response(&data)
}

async fn list_models_core(config: &AiConfig) -> Result<Vec<String>, String> {
    validate_model_list_config(config)?;
    let client = build_ai_http_client(config, 30)?;

    match config.provider {
        AiProvider::Claude | AiProvider::AnthropicCompatible => {
            list_claude_models(&client, config).await
        }
        AiProvider::Gemini => list_gemini_models(&client, config).await,
        AiProvider::Ollama
        | AiProvider::Openai
        | AiProvider::Deepseek
        | AiProvider::Qwen
        | AiProvider::MiniMax
        | AiProvider::OpenaiCompatible => list_openai_compatible_models(&client, config).await,
        AiProvider::Custom => {
            if uses_anthropic_messages_api(config) {
                list_claude_models(&client, config).await
            } else {
                list_openai_compatible_models(&client, config).await
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Error classification, diagnostics and retry
// ---------------------------------------------------------------------------

fn truncate_diagnostic(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

/// Read a non-2xx response into a categorized, UI-ready error string.
/// The API key is masked out of the body and any `Retry-After` header is
/// embedded so the retry helpers can honour server-requested delays.
async fn categorized_http_error(
    response: reqwest::Response,
    provider: &str,
    api_key: &str,
) -> String {
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|data| extract_api_error(&data).or_else(|| Some(data.to_string())))
        .unwrap_or_else(|| {
            if body.trim().is_empty() {
                "empty response body".to_string()
            } else {
                body.trim().to_string()
            }
        });
    let api_key = api_key.trim();
    let detail = if api_key.is_empty() {
        detail
    } else {
        detail.replace(api_key, "***")
    };
    let detail = truncate_diagnostic(&detail, 500);
    let diagnostic = format!("HTTP {}: {detail}", status.as_u16());
    maybe_tag_retry_after(
        &headers,
        format!(
            "[{}] {provider} API error ({diagnostic})",
            classify_error(&diagnostic)
        ),
    )
}

fn format_transport_error(provider: &str, error: reqwest::Error) -> String {
    // Request URLs may contain credentials in query parameters, notably Gemini API keys.
    format!("{provider} request failed: {}", error.without_url())
}

/// Extract an error string from a non-2xx streaming response, preserving any
/// `Retry-After` header so the retry helpers can honour server-requested delays.
///
/// Mirrors [`categorized_http_error`]: always embeds `HTTP <status>` in the
/// diagnostic so that [`classify_error`] works correctly on empty-body or
/// non-JSON responses (e.g. a bare 429 from a proxy).
async fn stream_error(response: reqwest::Response, fallback: &str) -> String {
    let status = response.status();
    let headers = response.headers().clone();
    // Read the body as text first so we still have it when JSON parsing fails.
    let body_text = response.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<serde_json::Value>(&body_text)
        .ok()
        .and_then(|data| extract_api_error(&data))
        .unwrap_or_else(|| {
            let trimmed = body_text.trim();
            if trimmed.is_empty() {
                "empty response body".to_string()
            } else {
                trimmed.to_string()
            }
        });
    let diagnostic = format!("HTTP {}: {detail}", status.as_u16());
    maybe_tag_retry_after(
        &headers,
        format!(
            "[{}] {fallback} API error ({diagnostic})",
            classify_error(&diagnostic)
        ),
    )
}

/// Map an error message to a coarse category the UI can use for guidance.
fn classify_error(msg: &str) -> &'static str {
    let lower = msg.to_ascii_lowercase();
    if lower.contains("401")
        || lower.contains("403")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("permission denied")
        || lower.contains("permission_denied")
        || lower.contains("invalid api key")
        || lower.contains("incorrect api key")
        || lower.contains("api key not valid")
        || lower.contains("api_key_invalid")
        || lower.contains("authentication_error")
        || lower.contains("permission_error")
    {
        "auth"
    } else if lower.contains("404")
        || lower.contains("not found")
        || lower.contains("model not found")
        || lower.contains("model does not exist")
        || lower.contains("not supported for generatecontent")
        || lower.contains("not_found_error")
    {
        "modelNotFound"
    } else if lower.contains("429")
        || lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("resource_exhausted")
        || lower.contains("quota exceeded")
        || lower.contains("rate_limit_error")
    {
        "rateLimit"
    } else if lower.contains("timeout") || lower.contains("timed out") || lower.contains("504") {
        "timeout"
    } else if lower.contains("finishreason=max_tokens")
        || lower.contains("thinking tokens")
        || lower.contains("request_too_large")
    {
        "tokenLimit"
    } else if lower.contains("safety filter") || lower.contains("prompt was blocked") {
        "safety"
    } else if lower.contains("no text")
        || lower.contains("response body was empty")
        || lower.contains("no sse data events")
        || lower.contains("without text")
    {
        "emptyResponse"
    } else if lower.contains("connect")
        || lower.contains("dns")
        || lower.contains("resolve")
        || lower.contains("502")
        || lower.contains("503")
        || lower.contains("api_error")
        || lower.contains("overloaded_error")
    {
        "network"
    } else {
        "unknown"
    }
}

/// Whether an error is transient and worth retrying.
/// Rate limits, timeouts, network blips, and empty responses are retryable;
/// authentication failures, missing models, safety blocks, and token limits are not.
fn is_retryable_error(error: &str) -> bool {
    matches!(
        classify_error(error),
        "rateLimit" | "timeout" | "network" | "emptyResponse"
    )
}

/// Extract Retry-After seconds from an HTTP header value.
///
/// Only integer seconds are parsed; `None` for missing or unparseable values
/// means the caller falls back to its default back-off.
pub fn parse_retry_after_secs(value: &str) -> Option<u64> {
    value.trim().parse::<u64>().ok()
}

fn retry_after_secs(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    parse_retry_after_secs(value)
}

/// Parse a Retry-After duration from an error string.
///
/// Recognises the `[retry-after:N]` prefix (N in whole seconds) optionally
/// embedded by [`categorized_http_error`] or stream error sites.
fn parse_retry_after(error: &str) -> Option<std::time::Duration> {
    let rest = error.strip_prefix("[retry-after:")?;
    let end = rest.find(']')?;
    let secs: u64 = rest[..end].parse().ok()?;
    Some(std::time::Duration::from_secs(secs))
}

/// Prepend a `[retry-after:N]` tag to an error string if the headers include
/// a Retry-After value.
fn maybe_tag_retry_after(headers: &reqwest::header::HeaderMap, mut error: String) -> String {
    if let Some(secs) = retry_after_secs(headers) {
        error = format!("[retry-after:{secs}]{error}");
    }
    error
}

/// Execute an async operation with automatic retry on transient errors.
///
/// Uses the `max_retries` field from `config` (capped at [`MAX_MAX_RETRIES`]).
/// Exponential back-off starts at 500 ms and doubles each attempt, with a
/// small jitter to spread retries across concurrent tasks.
async fn with_retry<F, Fut, T>(config: &AiConfig, mut op: F) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let max = config.max_retries.min(MAX_MAX_RETRIES);
    let mut last_err: Option<String> = None;

    for attempt in 0..=max {
        if attempt > 0 {
            // Exponential back-off: 500ms, 1s, 2s, 4s … with ±25 % jitter.
            let base_ms = 2u64.pow(attempt.saturating_sub(1)) * 500;
            // Honour Retry-After from the previous error response.
            let server_delay_ms = last_err
                .as_ref()
                .and_then(|e| parse_retry_after(e))
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let base_ms = base_ms.max(server_delay_ms);
            let jitter = (base_ms as f64 * 0.25) as u64;
            // We can't use thread_rng here (wasm compat), so rely on the OS
            // scheduler to naturally jitter concurrent tasks.  A tiny fixed
            // offset per attempt avoids thundering-herd for a single caller.
            let delay_ms = base_ms + (attempt as u64 * 37) % jitter.max(1);
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            tracing::debug!(
                "[ai][retry] attempt {attempt}/{} ({}ms back-off)",
                max,
                delay_ms
            );
        }

        match op().await {
            Ok(result) => return Ok(result),
            Err(err) if attempt < max && is_retryable_error(&err) => {
                tracing::warn!("[ai][retry] transient error, will retry");
                last_err = Some(err);
            }
            Err(err) => return Err(err),
        }
    }

    Err(last_err.unwrap_or_else(|| "retry exhausted with no recorded error".to_string()))
}

/// Variant of [`with_retry`] for streaming operations.
///
/// Streaming requests are retried only when the `emitted` flag is still
/// `false` — that is, no text or reasoning delta has reached the UI. Once
/// content has been emitted, retrying would cause duplicate output, so the
/// error is propagated immediately regardless of whether it is transient.
///
/// The closure rebuilds the full `send` + stream-consume each attempt.
async fn with_stream_retry<F, Fut>(
    config: &AiConfig,
    emitted: &Arc<AtomicBool>,
    mut op: F,
) -> Result<(), String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let max = config.max_retries.min(MAX_MAX_RETRIES);
    let mut last_err: Option<String> = None;

    for attempt in 0..=max {
        if attempt > 0 {
            let base_ms = 2u64.pow(attempt.saturating_sub(1)) * 500;
            // Honour Retry-After from the previous error response.
            let server_delay_ms = last_err
                .as_ref()
                .and_then(|e| parse_retry_after(e))
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let base_ms = base_ms.max(server_delay_ms);
            let jitter = (base_ms as f64 * 0.25) as u64;
            let delay_ms = base_ms + (attempt as u64 * 37) % jitter.max(1);
            let delay = std::time::Duration::from_millis(delay_ms);
            tracing::debug!(
                "[ai][stream_retry] attempt {attempt}/{} ({}ms back-off)",
                max,
                delay_ms
            );
            tokio::time::sleep(delay).await;
        }

        match op().await {
            Ok(()) => return Ok(()),
            Err(err)
                if attempt < max
                    && is_retryable_error(&err)
                    && !emitted.load(Ordering::Relaxed) =>
            {
                tracing::warn!("[ai][stream_retry] transient error before content, will retry");
                last_err = Some(err);
            }
            Err(err) => return Err(err),
        }
    }

    Err(last_err.unwrap_or_else(|| "stream retry exhausted with no recorded error".to_string()))
}

// ---------------------------------------------------------------------------
// Non-streaming calls
// ---------------------------------------------------------------------------

async fn call_claude(
    client: &reqwest::Client,
    config: &AiConfig,
    system_prompt: &str,
    messages: &[AiMessage],
    max_tokens: u32,
) -> Result<String, String> {
    let body = build_claude_payload(config, system_prompt, messages, max_tokens, false);

    let res = client
        .post(resolve_endpoint(config))
        .headers(claude_headers(config)?)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Claude request failed: {e}"))?;

    let status = res.status();
    let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(
            extract_api_error(&data).unwrap_or_else(|| format!("Claude API error: {status}"))
        );
    }

    Ok(data["content"]
        .as_array()
        .and_then(|items| items.iter().find_map(|item| item["text"].as_str()))
        .unwrap_or_default()
        .to_string())
}

async fn call_openai_compatible(
    client: &reqwest::Client,
    config: &AiConfig,
    system_prompt: &str,
    messages: &[AiMessage],
    max_tokens: u32,
) -> Result<String, String> {
    let headers = maybe_bearer_headers(config)?;
    let body = build_openai_payload(config, system_prompt, messages, max_tokens, false);

    let res = client
        .post(resolve_endpoint(config))
        .headers(headers)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("AI request failed: {e}"))?;

    let status = res.status();
    let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(extract_api_error(&data).unwrap_or_else(|| format!("API error: {status}")));
    }

    Ok(openai_response_text(&data))
}

async fn call_responses_api(
    client: &reqwest::Client,
    config: &AiConfig,
    system_prompt: &str,
    messages: &[AiMessage],
    max_tokens: u32,
) -> Result<String, String> {
    let headers = maybe_bearer_headers(config)?;
    let body = build_responses_payload(config, system_prompt, messages, max_tokens, false);

    let res = client
        .post(resolve_endpoint(config))
        .headers(headers)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("AI request failed: {e}"))?;

    let status = res.status();
    let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(extract_api_error(&data).unwrap_or_else(|| format!("API error: {status}")));
    }

    Ok(responses_text(&data))
}

async fn call_gemini(
    client: &reqwest::Client,
    config: &AiConfig,
    system_prompt: &str,
    messages: &[AiMessage],
    max_tokens: u32,
) -> Result<String, String> {
    let body = build_gemini_payload(system_prompt, messages, max_tokens);

    let res = client
        .post(resolve_endpoint(config))
        .header("x-goog-api-key", normalized_api_key(config))
        .header(CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format_transport_error("Gemini", e))?;

    let status = res.status();
    let data: serde_json::Value = res
        .json()
        .await
        .map_err(|e| format_transport_error("Gemini", e))?;
    if !status.is_success() {
        return Err(
            extract_api_error(&data).unwrap_or_else(|| format!("Gemini API error: {status}"))
        );
    }

    Ok(gemini_text(&data))
}

async fn complete_inner(
    client: &reqwest::Client,
    config: &AiConfig,
    system_prompt: &str,
    messages: &[AiMessage],
    max_tokens: u32,
) -> Result<String, String> {
    if uses_anthropic_messages_api(config) {
        return call_claude(client, config, system_prompt, messages, max_tokens).await;
    }

    match config.provider {
        AiProvider::Gemini => {
            call_gemini(client, config, system_prompt, messages, max_tokens).await
        }
        AiProvider::Openai
        | AiProvider::Deepseek
        | AiProvider::Qwen
        | AiProvider::MiniMax
        | AiProvider::Ollama
        | AiProvider::OpenaiCompatible
        | AiProvider::Custom => {
            if config.api_style == AiApiStyle::Responses {
                call_responses_api(client, config, system_prompt, messages, max_tokens).await
            } else {
                call_openai_compatible(client, config, system_prompt, messages, max_tokens).await
            }
        }
        AiProvider::Claude | AiProvider::AnthropicCompatible => unreachable!(),
    }
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

/// Run a streaming completion. Text and reasoning deltas are forwarded to
/// `on_delta` as `(text_delta, reasoning_delta)` (either may be empty).
pub async fn stream_chat(
    config: &AiConfig,
    system_prompt: &str,
    messages: &[AiMessage],
    max_tokens: u32,
    cancelled: &Notify,
    on_delta: &(dyn Fn(&str, &str) + Send + Sync),
) -> Result<(), String> {
    validate_config(config)?;

    tokio::select! {
        biased;
        _ = cancelled.notified() => Err(AGENT_CANCELLED_ERROR.to_string()),
        result = stream_inner(config, system_prompt, messages, max_tokens, on_delta) => result,
    }
}

async fn stream_inner(
    config: &AiConfig,
    system_prompt: &str,
    messages: &[AiMessage],
    max_tokens: u32,
    on_delta: &(dyn Fn(&str, &str) + Send + Sync),
) -> Result<(), String> {
    // Thinking models can take much longer to produce the first token.
    let stream_timeout = if runtime_thinking_enabled(config) {
        600
    } else {
        120
    };
    let client = build_ai_http_client(config, stream_timeout)?;

    if uses_anthropic_messages_api(config) {
        return stream_claude(
            &client,
            config,
            system_prompt,
            messages,
            max_tokens,
            on_delta,
        )
        .await;
    }

    match config.provider {
        AiProvider::Gemini => {
            stream_gemini(
                &client,
                config,
                system_prompt,
                messages,
                max_tokens,
                on_delta,
            )
            .await
        }
        AiProvider::Openai
        | AiProvider::Deepseek
        | AiProvider::Qwen
        | AiProvider::MiniMax
        | AiProvider::Ollama
        | AiProvider::OpenaiCompatible
        | AiProvider::Custom => {
            if config.api_style == AiApiStyle::Responses {
                stream_responses_api(
                    &client,
                    config,
                    system_prompt,
                    messages,
                    max_tokens,
                    on_delta,
                )
                .await
            } else {
                stream_openai(
                    &client,
                    config,
                    system_prompt,
                    messages,
                    max_tokens,
                    on_delta,
                )
                .await
            }
        }
        AiProvider::Claude | AiProvider::AnthropicCompatible => unreachable!(),
    }
}

async fn stream_claude(
    client: &reqwest::Client,
    config: &AiConfig,
    system_prompt: &str,
    messages: &[AiMessage],
    max_tokens: u32,
    on_delta: &(dyn Fn(&str, &str) + Send + Sync),
) -> Result<(), String> {
    let body = build_claude_payload(config, system_prompt, messages, max_tokens, true);
    let headers = claude_headers(config)?;
    let endpoint = resolve_endpoint(config);
    let emitted = Arc::new(AtomicBool::new(false));

    with_stream_retry(config, &emitted, || {
        let body = body.clone();
        let headers = headers.clone();
        let endpoint = endpoint.clone();
        let emitted = emitted.clone();
        async move {
            let res = client
                .post(&endpoint)
                .headers(headers)
                .json(&body)
                .send()
                .await
                .map_err(|e| format!("Claude request failed: {e}"))?;
            if !res.status().is_success() {
                return Err(stream_error(res, "Claude API error").await);
            }

            let mut byte_stream = res.bytes_stream();
            let mut buf = Vec::new();
            let mut event_name: Option<String> = None;

            loop {
                let chunk = byte_stream.next().await;
                let Some(chunk) = chunk else { break };
                let chunk = chunk.map_err(|e| e.to_string())?;
                buf.extend_from_slice(&chunk);

                let mut finished = false;
                while let Some(line) = drain_next_stream_line(&mut buf)? {
                    if line.trim().is_empty() {
                        event_name = None;
                        continue;
                    }
                    if let Some(name) = stream_event_name(&line) {
                        event_name = Some(name.to_string());
                        continue;
                    }
                    let Some(data) = stream_data_payload(&line) else {
                        continue;
                    };
                    if data == "[DONE]" {
                        finished = true;
                        break;
                    }

                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                        if let Some(error) = anthropic_stream_error(event_name.as_deref(), &event) {
                            return Err(error);
                        }
                        if let Some(text) = claude_stream_text(&event) {
                            if !text.is_empty() {
                                emitted.store(true, Ordering::Relaxed);
                                on_delta(text, "");
                            }
                        }
                    }
                }

                if finished {
                    break;
                }
            }

            Ok(())
        }
    })
    .await
}

async fn stream_openai(
    client: &reqwest::Client,
    config: &AiConfig,
    system_prompt: &str,
    messages: &[AiMessage],
    max_tokens: u32,
    on_delta: &(dyn Fn(&str, &str) + Send + Sync),
) -> Result<(), String> {
    let headers = maybe_bearer_headers(config)?;
    let body = build_openai_payload(config, system_prompt, messages, max_tokens, true);
    let endpoint = resolve_endpoint(config);
    let is_minimax = matches!(config.provider, AiProvider::MiniMax);
    let minimax_semantics = minimax_stream_semantics(config);
    let emitted = Arc::new(AtomicBool::new(false));

    with_stream_retry(config, &emitted, || {
        stream_openai_attempt(
            client.post(&endpoint).headers(headers.clone()).json(&body),
            is_minimax,
            minimax_semantics,
            on_delta,
            &emitted,
        )
    })
    .await
}

async fn stream_openai_attempt(
    request: reqwest::RequestBuilder,
    is_minimax: bool,
    minimax_semantics: MiniMaxStreamSemantics,
    on_delta: &(dyn Fn(&str, &str) + Send + Sync),
    emitted: &AtomicBool,
) -> Result<(), String> {
    let mut minimax_state = MiniMaxStreamState::new(minimax_semantics);
    let res = request
        .send()
        .await
        .map_err(|e| format!("AI request failed: {e}"))?;
    if !res.status().is_success() {
        return Err(stream_error(res, "API error").await);
    }

    let mut byte_stream = res.bytes_stream();
    let mut buf = Vec::new();
    let mut finish_reason_deadline = None;

    loop {
        let chunk = if let Some(deadline) = finish_reason_deadline {
            match tokio::time::timeout_at(deadline, byte_stream.next()).await {
                Ok(chunk) => chunk,
                Err(_) => break,
            }
        } else {
            byte_stream.next().await
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|e| e.to_string())?;
        buf.extend_from_slice(&chunk);

        let mut finished = false;
        while let Some(line) = drain_next_stream_line(&mut buf)? {
            let Some(data) = stream_data_payload(&line) else {
                continue;
            };
            if data == "[DONE]" {
                finished = true;
                break;
            }

            if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                if let Some(error) = extract_api_error(&event) {
                    return Err(error);
                }
                if is_minimax {
                    let delta = minimax_state.process(&event);
                    if let Some(reasoning) = delta.reasoning {
                        emitted.store(true, Ordering::Relaxed);
                        on_delta("", &reasoning);
                    }
                    if let Some(text) = delta.text {
                        emitted.store(true, Ordering::Relaxed);
                        on_delta(&text, "");
                    }
                } else {
                    if let Some(reasoning) = openai_stream_reasoning(&event) {
                        emitted.store(true, Ordering::Relaxed);
                        on_delta("", reasoning);
                    }
                    if let Some(text) = openai_stream_text(&event) {
                        emitted.store(true, Ordering::Relaxed);
                        on_delta(&text, "");
                    }
                }
                if finish_reason_deadline.is_none() && openai_stream_has_finish_reason(&event) {
                    finish_reason_deadline =
                        Some(tokio::time::Instant::now() + std::time::Duration::from_secs(1));
                }
            }
        }

        if finished {
            break;
        }
    }

    Ok(())
}

async fn stream_responses_api(
    client: &reqwest::Client,
    config: &AiConfig,
    system_prompt: &str,
    messages: &[AiMessage],
    max_tokens: u32,
    on_delta: &(dyn Fn(&str, &str) + Send + Sync),
) -> Result<(), String> {
    let headers = maybe_bearer_headers(config)?;
    let body = build_responses_payload(config, system_prompt, messages, max_tokens, true);
    let endpoint = resolve_endpoint(config);
    let emitted = Arc::new(AtomicBool::new(false));

    with_stream_retry(config, &emitted, || {
        stream_responses_api_attempt(client, &body, &headers, &endpoint, on_delta, &emitted)
    })
    .await
}

async fn stream_responses_api_attempt(
    client: &reqwest::Client,
    body: &serde_json::Value,
    headers: &HeaderMap,
    endpoint: &str,
    on_delta: &(dyn Fn(&str, &str) + Send + Sync),
    emitted: &AtomicBool,
) -> Result<(), String> {
    let res = client
        .post(endpoint)
        .headers(headers.clone())
        .json(body)
        .send()
        .await
        .map_err(|e| format!("AI request failed: {e}"))?;
    if !res.status().is_success() {
        return Err(stream_error(res, "API error").await);
    }

    let mut byte_stream = res.bytes_stream();
    let mut buf = Vec::new();

    loop {
        let chunk = byte_stream.next().await;
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|e| e.to_string())?;
        buf.extend_from_slice(&chunk);

        let mut finished = false;
        while let Some(line) = drain_next_stream_line(&mut buf)? {
            let Some(data) = stream_data_payload(&line) else {
                continue;
            };
            if data == "[DONE]" {
                finished = true;
                break;
            }

            if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                match event["type"].as_str() {
                    Some("response.completed") => return Ok(()),
                    Some("response.failed" | "response.incomplete" | "error") => {
                        return Err(event["response"]["error"]["message"]
                            .as_str()
                            .or_else(|| event["message"].as_str())
                            .or_else(|| event["response"]["incomplete_details"]["reason"].as_str())
                            .unwrap_or("The provider ended the response before completion")
                            .to_owned());
                    }
                    _ => {}
                }
                if let Some(text) = responses_stream_text(&event) {
                    if !text.is_empty() {
                        emitted.store(true, Ordering::Relaxed);
                        on_delta(text, "");
                    }
                }
            }
        }

        if finished {
            break;
        }
    }

    Ok(())
}

async fn stream_gemini(
    client: &reqwest::Client,
    config: &AiConfig,
    system_prompt: &str,
    messages: &[AiMessage],
    max_tokens: u32,
    on_delta: &(dyn Fn(&str, &str) + Send + Sync),
) -> Result<(), String> {
    let body = build_gemini_payload(system_prompt, messages, max_tokens);
    let endpoint = resolve_gemini_stream_endpoint(config);
    let api_key = normalized_api_key(config).to_string();
    let emitted = Arc::new(AtomicBool::new(false));

    with_stream_retry(config, &emitted, || {
        stream_gemini_attempt(client, &body, &endpoint, &api_key, on_delta, &emitted)
    })
    .await
}

async fn stream_gemini_attempt(
    client: &reqwest::Client,
    body: &serde_json::Value,
    endpoint: &str,
    api_key: &str,
    on_delta: &(dyn Fn(&str, &str) + Send + Sync),
    emitted: &AtomicBool,
) -> Result<(), String> {
    let res = client
        .post(endpoint)
        .query(&[("alt", "sse")])
        .header("x-goog-api-key", api_key)
        .header(CONTENT_TYPE, "application/json")
        .json(body)
        .send()
        .await
        .map_err(|e| format_transport_error("Gemini", e))?;
    if !res.status().is_success() {
        return Err(stream_error(res, "Gemini API error").await);
    }

    let mut byte_stream = res.bytes_stream();
    let mut buf = Vec::new();

    loop {
        let chunk = byte_stream.next().await;
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|e| e.without_url().to_string())?;
        buf.extend_from_slice(&chunk);

        while let Some(line) = drain_next_stream_line(&mut buf)? {
            let Some(data) = stream_data_payload(&line) else {
                continue;
            };
            if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                let text = gemini_text(&event);
                if !text.is_empty() {
                    emitted.store(true, Ordering::Relaxed);
                    on_delta(&text, "");
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Connection test
// ---------------------------------------------------------------------------

const TEST_PROMPT: &str = "Who are you?";

/// Minimal system prompt for the connection-test request: keep the answer
/// short so the probe stays fast.
const TEST_SYSTEM_PROMPT: &str = "Answer directly in one short sentence.";

/// Result of an AI connection test.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiTestResult {
    pub ok: bool,
    pub category: String,
    pub message: String,
    pub latency_ms: u128,
}

fn test_result_failure(message: &str) -> AiTestResult {
    let category = classify_error(message);
    let prefixed = format!("[{category}]");
    let message = if message.starts_with(&prefixed) {
        message.to_string()
    } else {
        format!("{prefixed} {message}")
    };
    AiTestResult {
        ok: false,
        category: category.to_string(),
        message,
        latency_ms: 0,
    }
}

/// Probe the provider with a minimal non-streaming request (user message
/// "Who are you?", `max_tokens` = 16) and report the total elapsed time.
/// Errors are classified into a coarse category for UI guidance.
pub async fn test_connection(config: &AiConfig) -> AiTestResult {
    if let Err(error) = validate_endpoint(config) {
        return test_result_failure(&error);
    }
    // Auto-discover a model when the config has none.
    let mut resolved_config = config.clone();
    if resolved_config.model.trim().is_empty() {
        match list_models_core(&resolved_config).await {
            Ok(models) if !models.is_empty() => {
                resolved_config.model = models.into_iter().next().unwrap()
            }
            Ok(_) => {
                return test_result_failure(
                    "[modelDiscoveryUnsupported] Model discovery returned no models. Save the provider and enter a model ID manually.",
                );
            }
            Err(e) => return test_result_failure(&e),
        }
    }
    if let Err(e) = validate_config(&resolved_config) {
        return test_result_failure(&e);
    }
    let client = match build_ai_http_client(&resolved_config, 15) {
        Ok(client) => client,
        Err(e) => return test_result_failure(&e),
    };

    let start = std::time::Instant::now();
    let result = with_retry(&resolved_config, || {
        let client = client.clone();
        let config = resolved_config.clone();
        async move {
            let messages = vec![AiMessage {
                role: "user".to_string(),
                content: TEST_PROMPT.to_string(),
            }];
            complete_inner(&client, &config, TEST_SYSTEM_PROMPT, &messages, 16).await
        }
    })
    .await;
    let latency_ms = start.elapsed().as_millis();

    match result {
        Ok(_) => AiTestResult {
            ok: true,
            category: "ok".to_string(),
            message: format!("OK — {latency_ms}ms"),
            latency_ms,
        },
        Err(e) => {
            let category = classify_error(&e);
            let prefixed = format!("[{category}]");
            AiTestResult {
                ok: false,
                category: category.to_string(),
                message: if e.starts_with(&prefixed) {
                    e
                } else {
                    format!("{prefixed} {e}")
                },
                latency_ms,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Public model listing
// ---------------------------------------------------------------------------

/// List the model ids available to the configured provider.
pub async fn list_models(config: &AiConfig) -> Result<Vec<String>, String> {
    list_models_core(config).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::{TcpListener, TcpStream},
        sync::Mutex,
        time::Duration,
    };

    fn local_config(endpoint: String, api_style: AiApiStyle) -> AiConfig {
        AiConfig {
            provider: AiProvider::OpenaiCompatible,
            api_key: String::new(),
            auth_method: AiAuthMethod::Bearer,
            endpoint,
            model: "test-model".into(),
            models: Vec::new(),
            api_style,
            proxy_enabled: false,
            proxy_url: String::new(),
            enable_thinking: false,
            context_window: None,
            max_retries: 0,
        }
    }

    fn mock_server(
        handler: impl FnOnce(TcpStream) + Send + 'static,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut reader = BufReader::new(socket);
            loop {
                let mut line = String::new();
                assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
            }
            handler(reader.into_inner());
        });
        (format!("http://{addr}/v1"), worker)
    }

    #[tokio::test]
    async fn streams_chinese_sql_from_chat_completions() {
        let (endpoint, server) = mock_server(|mut socket| {
            let body = "data: {\"choices\":[{\"delta\":{\"content\":\"SELECT '中文';\"}}]}\r\n\r\ndata: [DONE]\r\n\r\n";
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let split = response.find('中').unwrap() + 1;
            socket.write_all(&response.as_bytes()[..split]).unwrap();
            std::thread::sleep(Duration::from_millis(20));
            socket.write_all(&response.as_bytes()[split..]).unwrap();
        });
        let config = local_config(endpoint, AiApiStyle::Completions);
        let output = Mutex::new(String::new());
        tokio::time::timeout(
            Duration::from_secs(3),
            stream_chat(&config, "", &[], 128, &Notify::new(), &|text, _| {
                output.lock().unwrap().push_str(text);
            }),
        )
        .await
        .unwrap()
        .unwrap();
        server.join().unwrap();
        assert_eq!(output.into_inner().unwrap(), "SELECT '中文';");
    }

    #[tokio::test]
    async fn cancels_while_provider_has_not_sent_headers() {
        let (started, waiting) = tokio::sync::oneshot::channel();
        let (release, stopped) = std::sync::mpsc::channel();
        let (endpoint, server) = mock_server(move |_socket| {
            started.send(()).unwrap();
            let _ = stopped.recv_timeout(Duration::from_secs(5));
        });
        let config = local_config(endpoint, AiApiStyle::Completions);
        let cancel = Notify::new();
        let (result, ()) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(
                stream_chat(&config, "", &[], 128, &cancel, &|_, _| {}),
                async {
                    waiting.await.unwrap();
                    cancel.notify_one();
                }
            )
        })
        .await
        .unwrap();
        release.send(()).unwrap();
        server.join().unwrap();
        assert!(is_cancelled_error(&result.unwrap_err()));
    }

    #[tokio::test]
    async fn responses_reports_in_band_provider_failure() {
        let (endpoint, server) = mock_server(|mut socket| {
            let body = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"provider failed\"}}}\n\n";
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        });
        let config = local_config(endpoint, AiApiStyle::Responses);
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            stream_chat(&config, "", &[], 128, &Notify::new(), &|_, _| {}),
        )
        .await
        .unwrap();
        server.join().unwrap();
        assert_eq!(result.unwrap_err(), "provider failed");
    }
}

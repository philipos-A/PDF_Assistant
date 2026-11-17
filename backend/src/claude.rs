//! Claude Messages API: request building and streaming over raw HTTP.
//!
//! Anthropic has no official Rust SDK, so this calls `POST /v1/messages`
//! directly with `reqwest` and parses the Server-Sent Events stream by hand.

use std::time::Duration;

use base64::Engine;
use futures_util::{Stream, StreamExt};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::Settings;
use crate::prompts::{SYSTEM_PROMPT, build_reader_context};

/// Value of the `anthropic-version` header.
pub const API_VERSION: &str = "2026-10-21";
/// Beta that enables server-side refusal fallbacks (`fallbacks: "default"`).
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const MAX_RETRIES: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// One conversation turn. User turns carry the reader context they were asked with,
/// so earlier turns can be rebuilt byte-for-byte and the prompt cache keeps hitting.
#[derive(Clone, Debug, Deserialize)]
pub struct Turn {
    pub role: Role,
    pub content: String,
    #[serde(default)]
    pub current_page: Option<u32>,
    #[serde(default)]
    pub visible_pages: Option<Vec<u32>>,
    #[serde(default)]
    pub selected_text: Option<String>,
}

/// Drop empty turns and enforce user/assistant alternation. The result starts with a
/// user turn and ends with an assistant turn, so the new question can follow it.
pub fn normalize_history(history: Vec<Turn>) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::new();
    for turn in history {
        if turn.content.trim().is_empty() || (out.is_empty() && turn.role != Role::User) {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.role == turn.role => *last = turn, // keep the later one
            _ => out.push(turn),
        }
    }
    if out.last().is_some_and(|t| t.role == Role::User) {
        out.pop();
    }
    out
}

fn user_text(turn: &Turn, page_count: Option<usize>) -> String {
    match turn.current_page {
        None => turn.content.clone(),
        Some(page) => {
            let ctx = build_reader_context(
                page,
                turn.visible_pages.as_deref().unwrap_or(&[]),
                turn.selected_text.as_deref(),
                page_count,
            );
            format!("{ctx}\n\n{}", turn.content)
        }
    }
}

/// Build the `messages` array. The PDF is the first block of the first user turn,
/// with a cache breakpoint, so system prompt + PDF form a cached prefix.
pub fn build_messages(
    pdf: &[u8],
    history: Vec<Turn>,
    question: Turn,
    page_count: Option<usize>,
) -> Vec<Value> {
    let document_block = json!({
        "type": "document",
        "source": {
            "type": "base64",
            "media_type": "application/pdf",
            "data": base64::engine::general_purpose::STANDARD.encode(pdf),
        },
        "cache_control": {"type": "ephemeral"},
    });

    let mut turns = normalize_history(history);
    turns.push(question);

    turns
        .iter()
        .enumerate()
        .map(|(i, turn)| match turn.role {
            Role::User => {
                let text = json!({"type": "text", "text": user_text(turn, page_count)});
                let content = if i == 0 {
                    vec![document_block.clone(), text]
                } else {
                    vec![text]
                };
                json!({"role": "user", "content": content})
            }
            Role::Assistant => json!({"role": "assistant", "content": turn.content}),
        })
        .collect()
}

/// The JSON body sent to `POST /v1/messages`.
pub fn request_body(settings: &Settings, messages: Vec<Value>) -> Value {
    let mut body = json!({
        "model": settings.claude_model,
        "max_tokens": settings.max_output_tokens,
        "system": SYSTEM_PROMPT,
        // Top-level breakpoint caches the growing conversation; the PDF block has its own.
        "cache_control": {"type": "ephemeral"},
        "output_config": {"effort": settings.claude_effort},
        "messages": messages,
        "stream": true,
    });
    if settings.enable_fallbacks {
        // On a safety-classifier decline, the API re-runs the request on a
        // fallback model inside the same stream.
        body["fallbacks"] = json!("default");
    }
    body
}

/// An event for the browser: `status`, `delta`, `done` or `error`.
#[derive(Debug, Clone, PartialEq)]
pub struct OutEvent {
    pub event: &'static str,
    pub data: Value,
}

impl OutEvent {
    fn new(event: &'static str, data: Value) -> Self {
        Self { event, data }
    }
    fn error(message: impl Into<String>) -> Self {
        Self::new("error", json!({"message": message.into()}))
    }
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        // Per-read timeout: generous, since the model may think for a while before writing.
        .read_timeout(Duration::from_secs(300))
        .build()
        .expect("failed to build HTTP client")
}

fn api_error_message(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    v.pointer("/error/message")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn status_error(settings: &Settings, status: StatusCode, body: &str) -> String {
    let detail = api_error_message(body).unwrap_or_else(|| body.chars().take(300).collect());
    match status.as_u16() {
        401 => {
            "The Anthropic API key was rejected. Check ANTHROPIC_API_KEY in backend/.env.".into()
        }
        403 => "This API key does not have access to the configured model.".into(),
        404 => format!(
            "Model '{}' was not found. Check CLAUDE_MODEL in backend/.env.",
            settings.claude_model
        ),
        429 => "Rate limited by the Anthropic API. Wait a moment and try again.".into(),
        529 => "Claude is overloaded right now. Try again in a moment.".into(),
        400 | 413 => format!("The request was rejected: {detail}"),
        code => format!("Anthropic API error ({code}): {detail}"),
    }
}

fn retryable(status: StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 409 | 429 | 500..=599)
}

fn retry_delay(resp: &reqwest::Response, attempt: u32) -> Duration {
    let header = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<f64>().ok());
    match header {
        Some(secs) => Duration::from_secs_f64(secs.clamp(0.0, 10.0)),
        None => Duration::from_millis(500 * 2u64.pow(attempt)),
    }
}

/// Send the request, retrying transient failures before any output has streamed.
async fn send(
    http: &reqwest::Client,
    settings: &Settings,
    body: &Value,
) -> Result<reqwest::Response, String> {
    let url = format!(
        "{}/v1/messages",
        settings.anthropic_base_url.trim_end_matches('/')
    );
    let mut attempt = 0;
    loop {
        let mut req = http
            .post(&url)
            .header("x-api-key", &settings.anthropic_api_key)
            .header("anthropic-version", API_VERSION)
            .header("accept", "text/event-stream")
            .json(body);
        if settings.enable_fallbacks {
            req = req.header("anthropic-beta", FALLBACK_BETA);
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => return Ok(resp),
            Ok(resp) if retryable(resp.status()) && attempt < MAX_RETRIES => {
                let delay = retry_delay(&resp, attempt);
                tracing::warn!(status = %resp.status(), ?delay, "retrying Anthropic request");
                tokio::time::sleep(delay).await;
            }
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                tracing::warn!(%status, body = %text, "Anthropic request failed");
                return Err(status_error(settings, status, &text));
            }
            Err(e) if (e.is_connect() || e.is_timeout()) && attempt < MAX_RETRIES => {
                tracing::warn!(error = %e, "retrying Anthropic request after connection error");
                tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempt))).await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not reach Anthropic");
                return Err(
                    "Could not reach the Anthropic API. Check your network connection.".into(),
                );
            }
        }
        attempt += 1;
    }
}

/// Usage numbers, merged from `message_start` and `message_delta`.
#[derive(Default)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_input_tokens: u64,
    cache_creation_input_tokens: u64,
}

impl Usage {
    fn merge(&mut self, v: Option<&Value>) {
        let Some(v) = v else { return };
        let get = |k: &str| v.get(k).and_then(Value::as_u64);
        if let Some(n) = get("input_tokens") {
            self.input_tokens = n;
        }
        if let Some(n) = get("output_tokens") {
            self.output_tokens = n;
        }
        if let Some(n) = get("cache_read_input_tokens") {
            self.cache_read_input_tokens = n;
        }
        if let Some(n) = get("cache_creation_input_tokens") {
            self.cache_creation_input_tokens = n;
        }
    }
}

/// Split a byte stream into SSE frames and yield each frame's (event, data).
fn sse_frames(
    bytes: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
) -> impl Stream<Item = Result<(String, String), reqwest::Error>> {
    async_stream::try_stream! {
        let mut bytes = bytes;
        let mut buf = String::new();
        let mut pending: Vec<u8> = Vec::new();
        while let Some(chunk) = bytes.next().await {
            pending.extend_from_slice(&chunk?);
            // Only decode complete UTF-8; keep a split multi-byte char for the next chunk.
            let valid = match std::str::from_utf8(&pending) {
                Ok(_) => pending.len(),
                Err(e) => e.valid_up_to(),
            };
            buf.push_str(&String::from_utf8_lossy(&pending[..valid]).replace("\r\n", "\n"));
            pending.drain(..valid);
            while let Some(idx) = buf.find("\n\n") {
                let frame: String = buf.drain(..idx + 2).collect();
                if let Some(parsed) = parse_frame(&frame) {
                    yield parsed;
                }
            }
        }
        if let Some(parsed) = parse_frame(&buf) {
            yield parsed;
        }
    }
}

fn parse_frame(frame: &str) -> Option<(String, String)> {
    let mut event = String::from("message");
    let mut data = Vec::new();
    for line in frame.lines() {
        if let Some(v) = line.strip_prefix("event:") {
            event = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("data:") {
            data.push(v.strip_prefix(' ').unwrap_or(v));
        }
    }
    (!data.is_empty()).then(|| (event, data.join("\n")))
}

/// Call Claude and translate its stream into events for the browser.
pub fn stream_answer(
    http: reqwest::Client,
    settings: Settings,
    messages: Vec<Value>,
) -> impl Stream<Item = OutEvent> {
    async_stream::stream! {
        let body = request_body(&settings, messages);
        let resp = match send(&http, &settings, &body).await {
            Ok(r) => r,
            Err(message) => {
                yield OutEvent::error(message);
                return;
            }
        };

        let mut model = settings.claude_model.clone();
        let mut usage = Usage::default();
        let mut stop_reason: Option<String> = None;
        let mut finished = false;

        let frames = sse_frames(resp.bytes_stream());
        futures_util::pin_mut!(frames);
        while let Some(frame) = frames.next().await {
            let (event, data) = match frame {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!(error = %e, "stream interrupted");
                    yield OutEvent::error("The connection to Claude was interrupted.");
                    return;
                }
            };
            let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
            match event.as_str() {
                "message_start" => {
                    if let Some(m) = v.pointer("/message/model").and_then(Value::as_str) {
                        model = m.to_string();
                    }
                    usage.merge(v.pointer("/message/usage"));
                }
                "content_block_start" => {
                    let state = match v.pointer("/content_block/type").and_then(Value::as_str) {
                        Some("thinking") => Some("thinking"),
                        Some("fallback") => Some("fallback"),
                        Some("text") => Some("writing"),
                        _ => None,
                    };
                    if let Some(state) = state {
                        yield OutEvent::new("status", json!({"state": state}));
                    }
                }
                "content_block_delta" => {
                    if v.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta")
                        && let Some(text) = v.pointer("/delta/text").and_then(Value::as_str)
                    {
                        yield OutEvent::new("delta", json!({"text": text}));
                    }
                }
                "message_delta" => {
                    if let Some(r) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                        stop_reason = Some(r.to_string());
                    }
                    usage.merge(v.get("usage"));
                }
                "message_stop" => finished = true,
                "error" => {
                    let kind = v.pointer("/error/type").and_then(Value::as_str).unwrap_or("");
                    let msg = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("unknown error");
                    yield OutEvent::error(if kind == "overloaded_error" {
                        "Claude is overloaded right now. Try again in a moment.".to_string()
                    } else {
                        format!("Claude returned an error: {msg}")
                    });
                    return;
                }
                _ => {} // ping, content_block_stop, unknown future events
            }
        }

        if stop_reason.as_deref() == Some("refusal") {
            yield OutEvent::error("Claude declined to answer this question. Try rephrasing it.");
            return;
        }
        if !finished && stop_reason.is_none() {
            yield OutEvent::error("The response from Claude ended unexpectedly.");
            return;
        }
        yield OutEvent::new("done", json!({
            "stop_reason": stop_reason,
            "model": model,
            "usage": {
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
                "cache_read_input_tokens": usage.cache_read_input_tokens,
                "cache_creation_input_tokens": usage.cache_creation_input_tokens,
            },
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(role: Role, content: &str) -> Turn {
        Turn {
            role,
            content: content.into(),
            current_page: None,
            visible_pages: None,
            selected_text: None,
        }
    }

    #[test]
    fn normalize_enforces_alternation() {
        let h = vec![
            turn(Role::Assistant, "orphan"),
            turn(Role::User, "q1"),
            turn(Role::User, "q1 retry"),
            turn(Role::Assistant, ""),
            turn(Role::Assistant, "a1"),
            turn(Role::User, "dangling"),
        ];
        let out = normalize_history(h);
        let got: Vec<_> = out.iter().map(|t| (t.role, t.content.as_str())).collect();
        assert_eq!(got, vec![(Role::User, "q1 retry"), (Role::Assistant, "a1")]);
    }

    #[test]
    fn parse_frame_handles_multiline_data() {
        assert_eq!(
            parse_frame("event: delta\ndata: a\ndata: b\n\n"),
            Some(("delta".into(), "a\nb".into()))
        );
        assert_eq!(parse_frame(": keep-alive\n\n"), None);
    }
}

//! Claude Messages API: request building and streaming over raw HTTP.
//!
//! Anthropic has no official Rust SDK, so this calls `POST /v1/messages`
//! directly with `reqwest` and parses the Server-Sent Events stream by hand.

use std::time::Duration;

use base64::Engine;
use futures_util::{Stream, StreamExt};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::Settings;
use crate::prompts::{ReaderContext, SYSTEM_PROMPT};

/// Value of the `anthropic-version` header.
pub const API_VERSION: &str = "2026-10-21";
/// Beta that enables server-side refusal fallbacks (`fallbacks: "default"`).
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const MAX_RETRIES: u32 = 2;

const OVERLOADED: &str = "Claude is overloaded right now. Try again in a moment.";
/// Sent when Claude declines. The frontend matches on `refused: true`, not on this text.
const REFUSED: &str = "Claude declined to answer this question. Try rephrasing it.";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Sent flattened (`current_page`, `visible_pages`, …); absent on assistant turns.
    #[serde(flatten)]
    pub context: Option<ReaderContext>,
}

/// Drop empty turns, strip reader context from assistant turns, and enforce
/// user/assistant alternation. The result starts with a user turn and ends with an
/// assistant turn, so the new question can follow it.
pub fn normalize_history(history: Vec<Turn>) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::new();
    for mut turn in history {
        if turn.role == Role::Assistant {
            turn.context = None;
        }
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

impl Turn {
    /// The text Claude sees for a user turn: reader context, then the question.
    fn user_text(&self, page_count: Option<usize>) -> String {
        match &self.context {
            None => self.content.clone(),
            Some(ctx) => format!("{}\n\n{}", ctx.render(page_count), self.content),
        }
    }
}

/// Where the PDF comes from: inline bytes, or a file uploaded once to the Files API.
pub enum DocumentSource<'a> {
    Base64(&'a [u8]),
    File(&'a str),
}

/// The PDF as a `document` block, with a cache breakpoint so system prompt + PDF
/// form a cached prefix. With `citations`, Claude can cite the pages it draws on.
pub fn document_block(source: &DocumentSource<'_>, citations: bool) -> Value {
    let source = match source {
        DocumentSource::Base64(pdf) => json!({
            "type": "base64",
            "media_type": "application/pdf",
            "data": base64::engine::general_purpose::STANDARD.encode(pdf),
        }),
        DocumentSource::File(file_id) => json!({"type": "file", "file_id": file_id}),
    };
    let mut block = json!({
        "type": "document",
        "source": source,
        "cache_control": {"type": "ephemeral"},
    });
    if citations {
        block["citations"] = json!({"enabled": true});
    }
    block
}

/// Build the `messages` array. The document block is the first block of the first
/// user turn and appears nowhere else.
pub fn build_messages(
    document: Value,
    history: Vec<Turn>,
    question: Turn,
    page_count: Option<usize>,
) -> Vec<Value> {
    let mut turns = normalize_history(history);
    turns.push(question);

    let mut document = Some(document);
    turns
        .iter()
        .map(|turn| match turn.role {
            Role::User => {
                let text = json!({"type": "text", "text": turn.user_text(page_count)});
                let content = match document.take() {
                    Some(doc) => vec![doc, text],
                    None => vec![text],
                };
                json!({"role": "user", "content": content})
            }
            Role::Assistant => json!({"role": "assistant", "content": turn.content}),
        })
        .collect()
}

/// A passage of the Document that an Answer cites. `n` is the number its marker shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Citation {
    pub n: usize,
    pub cited_text: String,
    pub start_page: u32,
    /// Last page the passage is on (inclusive).
    pub end_page: u32,
}

/// The marker written into an Answer where citation `n` applies.
pub fn citation_marker(n: usize) -> String {
    format!("[{n}](#cite-{n})")
}

/// Remove citation markers, so earlier Answers go back to Claude as it wrote them.
pub fn strip_citation_markers(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        match marker_len(tail) {
            Some(len) => rest = &tail[len..],
            None => {
                out.push('[');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Length of a `[n](#cite-n)` marker at the start of `s`, if there is one.
fn marker_len(s: &str) -> Option<usize> {
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    let a = digits(&s[1..]);
    if a == 0 || !s[1 + a..].starts_with("](#cite-") {
        return None;
    }
    let after = 1 + a + "](#cite-".len();
    let b = digits(&s[after..]);
    (b > 0 && s[after + b..].starts_with(')')).then_some(after + b + 1)
}

/// Turn a `page_location` citation from the stream into a numbered Citation,
/// reusing the number of an identical passage already cited.
fn page_citation(v: &Value, citations: &mut Vec<Citation>) -> Option<(Citation, bool)> {
    let c = v.pointer("/delta/citation")?;
    if c.get("type").and_then(Value::as_str) != Some("page_location") {
        return None;
    }
    let cited_text = c.get("cited_text")?.as_str()?.trim().to_string();
    let start_page = u32::try_from(c.get("start_page_number")?.as_u64()?).ok()?;
    // The API's end page is exclusive.
    let end_page = c
        .get("end_page_number")
        .and_then(Value::as_u64)
        .and_then(|e| u32::try_from(e).ok())
        .map_or(start_page, |e| e.saturating_sub(1).max(start_page));
    if let Some(existing) = citations
        .iter()
        .find(|x| x.cited_text == cited_text && x.start_page == start_page)
    {
        return Some((existing.clone(), false));
    }
    let citation = Citation {
        n: citations.len() + 1,
        cited_text,
        start_page,
        end_page,
    };
    citations.push(citation.clone());
    Some((citation, true))
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

/// An event for the browser: `status`, `delta`, `citation`, `done` or `error`.
/// One more, `file_missing`, is for the caller only: the uploaded PDF is gone and
/// should be uploaded again before retrying.
#[derive(Debug, Clone, PartialEq)]
pub struct OutEvent {
    pub event: &'static str,
    pub data: Value,
}

impl OutEvent {
    pub fn new(event: &'static str, data: Value) -> Self {
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
        529 => OVERLOADED.into(),
        400 | 413 => format!("The request was rejected: {detail}"),
        code => format!("Anthropic API error ({code}): {detail}"),
    }
}

fn retryable(status: StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 409 | 429 | 500..=599)
}

/// Stream `error` types that mean "try again", when no text has streamed yet.
fn retryable_stream_error(kind: &str) -> bool {
    matches!(kind, "overloaded_error" | "api_error" | "rate_limit_error")
}

/// Longest server-requested wait the reader sits through; anything longer fails at once.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(10);

fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(500 * 2u64.pow(attempt))
}

/// The wait the server asked for: `retry-after-ms`, else `retry-after` in seconds.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let number = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
    };
    number("retry-after-ms")
        .map(|ms| Duration::from_secs_f64(ms / 1000.0))
        .or_else(|| number("retry-after").map(Duration::from_secs_f64))
}

/// Why an attempt failed: try again after a delay, the uploaded PDF is gone, or
/// give up with a message.
enum Failure {
    Retry(Duration),
    FileMissing,
    Fatal(String),
}

/// A 404 naming a file means the Files API copy of the PDF no longer exists
/// (deleted, or the API key now belongs to another workspace).
fn file_missing(status: StatusCode, body: &str) -> bool {
    status == StatusCode::NOT_FOUND
        && api_error_message(body).is_some_and(|m| m.to_ascii_lowercase().contains("file"))
}

fn api_url(settings: &Settings, path: &str) -> String {
    format!(
        "{}{path}",
        settings.anthropic_base_url.trim_end_matches('/')
    )
}

/// Characters the Files API rejects in file names.
fn safe_upload_name(filename: &str) -> String {
    let name: String = filename
        .chars()
        .filter(|c| {
            !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\\' | '/') && !c.is_control()
        })
        .take(200)
        .collect();
    if name.trim().is_empty() {
        "document.pdf".into()
    } else {
        name
    }
}

/// Upload a PDF to the Files API and return its `file_id`.
pub async fn upload_file(
    http: &reqwest::Client,
    settings: &Settings,
    pdf: Vec<u8>,
    filename: &str,
) -> Result<String, String> {
    let part = reqwest::multipart::Part::bytes(pdf)
        .file_name(safe_upload_name(filename))
        .mime_str("application/pdf")
        .map_err(|e| e.to_string())?;
    let resp = http
        .post(api_url(settings, "/v1/files"))
        .header("x-api-key", &settings.anthropic_api_key)
        .header("anthropic-version", API_VERSION)
        .multipart(reqwest::multipart::Form::new().part("file", part))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "{status}: {}",
            api_error_message(&body).unwrap_or(body)
        ));
    }
    serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| v.get("id")?.as_str().map(str::to_string))
        .ok_or_else(|| "upload response had no file id".into())
}

/// Delete an uploaded file. A file that is already gone counts as deleted.
pub async fn delete_file(
    http: &reqwest::Client,
    settings: &Settings,
    file_id: &str,
) -> Result<(), String> {
    let resp = http
        .delete(api_url(settings, &format!("/v1/files/{file_id}")))
        .header("x-api-key", &settings.anthropic_api_key)
        .header("anthropic-version", API_VERSION)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    match resp.status() {
        s if s.is_success() || s == StatusCode::NOT_FOUND => Ok(()),
        s => Err(format!("{s}: {}", resp.text().await.unwrap_or_default())),
    }
}

/// Send the request once. Transient failures come back as `Failure::Retry`
/// while `attempt` is below `MAX_RETRIES`.
async fn send_once(
    http: &reqwest::Client,
    settings: &Settings,
    body: &Value,
    attempt: u32,
) -> Result<reqwest::Response, Failure> {
    let mut req = http
        .post(api_url(settings, "/v1/messages"))
        .header("x-api-key", &settings.anthropic_api_key)
        .header("anthropic-version", API_VERSION)
        .header("accept", "text/event-stream")
        .json(body);
    if settings.enable_fallbacks {
        req = req.header("anthropic-beta", FALLBACK_BETA);
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => Ok(resp),
        Ok(resp) => {
            let status = resp.status();
            // The API can override the usual status-based decision.
            let should_retry = match resp
                .headers()
                .get("x-should-retry")
                .and_then(|v| v.to_str().ok())
            {
                Some("true") => true,
                Some("false") => false,
                _ => retryable(status),
            };
            if should_retry && attempt < MAX_RETRIES {
                match retry_after(resp.headers()) {
                    // The server wants a longer wait than is worth making the reader sit through.
                    Some(wait) if wait > MAX_RETRY_AFTER => {}
                    wait => {
                        let delay = wait.unwrap_or_else(|| backoff(attempt));
                        tracing::warn!(%status, ?delay, "retrying Anthropic request");
                        return Err(Failure::Retry(delay));
                    }
                }
            }
            let text = resp.text().await.unwrap_or_default();
            tracing::warn!(%status, body = %text, "Anthropic request failed");
            if file_missing(status, &text) {
                return Err(Failure::FileMissing);
            }
            Err(Failure::Fatal(status_error(settings, status, &text)))
        }
        Err(e) if (e.is_connect() || e.is_timeout()) && attempt < MAX_RETRIES => {
            tracing::warn!(error = %e, "retrying Anthropic request after connection error");
            Err(Failure::Retry(backoff(attempt)))
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not reach Anthropic");
            Err(Failure::Fatal(
                "Could not reach the Anthropic API. Check your network connection.".into(),
            ))
        }
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
            buf.push_str(&decode_utf8_prefix(&mut pending).replace("\r\n", "\n"));
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

/// Decode and remove everything from `pending` except a trailing, still-incomplete
/// multi-byte char (kept for the next chunk). Invalid bytes become U+FFFD.
fn decode_utf8_prefix(pending: &mut Vec<u8>) -> String {
    let mut out = String::new();
    loop {
        match std::str::from_utf8(pending) {
            Ok(s) => {
                out.push_str(s);
                pending.clear();
                return out;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                out.push_str(std::str::from_utf8(&pending[..valid]).expect("valid prefix"));
                match e.error_len() {
                    Some(bad) => {
                        out.push('\u{FFFD}');
                        pending.drain(..valid + bad);
                    }
                    None => {
                        pending.drain(..valid);
                        return out;
                    }
                }
            }
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
        let mut attempt = 0;
        // Each pass is one request. Transient failures, including a stream `error`
        // that arrives before any text, start the next pass, up to MAX_RETRIES times.
        'attempt: loop {
            let resp = match send_once(&http, &settings, &body, attempt).await {
                Ok(r) => r,
                Err(Failure::Retry(delay)) => {
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                    continue 'attempt;
                }
                Err(Failure::FileMissing) => {
                    yield OutEvent::new("file_missing", json!({}));
                    return;
                }
                Err(Failure::Fatal(message)) => {
                    yield OutEvent::error(message);
                    return;
                }
            };

            let mut model = settings.claude_model.clone();
            let mut usage = Usage::default();
            let mut stop_reason: Option<String> = None;
            let mut finished = false;
            let mut sent_text = false;
            let mut citations: Vec<Citation> = Vec::new();
            // Markers already written into the current text block, so a passage cited
            // twice in one block gets one marker.
            let mut block_markers: Vec<usize> = Vec::new();

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
                        // After a mid-answer fallback, message_start still names the
                        // requested model; the fallback block names the one now answering.
                        if let Some(m) = v.pointer("/content_block/to/model").and_then(Value::as_str) {
                            model = m.to_string();
                        }
                        block_markers.clear();
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
                    "content_block_delta" => match v.pointer("/delta/type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            if let Some(text) = v.pointer("/delta/text").and_then(Value::as_str) {
                                sent_text = true;
                                yield OutEvent::new("delta", json!({"text": text}));
                            }
                        }
                        // A citation applies to the text block it arrives in: its details
                        // go in a `citation` event, and a numbered marker follows the text.
                        Some("citations_delta") => {
                            if let Some((citation, new)) = page_citation(&v, &mut citations) {
                                if new {
                                    yield OutEvent::new("citation", json!(citation));
                                }
                                if !block_markers.contains(&citation.n) {
                                    block_markers.push(citation.n);
                                    yield OutEvent::new("delta", json!({"text": citation_marker(citation.n)}));
                                }
                            }
                        }
                        _ => {}
                    },
                    "message_delta" => {
                        if let Some(r) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                            stop_reason = Some(r.to_string());
                        }
                        usage.merge(v.get("usage"));
                    }
                    "message_stop" => finished = true,
                    "error" => {
                        let kind = v.pointer("/error/type").and_then(Value::as_str).unwrap_or("");
                        if !sent_text && attempt < MAX_RETRIES && retryable_stream_error(kind) {
                            let delay = backoff(attempt);
                            tracing::warn!(kind, ?delay, "retrying after stream error");
                            tokio::time::sleep(delay).await;
                            attempt += 1;
                            continue 'attempt;
                        }
                        let msg = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("unknown error");
                        yield OutEvent::error(if kind == "overloaded_error" {
                            OVERLOADED.to_string()
                        } else {
                            format!("Claude returned an error: {msg}")
                        });
                        return;
                    }
                    _ => {} // ping, content_block_stop, unknown future events
                }
            }

            if stop_reason.as_deref() == Some("refusal") {
                yield OutEvent::new("error", json!({"message": REFUSED, "refused": true}));
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
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(role: Role, content: &str) -> Turn {
        Turn {
            role,
            content: content.into(),
            context: None,
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
    fn strip_removes_only_citation_markers() {
        let text =
            "Attention[1](#cite-1) is [all](#x) you need[2](#cite-2)[3](#cite-3). [4](#cite-x) [";
        assert_eq!(
            strip_citation_markers(text),
            "Attention is [all](#x) you need. [4](#cite-x) ["
        );
        assert_eq!(strip_citation_markers(&citation_marker(12)), "");
    }

    #[test]
    fn page_citations_are_numbered_and_deduplicated() {
        let delta = |text: &str, start: u64, end: u64| {
            json!({"delta": {"type": "citations_delta", "citation": {
                "type": "page_location", "cited_text": text, "document_index": 0,
                "start_page_number": start, "end_page_number": end,
            }}})
        };
        let mut all = Vec::new();
        let (a, new_a) = page_citation(&delta(" Softmax. ", 3, 4), &mut all).unwrap();
        assert!(new_a);
        assert_eq!(
            (a.n, a.cited_text.as_str(), a.start_page, a.end_page),
            (1, "Softmax.", 3, 3)
        );
        let (b, _) = page_citation(&delta("Spans pages.", 4, 6), &mut all).unwrap();
        assert_eq!(
            (b.n, b.start_page, b.end_page),
            (2, 4, 5),
            "end page is made inclusive"
        );
        let (again, new_again) = page_citation(&delta("Softmax.", 3, 4), &mut all).unwrap();
        assert_eq!((again.n, new_again), (1, false));
        let other = json!({"delta": {"type": "citations_delta", "citation": {"type": "char_location", "cited_text": "x"}}});
        assert!(page_citation(&other, &mut all).is_none());
    }

    #[test]
    fn document_block_uses_file_or_base64_source() {
        let file = document_block(&DocumentSource::File("file_1"), true);
        assert_eq!(file["source"], json!({"type": "file", "file_id": "file_1"}));
        assert_eq!(file["citations"]["enabled"], true);
        assert_eq!(file["cache_control"]["type"], "ephemeral");
        let inline = document_block(&DocumentSource::Base64(b"%PDF"), false);
        assert_eq!(inline["source"]["type"], "base64");
        assert!(inline.get("citations").is_none());
    }

    #[test]
    fn upload_names_drop_forbidden_characters() {
        assert_eq!(safe_upload_name("a<b>:c\"/d|e?*.pdf"), "abcde.pdf");
        assert_eq!(safe_upload_name("\u{1}"), "document.pdf");
    }

    #[test]
    fn parse_frame_handles_multiline_data() {
        assert_eq!(
            parse_frame("event: delta\ndata: a\ndata: b\n\n"),
            Some(("delta".into(), "a\nb".into()))
        );
        assert_eq!(parse_frame(": keep-alive\n\n"), None);
    }

    async fn frames_of(chunks: Vec<&'static [u8]>) -> Vec<(String, String)> {
        let input = futures_util::stream::iter(
            chunks
                .into_iter()
                .map(|c| Ok::<_, reqwest::Error>(bytes::Bytes::from_static(c))),
        );
        sse_frames(input).map(|f| f.expect("frame")).collect().await
    }

    #[tokio::test]
    async fn sse_frames_survive_invalid_utf8() {
        let got = frames_of(vec![
            b"event: delta\ndata: a\xff\n\n",
            b"event: delta\ndata: b\n\n",
        ])
        .await;
        assert_eq!(
            got,
            vec![
                ("delta".to_string(), "a\u{fffd}".to_string()),
                ("delta".to_string(), "b".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn sse_frames_join_split_multibyte_chars() {
        // "é" is 0xC3 0xA9, split across two chunks.
        let got = frames_of(vec![b"event: delta\ndata: caf\xc3", b"\xa9\n\n"]).await;
        assert_eq!(got, vec![("delta".to_string(), "café".to_string())]);
    }
}

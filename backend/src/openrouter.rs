//! Fallback Answers through OpenRouter, used when Claude can't serve a Question
//! (usage or credit ran out, overloaded, or unreachable).
//!
//! One request lists several models (`OPENROUTER_MODELS`, e.g. Gemini then GPT);
//! OpenRouter tries them in order, so an outage of the first is handled too. The
//! request carries the same system prompt, PDF, saved Turns and Reader Context as
//! a Claude request, in OpenRouter's chat-completions format, and the stream is
//! translated into the same events for the browser.
//!
//! These models have no built-in citations, so they're asked to tag the passages
//! they rely on (`{{cite p=4: exact quote}}`). `CiteTags` turns each tag into the
//! same `citation` event and `[n](#cite-n)` marker that Claude's citations produce.

use base64::Engine;
use futures_util::{Stream, StreamExt};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::claude::{
    Citation, OutEvent, Role, Turn, backoff, citation_marker, normalize_history, number_citation,
    sse_frames,
};
use crate::config::Settings;
use crate::prompts::SYSTEM_PROMPT;

const MAX_RETRIES: u32 = 1;
/// Sent when a fallback model declines (`finish_reason: "content_filter"`).
const REFUSED: &str = "The model declined to answer this question. Try rephrasing it.";

/// Added to the system prompt so fallback models mark the passages they use.
pub const CITATION_INSTRUCTIONS: &str = "\
Citing passages: the reader's screen highlights the passages you rely on. Right \
before a sentence that draws on a specific passage of the paper, write a tag \
{{cite p=PAGE: exact quote}}, where PAGE is the PDF page number (the file's \
first page is 1) and the exact quote is one or two sentences copied word for \
word from that page. Use one tag per passage, never explain or mention the \
tags, and keep writing page references such as \"p. 4\" in your prose too.
";

/// The JSON body sent to OpenRouter's `POST /chat/completions`.
pub fn request_body(
    settings: &Settings,
    pdf: &[u8],
    filename: &str,
    history: Vec<Turn>,
    question: Turn,
    page_count: Option<usize>,
) -> Value {
    let system = if settings.enable_citations {
        format!("{SYSTEM_PROMPT}\n{CITATION_INSTRUCTIONS}")
    } else {
        SYSTEM_PROMPT.to_string()
    };
    let mut messages = vec![json!({"role": "system", "content": system})];

    let mut turns = normalize_history(history);
    turns.push(question);
    // The PDF goes with the first user turn, as in a Claude request.
    let mut file = Some(json!({
        "type": "file",
        "file": {
            "filename": filename,
            "file_data": format!(
                "data:application/pdf;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(pdf)
            ),
        },
    }));
    for turn in &turns {
        messages.push(match turn.role {
            Role::User => {
                let text = json!({"type": "text", "text": turn.user_text(page_count)});
                let content = match file.take() {
                    Some(f) => vec![f, text],
                    None => vec![text],
                };
                json!({"role": "user", "content": content})
            }
            Role::Assistant => json!({"role": "assistant", "content": turn.content}),
        });
    }

    json!({
        "models": settings.openrouter_models,
        "messages": messages,
        "max_tokens": settings.max_output_tokens,
        "reasoning": {"effort": settings.claude_effort},
        "stream": true,
    })
}

/// Turns `{{cite p=N: quote}}` tags in streamed text into Citations and markers.
/// Text that might be the start of a tag is held back until it's complete.
#[derive(Default)]
pub struct CiteTags {
    pending: String,
    citations: Vec<Citation>,
}

const TAG_OPEN: &str = "{{cite";
const TAG_CLOSE: &str = "}}";
/// A "tag" longer than this was never going to close; show it as text.
const MAX_TAG_LEN: usize = 1200;

impl CiteTags {
    /// Feed streamed text; returns the `delta` and `citation` events to send, in order.
    pub fn push(&mut self, text: &str) -> Vec<OutEvent> {
        self.pending.push_str(text);
        let mut out = Vec::new();
        loop {
            let Some(start) = self.pending.find(TAG_OPEN) else {
                // Hold back an ending like "{{ci" that may become a tag.
                let keep = (1..TAG_OPEN.len())
                    .rev()
                    .find(|&k| self.pending.ends_with(&TAG_OPEN[..k]))
                    .unwrap_or(0);
                let text: String = self.pending.drain(..self.pending.len() - keep).collect();
                push_text(&mut out, text);
                break;
            };
            push_text(&mut out, self.pending.drain(..start).collect());
            match self.pending.find(TAG_CLOSE) {
                Some(end) => {
                    let tag: String = self.pending.drain(..end + TAG_CLOSE.len()).collect();
                    // A malformed tag is dropped rather than shown.
                    if let Some((text, start_page, end_page)) = parse_tag(&tag) {
                        let (citation, new) =
                            number_citation(&mut self.citations, text, start_page, end_page);
                        if new {
                            out.push(OutEvent::new("citation", json!(citation)));
                        }
                        out.push(delta(citation_marker(citation.n)));
                    }
                }
                None if self.pending.len() > MAX_TAG_LEN => {
                    push_text(&mut out, std::mem::take(&mut self.pending));
                    break;
                }
                None => break,
            }
        }
        out
    }

    /// The stream ended: send any held-back text (an unfinished tag is dropped).
    pub fn finish(&mut self) -> Vec<OutEvent> {
        let rest = std::mem::take(&mut self.pending);
        let mut out = Vec::new();
        if !rest.starts_with(TAG_OPEN) {
            push_text(&mut out, rest);
        }
        out
    }
}

fn delta(text: String) -> OutEvent {
    OutEvent::new("delta", json!({"text": text}))
}

fn push_text(out: &mut Vec<OutEvent>, text: String) {
    if !text.is_empty() {
        out.push(delta(text));
    }
}

/// `{{cite p=4: quote}}`, `{{cite p4-5: quote}}` or `{{cite p. 4: "quote"}}` →
/// (quote, first page, last page).
fn parse_tag(tag: &str) -> Option<(String, u32, u32)> {
    let inner = tag.strip_prefix(TAG_OPEN)?.strip_suffix(TAG_CLOSE)?;
    let (pages, quote) = inner.split_once(':')?;
    let pages = pages
        .trim()
        .trim_start_matches(['p', 'P'])
        .trim_start_matches(['.', '='])
        .trim();
    let (first, last) = match pages.split_once(['-', '–']) {
        Some((a, b)) => (a.trim(), b.trim()),
        None => (pages, pages),
    };
    let start: u32 = first.parse().ok().filter(|&p| p >= 1)?;
    let end: u32 = last.parse().ok().filter(|&p| p >= start).unwrap_or(start);
    let quote = quote
        .trim()
        .trim_matches(['"', '“', '”', '\''])
        .trim()
        .to_string();
    (!quote.is_empty()).then_some((quote, start, end))
}

fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.pointer("/error/message")?.as_str().map(str::to_string))
        .unwrap_or_else(|| body.chars().take(300).collect())
}

fn status_error(status: StatusCode, body: &str) -> String {
    match status.as_u16() {
        401 => "OpenRouter rejected the API key. Check OPENROUTER_API_KEY.".into(),
        402 => "Your OpenRouter account is out of credit.".into(),
        429 => "The fallback models are rate limited right now.".into(),
        code => format!(
            "The fallback models are unavailable ({code}): {}",
            error_message(body)
        ),
    }
}

fn retryable(status: StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 429 | 500..=599)
}

async fn send(
    http: &reqwest::Client,
    settings: &Settings,
    body: &Value,
) -> Result<reqwest::Response, String> {
    let url = format!(
        "{}/chat/completions",
        settings.openrouter_base_url.trim_end_matches('/')
    );
    let mut attempt = 0;
    loop {
        let result = http
            .post(&url)
            .bearer_auth(settings.openrouter_api_key.trim())
            .header("accept", "text/event-stream")
            .json(body)
            .send()
            .await;
        match result {
            Ok(resp) if resp.status().is_success() => return Ok(resp),
            Ok(resp) if retryable(resp.status()) && attempt < MAX_RETRIES => {
                tracing::warn!(status = %resp.status(), "retrying OpenRouter request");
            }
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                tracing::warn!(%status, body = %text, "OpenRouter request failed");
                return Err(status_error(status, &text));
            }
            Err(e) if (e.is_connect() || e.is_timeout()) && attempt < MAX_RETRIES => {
                tracing::warn!(error = %e, "retrying OpenRouter request after connection error");
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not reach OpenRouter");
                return Err("Could not reach OpenRouter. Check your network connection.".into());
            }
        }
        tokio::time::sleep(backoff(attempt)).await;
        attempt += 1;
    }
}

/// Call the fallback models and translate their stream into events for the browser:
/// `status`, `delta`, `citation`, then `done` (or `error`).
pub fn stream_answer(
    http: reqwest::Client,
    settings: Settings,
    body: Value,
) -> impl Stream<Item = OutEvent> {
    async_stream::stream! {
        let resp = match send(&http, &settings, &body).await {
            Ok(r) => r,
            Err(message) => {
                yield OutEvent::error(message);
                return;
            }
        };

        let mut model: Option<String> = None;
        let mut finish_reason: Option<String> = None;
        let mut usage = json!({});
        let mut writing = false;
        let mut thinking = false;
        let mut done = false;
        let mut tags = CiteTags::default();

        let frames = sse_frames(resp.bytes_stream());
        futures_util::pin_mut!(frames);
        while let Some(frame) = frames.next().await {
            let data = match frame {
                Ok((_, data)) => data,
                Err(e) => {
                    tracing::warn!(error = %e, "OpenRouter stream interrupted");
                    yield OutEvent::error("The connection to the fallback model was interrupted.");
                    return;
                }
            };
            if data.trim() == "[DONE]" {
                done = true;
                break;
            }
            let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
            if let Some(m) = v.get("model").and_then(Value::as_str) {
                model = Some(m.to_string());
            }
            if let Some(err) = v.get("error") {
                let message = err.get("message").and_then(Value::as_str).unwrap_or("unknown error");
                yield OutEvent::error(format!("The fallback model returned an error: {message}"));
                return;
            }
            if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
                usage = u.clone();
            }
            let choice = v.pointer("/choices/0");
            if choice.and_then(|c| c.pointer("/delta/reasoning")).is_some_and(|r| r.is_string()) && !thinking && !writing {
                thinking = true;
                yield OutEvent::new("status", json!({"state": "thinking"}));
            }
            if let Some(text) = choice.and_then(|c| c.pointer("/delta/content")).and_then(Value::as_str)
                && !text.is_empty()
            {
                if !writing {
                    writing = true;
                    yield OutEvent::new("status", json!({"state": "writing"}));
                }
                for event in tags.push(text) {
                    yield event;
                }
            }
            if let Some(r) = choice.and_then(|c| c.get("finish_reason")).and_then(Value::as_str) {
                finish_reason = Some(r.to_string());
            }
        }
        for event in tags.finish() {
            yield event;
        }

        match finish_reason.as_deref() {
            Some("content_filter") => {
                yield OutEvent::new("error", json!({"message": REFUSED, "refused": true}));
                return;
            }
            Some("error") => {
                yield OutEvent::error("The fallback model stopped with an error.");
                return;
            }
            None if !done => {
                yield OutEvent::error("The response from the fallback model ended unexpectedly.");
                return;
            }
            _ => {}
        }
        let stop_reason = match finish_reason.as_deref() {
            Some("length") => "max_tokens",
            _ => "end_turn",
        };
        let tokens = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
        yield OutEvent::new("done", json!({
            "stop_reason": stop_reason,
            "model": model.unwrap_or_else(|| settings.openrouter_models.first().cloned().unwrap_or_default()),
            "usage": {
                "input_tokens": tokens("prompt_tokens"),
                "output_tokens": tokens("completion_tokens"),
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
            },
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(events: &[OutEvent]) -> String {
        events
            .iter()
            .filter(|e| e.event == "delta")
            .map(|e| e.data["text"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn tags_become_citations_and_markers_even_when_split() {
        let mut tags = CiteTags::default();
        let mut events = Vec::new();
        for chunk in [
            "Intro. {",
            "{cite p=4: The model uses",
            " attention.}}It attends. {{ci",
            "te p. 4: \"The model uses attention.\"}}Again.",
        ] {
            events.extend(tags.push(chunk));
        }
        events.extend(tags.finish());
        assert_eq!(
            texts(&events),
            "Intro. [1](#cite-1)It attends. [1](#cite-1)Again."
        );
        let cites: Vec<_> = events.iter().filter(|e| e.event == "citation").collect();
        assert_eq!(cites.len(), 1, "the same passage is numbered once");
        assert_eq!(cites[0].data["cited_text"], "The model uses attention.");
        assert_eq!(
            (
                cites[0].data["start_page"].as_u64(),
                cites[0].data["end_page"].as_u64()
            ),
            (Some(4), Some(4))
        );
    }

    #[test]
    fn page_ranges_and_bad_tags() {
        assert_eq!(
            parse_tag("{{cite p5-6: Spans.}}"),
            Some(("Spans.".into(), 5, 6))
        );
        assert_eq!(parse_tag("{{cite p=0: Bad page}}"), None);
        assert_eq!(parse_tag("{{cite p=2: }}"), None);
        let mut tags = CiteTags::default();
        let mut events = tags.push("A {{cite no page}} B");
        events.extend(tags.finish());
        assert_eq!(texts(&events), "A  B", "malformed tags are dropped");
    }

    #[test]
    fn plain_braces_are_not_held_forever() {
        let mut tags = CiteTags::default();
        let mut events = tags.push("Set {x} and {{y}}");
        events.extend(tags.finish());
        assert_eq!(texts(&events), "Set {x} and {{y}}");
    }

    #[test]
    fn request_has_models_pdf_and_history() {
        let settings = Settings {
            openrouter_models: vec!["google/a".into(), "openai/b".into()],
            ..Settings::default()
        };
        let history = vec![
            Turn {
                role: Role::User,
                content: "q1".into(),
                context: None,
            },
            Turn {
                role: Role::Assistant,
                content: "a1".into(),
                context: None,
            },
        ];
        let question = Turn {
            role: Role::User,
            content: "q2".into(),
            context: None,
        };
        let body = request_body(
            &settings,
            b"%PDF-1.4",
            "paper.pdf",
            history,
            question,
            Some(3),
        );
        assert_eq!(body["models"], json!(["google/a", "openai/b"]));
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["role"], "system");
        assert!(
            messages[0]["content"]
                .as_str()
                .unwrap()
                .contains("{{cite p=PAGE")
        );
        assert_eq!(messages[1]["content"][0]["type"], "file");
        assert!(
            messages[1]["content"][0]["file"]["file_data"]
                .as_str()
                .unwrap()
                .starts_with("data:application/pdf;base64,")
        );
        assert_eq!(
            messages[3]["content"].as_array().unwrap().len(),
            1,
            "the PDF is sent once"
        );
    }
}

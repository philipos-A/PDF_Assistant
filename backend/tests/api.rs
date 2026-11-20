//! End-to-end API tests. Claude is replaced by a local mock server that
//! records each request and replies with a canned SSE stream, so these tests
//! exercise the real HTTP client and stream parser without calling Anthropic.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use http_body_util::BodyExt;
use pdf_assistant_backend::config::Settings;
use pdf_assistant_backend::{AppState, app};
use serde_json::{Value, json};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers

/// A minimal valid PDF with `pages` pages of selectable text.
fn make_pdf(pages: usize) -> Vec<u8> {
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {pages} >>",
            (0..pages)
                .map(|i| format!("{} 0 R", 3 + i * 2))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    ];
    let font = 3 + pages * 2;
    for i in 0..pages {
        let text = format!("BT /F1 18 Tf 72 700 Td (Page {}) Tj ET", i + 1);
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R /Resources << /Font << /F1 {font} 0 R >> >> >>",
            4 + i * 2
        ));
        objs.push(format!(
            "<< /Length {} >>\nstream\n{text}\nendstream",
            text.len()
        ));
    }
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());

    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (n, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", n + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
    for off in offsets {
        out.extend(format!("{off:010} 00000 n \n").bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .bytes(),
    );
    out
}

fn multipart(filename: &str, content_type: &str, data: &[u8]) -> Request<Body> {
    let boundary = "XBOUNDARYX";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(data);
    body.extend(format!("\r\n--{boundary}--\r\n").bytes());
    Request::post("/api/documents")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap()
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, HeaderMap, String) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

fn json_post(uri: &str, body: Value) -> Request<Body> {
    Request::post(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn parse_sse(text: &str) -> Vec<(String, Value)> {
    text.replace("\r\n", "\n")
        .split("\n\n")
        .filter_map(|block| {
            let mut event = None;
            let mut data = None;
            for line in block.lines() {
                if let Some(v) = line.strip_prefix("event:") {
                    event = Some(v.trim().to_string());
                } else if let Some(v) = line.strip_prefix("data:") {
                    data = Some(v.trim().to_string());
                }
            }
            Some((event?, serde_json::from_str(&data?).ok()?))
        })
        .collect()
}

fn sse(event: &str, data: Value) -> String {
    format!("event: {event}\ndata: {data}\n\n")
}

/// A canned successful Claude stream: thinking block, then text in two deltas.
fn ok_stream(stop_reason: &str) -> String {
    [
        sse("message_start", json!({"type": "message_start", "message": {"model": "claude-sonnet-5-5", "usage": {"input_tokens": 12, "cache_read_input_tokens": 3000, "cache_creation_input_tokens": 0}}})),
        sse("ping", json!({"type": "ping"})),
        sse("content_block_start", json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}})),
        sse("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
        sse("content_block_start", json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}})),
        sse("content_block_delta", json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Héllo "}})),
        sse("content_block_delta", json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "from p. 2"}})),
        sse("content_block_stop", json!({"type": "content_block_stop", "index": 1})),
        sse("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": stop_reason}, "usage": {"output_tokens": 7}})),
        sse("message_stop", json!({"type": "message_stop"})),
    ]
    .concat()
}

#[derive(Default)]
struct Recorded {
    body: Option<Value>,
    headers: Option<HeaderMap>,
    calls: usize,
}

/// Start a mock Anthropic API. `respond` returns (status, content-type, body) for each call
/// number. Every reply carries `retry-after: 0`.
async fn mock_claude(
    respond: impl Fn(usize) -> (StatusCode, &'static str, String) + Send + Sync + 'static,
) -> (String, Arc<Mutex<Recorded>>) {
    mock_claude_with_headers(move |n| {
        let (status, ctype, body) = respond(n);
        (status, ctype, body, vec![("retry-after", "0")])
    })
    .await
}

type MockReply = (
    StatusCode,
    &'static str,
    String,
    Vec<(&'static str, &'static str)>,
);

/// Like `mock_claude`, but `respond` also chooses the extra response headers.
async fn mock_claude_with_headers(
    respond: impl Fn(usize) -> MockReply + Send + Sync + 'static,
) -> (String, Arc<Mutex<Recorded>>) {
    let rec = Arc::new(Mutex::new(Recorded::default()));
    let rec2 = rec.clone();
    let respond = Arc::new(respond);
    let router = Router::new().route(
        "/v1/messages",
        post(move |headers: HeaderMap, body: String| {
            let rec = rec2.clone();
            let respond = respond.clone();
            async move {
                let n = {
                    let mut r = rec.lock().unwrap();
                    r.calls += 1;
                    r.body = serde_json::from_str(&body).ok();
                    r.headers = Some(headers);
                    r.calls
                };
                let (status, ctype, body, extra) = respond(n);
                let mut resp = (status, [("content-type", ctype)], body).into_response();
                for (name, value) in extra {
                    resp.headers_mut().insert(name, value.parse().unwrap());
                }
                resp
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, rec)
}

fn test_app(base_url: &str, api_key: &str) -> (Router, tempdir::Dir) {
    let dir = tempdir::Dir::new();
    let settings = Settings {
        anthropic_api_key: api_key.into(),
        anthropic_base_url: base_url.into(),
        storage_dir: dir.path.clone(),
        max_upload_mb: 1,
        ..Settings::default()
    };
    (app(AppState::new(settings).unwrap()), dir)
}

/// Tiny self-cleaning temp dir (avoids an extra dependency).
mod tempdir {
    use std::path::PathBuf;
    pub struct Dir {
        pub path: PathBuf,
    }
    impl Dir {
        pub fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "pdf-assistant-test-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

async fn upload(app: &Router, pages: usize) -> String {
    let (status, _, body) = send(
        app,
        multipart("paper.pdf", "application/pdf", &make_pdf(pages)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_str::<Value>(&body).unwrap()["doc_id"]
        .as_str()
        .unwrap()
        .to_string()
}

// ---------------------------------------------------------------------------
// Health and documents

#[tokio::test]
async fn health_reports_configuration() {
    let (app, _dir) = test_app("http://unused", "test-key");
    let (status, _, body) = send(
        &app,
        Request::get("/api/health").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["status"], "ok");
    assert_eq!(v["model"], "claude-sonnet-5-5");
    assert_eq!(v["api_key_configured"], true);
}

#[tokio::test]
async fn upload_and_fetch_pdf() {
    let (app, dir) = test_app("http://unused", "test-key");
    let (status, _, body) = send(
        &app,
        multipart("../../evil/paper.pdf", "application/pdf", &make_pdf(3)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["page_count"], 3);
    assert_eq!(v["filename"], "paper.pdf", "directory parts are stripped");
    let id = v["doc_id"].as_str().unwrap();
    assert!(dir.path.join(format!("{id}.pdf")).exists());

    let (status, _, meta) = send(
        &app,
        Request::get(format!("/api/documents/{id}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<Value>(&meta).unwrap()["page_count"],
        3
    );

    let (status, headers, file) = send(
        &app,
        Request::get(format!("/api/documents/{id}/file"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/pdf");
    assert!(file.starts_with("%PDF-"));
}

#[tokio::test]
async fn upload_validation() {
    let (app, _dir) = test_app("http://unused", "test-key");

    let (status, _, _) = send(&app, multipart("notes.txt", "text/plain", b"hello")).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "non-PDF type");

    let (status, _, _) = send(
        &app,
        multipart("fake.pdf", "application/pdf", b"not really a pdf"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "missing %PDF- header"
    );

    let (status, _, body) = send(
        &app,
        multipart("broken.pdf", "application/pdf", b"%PDF-1.7\ngarbage"),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "unparseable PDF");
    assert!(body.contains("detail"));

    let mut big = b"%PDF-1.7\n".to_vec();
    big.resize(1024 * 1024 + 10, b'0'); // max_upload_mb = 1 in tests
    let (status, _, body) = send(&app, multipart("big.pdf", "application/pdf", &big)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
}

#[tokio::test]
async fn unknown_or_malicious_doc_ids_are_404() {
    let (app, _dir) = test_app("http://unused", "test-key");
    for uri in [
        "/api/documents/00000000000000000000000000000000",
        "/api/documents/..%2F..%2Fetc%2Fpasswd/file",
    ] {
        let (status, _, _) = send(&app, Request::get(uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
}

// ---------------------------------------------------------------------------
// Chat

#[tokio::test]
async fn chat_streams_answer_and_builds_cached_request() {
    let (url, rec) =
        mock_claude(|_| (StatusCode::OK, "text/event-stream", ok_stream("end_turn"))).await;
    let (app, _dir) = test_app(&url, "test-key");
    let doc_id = upload(&app, 4).await;

    let req = json_post(
        "/api/chat",
        json!({
            "doc_id": doc_id,
            "question": "  What does this mean?  ",
            "current_page": 2,
            "visible_pages": [3, 2],
            "selected_text": "attention is all you need",
            "selection_page": 3,
            "history": [
                {"role": "user", "content": "Summarize p. 1", "current_page": 1, "visible_pages": [1]},
                {"role": "assistant", "content": "It introduces...", "current_page": 9},
            ],
        }),
    );
    let (status, headers, body) = send(&app, req).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );

    // Browser-facing events.
    let events = parse_sse(&body);
    let names: Vec<&str> = events.iter().map(|(e, _)| e.as_str()).collect();
    assert_eq!(names, ["status", "status", "delta", "delta", "done"]);
    assert_eq!(events[0].1["state"], "thinking");
    let text: String = events
        .iter()
        .filter(|(e, _)| e == "delta")
        .map(|(_, d)| d["text"].as_str().unwrap())
        .collect();
    assert_eq!(text, "Héllo from p. 2");
    let done = &events.last().unwrap().1;
    assert_eq!(done["stop_reason"], "end_turn");
    assert_eq!(done["usage"]["cache_read_input_tokens"], 3000);
    assert_eq!(done["usage"]["output_tokens"], 7);

    // What was sent to Claude.
    let rec = rec.lock().unwrap();
    let headers = rec.headers.as_ref().unwrap();
    assert_eq!(headers["x-api-key"], "test-key");
    assert_eq!(headers["anthropic-version"], "2026-10-21");
    assert_eq!(headers["anthropic-beta"], "server-side-fallback-2026-07-01");

    let sent = rec.body.as_ref().unwrap();
    assert_eq!(sent["model"], "claude-sonnet-5-5");
    assert_eq!(sent["stream"], true);
    assert_eq!(sent["fallbacks"], "default");
    assert_eq!(sent["output_config"]["effort"], "medium");
    assert_eq!(sent["cache_control"]["type"], "ephemeral");
    assert!(
        sent["system"]
            .as_str()
            .unwrap()
            .contains("reading companion")
    );
    assert!(sent.get("temperature").is_none());

    let msgs = sent["messages"].as_array().unwrap();
    let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["user", "assistant", "user"]);

    // The PDF is the first block of the first turn, cached, and nowhere else.
    let doc = &msgs[0]["content"][0];
    assert_eq!(doc["type"], "document");
    assert_eq!(doc["source"]["media_type"], "application/pdf");
    assert_eq!(doc["cache_control"]["type"], "ephemeral");
    assert!(!doc["source"]["data"].as_str().unwrap().contains('\n'));
    assert_eq!(msgs[2]["content"].as_array().unwrap().len(), 1);

    // Reader context on the new question, and on the earlier user turn.
    let last = msgs[2]["content"][0]["text"].as_str().unwrap();
    assert!(last.contains("Current page: 2 of 4"));
    assert!(last.contains("Pages visible on screen: 2, 3"));
    assert!(last.contains(
        "Text the reader has selected on page 3:\n<selection>\nattention is all you need\n</selection>"
    ));
    assert!(last.ends_with("\n\nWhat does this mean?"));
    assert!(
        msgs[0]["content"][1]["text"]
            .as_str()
            .unwrap()
            .contains("Current page: 1 of 4")
    );
    assert_eq!(
        msgs[1]["content"], "It introduces...",
        "assistant turns carry no context"
    );
}

#[tokio::test]
async fn chat_refusal_becomes_error_event() {
    let (url, _) =
        mock_claude(|_| (StatusCode::OK, "text/event-stream", ok_stream("refusal"))).await;
    let (app, _dir) = test_app(&url, "test-key");
    let doc_id = upload(&app, 1).await;
    let (_, _, body) = send(
        &app,
        json_post(
            "/api/chat",
            json!({"doc_id": doc_id, "question": "hi", "current_page": 1}),
        ),
    )
    .await;
    let (event, data) = parse_sse(&body).pop().unwrap();
    assert_eq!(event, "error");
    assert!(data["message"].as_str().unwrap().contains("declined"));
    // Lets the frontend discard the partial answer that streamed before the refusal.
    assert_eq!(data["refused"], true);
}

#[tokio::test]
async fn chat_reports_fallback_and_mid_stream_errors() {
    let stream = [
        sse("message_start", json!({"type": "message_start", "message": {"model": "claude-sonnet-5", "usage": {}}})),
        sse("content_block_start", json!({"type": "content_block_start", "index": 0, "content_block": {"type": "fallback"}})),
        sse("content_block_delta", json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Partial"}})),
        sse("error", json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}})),
    ]
    .concat();
    let (url, rec) =
        mock_claude(move |_| (StatusCode::OK, "text/event-stream", stream.clone())).await;
    let (app, _dir) = test_app(&url, "test-key");
    let doc_id = upload(&app, 1).await;
    let (_, _, body) = send(
        &app,
        json_post(
            "/api/chat",
            json!({"doc_id": doc_id, "question": "hi", "current_page": 1}),
        ),
    )
    .await;
    let events = parse_sse(&body);
    assert_eq!(
        events[0],
        ("status".to_string(), json!({"state": "fallback"}))
    );
    assert_eq!(events[1].0, "delta");
    assert_eq!(events[2].0, "error");
    assert!(
        events[2].1["message"]
            .as_str()
            .unwrap()
            .contains("overloaded")
    );
    // Output had already streamed, so the error is not retried.
    assert_eq!(rec.lock().unwrap().calls, 1);
}

#[tokio::test]
async fn chat_done_names_fallback_model_after_mid_answer_switch() {
    let stream = [
        sse("message_start", json!({"type": "message_start", "message": {"model": "claude-sonnet-5-5", "usage": {}}})),
        sse("content_block_start", json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})),
        sse("content_block_delta", json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Partial"}})),
        sse("content_block_start", json!({"type": "content_block_start", "index": 1, "content_block": {"type": "fallback", "from": {"model": "claude-sonnet-5-5"}, "to": {"model": "claude-sonnet-5"}}})),
        sse("content_block_start", json!({"type": "content_block_start", "index": 2, "content_block": {"type": "text", "text": ""}})),
        sse("content_block_delta", json!({"type": "content_block_delta", "index": 2, "delta": {"type": "text_delta", "text": "Answer"}})),
        sse("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 3}})),
        sse("message_stop", json!({"type": "message_stop"})),
    ]
    .concat();
    let (url, _) =
        mock_claude(move |_| (StatusCode::OK, "text/event-stream", stream.clone())).await;
    let (app, _dir) = test_app(&url, "test-key");
    let doc_id = upload(&app, 1).await;
    let (_, _, body) = send(
        &app,
        json_post(
            "/api/chat",
            json!({"doc_id": doc_id, "question": "hi", "current_page": 1}),
        ),
    )
    .await;
    let events = parse_sse(&body);
    let done = events
        .iter()
        .find(|(e, _)| e == "done")
        .expect("done event");
    assert_eq!(done.1["model"], "claude-sonnet-5");
}

#[tokio::test]
async fn chat_retries_transient_errors_then_succeeds() {
    let (url, rec) = mock_claude(|n| {
        if n == 1 {
            (
                StatusCode::from_u16(529).unwrap(),
                "application/json",
                r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#
                    .into(),
            )
        } else {
            (StatusCode::OK, "text/event-stream", ok_stream("end_turn"))
        }
    })
    .await;
    let (app, _dir) = test_app(&url, "test-key");
    let doc_id = upload(&app, 1).await;
    let (_, _, body) = send(
        &app,
        json_post(
            "/api/chat",
            json!({"doc_id": doc_id, "question": "hi", "current_page": 1}),
        ),
    )
    .await;
    assert_eq!(parse_sse(&body).last().unwrap().0, "done");
    assert_eq!(rec.lock().unwrap().calls, 2);
}

async fn ask(app: &Router, doc_id: &str) -> Vec<(String, Value)> {
    let (_, _, body) = send(
        app,
        json_post(
            "/api/chat",
            json!({"doc_id": doc_id, "question": "hi", "current_page": 1}),
        ),
    )
    .await;
    parse_sse(&body)
}

#[tokio::test]
async fn chat_retries_stream_error_before_any_output() {
    let (url, rec) = mock_claude(|n| {
        let body = if n == 1 {
            [
                sse("message_start", json!({"type": "message_start", "message": {"model": "claude-sonnet-5-5", "usage": {}}})),
                sse("error", json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}})),
            ]
            .concat()
        } else {
            ok_stream("end_turn")
        };
        (StatusCode::OK, "text/event-stream", body)
    })
    .await;
    let (app, _dir) = test_app(&url, "test-key");
    let doc_id = upload(&app, 1).await;
    let events = ask(&app, &doc_id).await;
    assert!(events.iter().all(|(e, _)| e != "error"), "{events:?}");
    assert!(events.iter().any(|(e, _)| e == "done"));
    assert_eq!(rec.lock().unwrap().calls, 2);
}

#[tokio::test]
async fn chat_fails_fast_when_retry_after_is_too_long() {
    let (url, rec) = mock_claude_with_headers(|_| {
        (
            StatusCode::TOO_MANY_REQUESTS,
            "application/json",
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#.into(),
            vec![("retry-after", "30")],
        )
    })
    .await;
    let (app, _dir) = test_app(&url, "test-key");
    let doc_id = upload(&app, 1).await;
    let events = ask(&app, &doc_id).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, "error");
    assert!(
        events[0].1["message"]
            .as_str()
            .unwrap()
            .contains("Rate limited")
    );
    assert_eq!(rec.lock().unwrap().calls, 1);
}

#[tokio::test]
async fn chat_prefers_retry_after_ms() {
    let (url, rec) = mock_claude_with_headers(|n| {
        if n == 1 {
            (
                StatusCode::from_u16(529).unwrap(),
                "application/json",
                r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#
                    .into(),
                // retry-after alone would exceed the cap; retry-after-ms wins.
                vec![("retry-after-ms", "5"), ("retry-after", "30")],
            )
        } else {
            (
                StatusCode::OK,
                "text/event-stream",
                ok_stream("end_turn"),
                vec![],
            )
        }
    })
    .await;
    let (app, _dir) = test_app(&url, "test-key");
    let doc_id = upload(&app, 1).await;
    let events = ask(&app, &doc_id).await;
    assert!(events.iter().any(|(e, _)| e == "done"), "{events:?}");
    assert_eq!(rec.lock().unwrap().calls, 2);
}

#[tokio::test]
async fn chat_respects_x_should_retry_false() {
    let (url, rec) = mock_claude_with_headers(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "application/json",
            r#"{"type":"error","error":{"type":"api_error","message":"boom"}}"#.into(),
            vec![("retry-after", "0"), ("x-should-retry", "false")],
        )
    })
    .await;
    let (app, _dir) = test_app(&url, "test-key");
    let doc_id = upload(&app, 1).await;
    let events = ask(&app, &doc_id).await;
    assert_eq!(events[0].0, "error");
    assert_eq!(rec.lock().unwrap().calls, 1);
}

#[tokio::test]
async fn chat_maps_auth_errors_without_retrying() {
    let (url, rec) = mock_claude(|_| {
        (StatusCode::UNAUTHORIZED, "application/json", r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#.into())
    })
    .await;
    let (app, _dir) = test_app(&url, "bad-key");
    let doc_id = upload(&app, 1).await;
    let (_, _, body) = send(
        &app,
        json_post(
            "/api/chat",
            json!({"doc_id": doc_id, "question": "hi", "current_page": 1}),
        ),
    )
    .await;
    let (event, data) = parse_sse(&body).pop().unwrap();
    assert_eq!(event, "error");
    assert!(
        data["message"]
            .as_str()
            .unwrap()
            .contains("API key was rejected")
    );
    assert_eq!(rec.lock().unwrap().calls, 1);
}

#[tokio::test]
async fn chat_request_validation() {
    let (app, _dir) = test_app("http://unused", "test-key");
    let unknown = json!({"doc_id": "f".repeat(32), "question": "hi", "current_page": 1});
    assert_eq!(
        send(&app, json_post("/api/chat", unknown)).await.0,
        StatusCode::NOT_FOUND
    );

    let empty = json!({"doc_id": "f".repeat(32), "question": "   ", "current_page": 1});
    assert_eq!(
        send(&app, json_post("/api/chat", empty)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let (status, _, body) = send(&app, json_post("/api/chat", json!({"question": "hi"}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("detail"), "{body}");
}

#[tokio::test]
async fn chat_without_api_key_is_503() {
    let (app, _dir) = test_app("http://unused", "");
    let (status, _, body) = send(
        &app,
        json_post(
            "/api/chat",
            json!({"doc_id": "f".repeat(32), "question": "hi", "current_page": 1}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body.contains("ANTHROPIC_API_KEY"));
    let (_, _, health) = send(
        &app,
        Request::get("/api/health").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(
        serde_json::from_str::<Value>(&health).unwrap()["api_key_configured"],
        false
    );
}

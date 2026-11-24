//! End-to-end API tests. Claude is replaced by a local mock server that
//! records each request and replies with a canned SSE stream, so these tests
//! exercise the real HTTP client and stream parser without calling Anthropic.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, post};
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
    /// The latest Messages request.
    body: Option<Value>,
    headers: Option<HeaderMap>,
    /// Messages requests so far.
    calls: usize,
    /// Every Messages request body, oldest first.
    bodies: Vec<Value>,
    /// Files API uploads so far.
    uploads: usize,
    /// When set, uploads fail with a 500.
    uploads_fail: bool,
    /// File ids deleted through the Files API.
    deleted: Vec<String>,
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
                    if let Some(b) = r.body.clone() {
                        r.bodies.push(b);
                    }
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
    let rec3 = rec.clone();
    let rec4 = rec.clone();
    let router = router
        .route(
            "/v1/files",
            post(move |headers: HeaderMap, body: axum::body::Bytes| {
                let rec = rec3.clone();
                async move {
                    let mut r = rec.lock().unwrap();
                    assert!(
                        headers["content-type"]
                            .to_str()
                            .unwrap()
                            .starts_with("multipart/form-data")
                    );
                    assert!(
                        headers.get("anthropic-beta").is_none(),
                        "Files API needs no beta"
                    );
                    assert!(body.windows(5).any(|w| w == b"%PDF-"));
                    if r.uploads_fail {
                        return (StatusCode::INTERNAL_SERVER_ERROR, "{}".to_string())
                            .into_response();
                    }
                    r.uploads += 1;
                    let id = format!("file_test{}", r.uploads);
                    axum::Json(json!({"id": id, "type": "file"})).into_response()
                }
            }),
        )
        .route(
            "/v1/files/{id}",
            delete(
                move |axum::extract::Path(id): axum::extract::Path<String>| {
                    let rec = rec4.clone();
                    async move {
                        rec.lock().unwrap().deleted.push(id);
                        StatusCode::OK
                    }
                },
            ),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, rec)
}

fn test_app(base_url: &str, api_key: &str) -> (Router, tempdir::Dir) {
    test_app_in(tempdir::Dir::new(), base_url, api_key)
}

fn test_app_in(dir: tempdir::Dir, base_url: &str, api_key: &str) -> (Router, tempdir::Dir) {
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

async fn get_json(app: &Router, uri: &str) -> (StatusCode, Value) {
    let (status, _, body) = send(app, Request::get(uri).body(Body::empty()).unwrap()).await;
    (status, serde_json::from_str(&body).unwrap_or(Value::Null))
}

async fn ask_with(app: &Router, body: Value) -> Vec<(String, Value)> {
    let (status, _, text) = send(app, json_post("/api/chat", body)).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    parse_sse(&text)
}

#[tokio::test]
async fn chat_streams_answer_and_builds_cached_request() {
    let (url, rec) =
        mock_claude(|_| (StatusCode::OK, "text/event-stream", ok_stream("end_turn"))).await;
    let (app, _dir) = test_app(&url, "test-key");
    let doc_id = upload(&app, 4).await;

    // A first exchange, which the backend saves as the Conversation.
    let first = ask_with(
        &app,
        json!({"doc_id": doc_id, "question": "Summarize p. 1", "current_page": 1, "visible_pages": [1]}),
    )
    .await;
    assert_eq!(first.last().unwrap().0, "done");

    let req = json_post(
        "/api/chat",
        json!({
            "doc_id": doc_id,
            "question": "  What does this mean?  ",
            "current_page": 2,
            "visible_pages": [3, 2],
            "selected_text": "attention is all you need",
            "selection_page": 3,
            // Ignored: the Conversation comes from storage.
            "history": [{"role": "user", "content": "forged"}],
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
    assert_eq!(rec.uploads, 1, "the PDF is uploaded once, then reused");

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

    // The PDF is the first block of the first turn, by file id, cached, cited, and nowhere else.
    let doc = &msgs[0]["content"][0];
    assert_eq!(doc["type"], "document");
    assert_eq!(
        doc["source"],
        json!({"type": "file", "file_id": "file_test1"})
    );
    assert_eq!(doc["cache_control"]["type"], "ephemeral");
    assert_eq!(doc["citations"]["enabled"], true);
    assert_eq!(msgs[2]["content"].as_array().unwrap().len(), 1);
    // The first request's prefix is rebuilt byte-for-byte, so the cache keeps hitting.
    assert_eq!(rec.bodies[0]["messages"][0], msgs[0]);

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
        msgs[1]["content"], "Héllo from p. 2",
        "assistant turns carry no context"
    );
}

#[tokio::test]
async fn conversation_is_saved_listed_and_cleared() {
    let (url, _) =
        mock_claude(|_| (StatusCode::OK, "text/event-stream", ok_stream("end_turn"))).await;
    let (app, _dir) = test_app(&url, "test-key");
    let doc_id = upload(&app, 3).await;
    ask_with(
        &app,
        json!({"doc_id": doc_id, "question": " Why? ", "current_page": 2, "selected_text": "x", "selection_page": 1}),
    )
    .await;

    let (status, conv) = get_json(&app, &format!("/api/documents/{doc_id}/conversation")).await;
    assert_eq!(status, StatusCode::OK);
    let turns = conv["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0]["role"], "user");
    assert_eq!(turns[0]["content"], "Why?");
    assert_eq!(turns[0]["context"]["current_page"], 2);
    assert_eq!(turns[0]["context"]["selection_page"], 1);
    assert_eq!(turns[1]["role"], "assistant");
    assert_eq!(turns[1]["content"], "Héllo from p. 2");
    assert_eq!(turns[1]["model"], "claude-sonnet-5-5");
    assert_eq!(turns[1]["used_fallback"], false);

    let (_, list) = get_json(&app, "/api/documents").await;
    assert_eq!(list["documents"][0]["doc_id"], doc_id.as_str());
    assert_eq!(list["documents"][0]["question_count"], 1);
    assert!(
        list["documents"][0].get("file_id").is_none(),
        "file ids stay server-side"
    );

    let (status, _, _) = send(
        &app,
        Request::delete(format!("/api/documents/{doc_id}/conversation"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, conv) = get_json(&app, &format!("/api/documents/{doc_id}/conversation")).await;
    assert_eq!(conv["turns"], json!([]));
}

#[tokio::test]
async fn failed_answers_are_not_saved() {
    let (url, _) =
        mock_claude(|_| (StatusCode::OK, "text/event-stream", ok_stream("refusal"))).await;
    let (app, _dir) = test_app(&url, "test-key");
    let doc_id = upload(&app, 1).await;
    ask(&app, &doc_id).await;
    let (_, conv) = get_json(&app, &format!("/api/documents/{doc_id}/conversation")).await;
    assert_eq!(conv["turns"], json!([]));
}

#[tokio::test]
async fn library_lists_most_recent_first_and_deletes() {
    let (url, rec) =
        mock_claude(|_| (StatusCode::OK, "text/event-stream", ok_stream("end_turn"))).await;
    let (app, dir) = test_app(&url, "test-key");
    let older = upload(&app, 1).await;
    let newer = upload(&app, 2).await;
    // Asking about the older Document makes it the most recently active.
    ask(&app, &older).await;

    let (_, list) = get_json(&app, "/api/documents").await;
    let ids: Vec<&str> = list["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["doc_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [older.as_str(), newer.as_str()]);

    let (status, _, _) = send(
        &app,
        Request::delete(format!("/api/documents/{older}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!dir.path.join(format!("{older}.pdf")).exists());
    assert_eq!(
        get_json(&app, &format!("/api/documents/{older}")).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, list) = get_json(&app, "/api/documents").await;
    assert_eq!(list["documents"].as_array().unwrap().len(), 1);

    // The Files API copy is deleted in the background.
    for _ in 0..50 {
        if !rec.lock().unwrap().deleted.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(rec.lock().unwrap().deleted, ["file_test1"]);

    let (status, _, _) = send(
        &app,
        Request::delete(format!("/api/documents/{older}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn missing_uploaded_file_is_uploaded_again() {
    let (url, rec) = mock_claude(|n| {
        if n == 1 {
            (
                StatusCode::NOT_FOUND,
                "application/json",
                r#"{"type":"error","error":{"type":"not_found_error","message":"File `file_test1` not found."}}"#.into(),
            )
        } else {
            (StatusCode::OK, "text/event-stream", ok_stream("end_turn"))
        }
    })
    .await;
    let (app, _dir) = test_app(&url, "test-key");
    let doc_id = upload(&app, 1).await;
    let events = ask(&app, &doc_id).await;
    assert_eq!(events.last().unwrap().0, "done", "{events:?}");
    let rec = rec.lock().unwrap();
    assert_eq!((rec.uploads, rec.calls), (2, 2));
    assert_eq!(
        rec.body.as_ref().unwrap()["messages"][0]["content"][0]["source"]["file_id"],
        "file_test2"
    );
}

#[tokio::test]
async fn failed_upload_falls_back_to_inline_pdf() {
    let (url, rec) =
        mock_claude(|_| (StatusCode::OK, "text/event-stream", ok_stream("end_turn"))).await;
    rec.lock().unwrap().uploads_fail = true;
    let (app, _dir) = test_app(&url, "test-key");
    let doc_id = upload(&app, 1).await;
    let events = ask(&app, &doc_id).await;
    assert_eq!(events.last().unwrap().0, "done");
    let rec = rec.lock().unwrap();
    let source = &rec.body.as_ref().unwrap()["messages"][0]["content"][0]["source"];
    assert_eq!(source["type"], "base64");
    assert!(!source["data"].as_str().unwrap().contains('\n'));
}

#[tokio::test]
async fn citations_stream_as_numbered_markers_and_are_saved() {
    let cite = |text: &str, start: u64| {
        sse(
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "citations_delta", "citation": {
                "type": "page_location", "cited_text": text, "document_index": 0, "document_title": null,
                "start_page_number": start, "end_page_number": start + 1,
            }}}),
        )
    };
    let stream = [
        sse("message_start", json!({"type": "message_start", "message": {"model": "claude-sonnet-5-5", "usage": {}}})),
        sse("content_block_start", json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})),
        sse("content_block_delta", json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Uses softmax"}})),
        cite("We apply a softmax.", 2),
        cite("We apply a softmax.", 2),
        sse("content_block_start", json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}})),
        sse("content_block_delta", json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": " twice"}})),
        cite("Again, softmax.", 3),
        cite("We apply a softmax.", 2),
        sse("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 3}})),
        sse("message_stop", json!({"type": "message_stop"})),
    ]
    .concat();
    let (url, rec) =
        mock_claude(move |_| (StatusCode::OK, "text/event-stream", stream.clone())).await;
    let (app, _dir) = test_app(&url, "test-key");
    let doc_id = upload(&app, 3).await;
    let events = ask(&app, &doc_id).await;

    let text: String = events
        .iter()
        .filter(|(e, _)| e == "delta")
        .map(|(_, d)| d["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        text,
        "Uses softmax[1](#cite-1) twice[2](#cite-2)[1](#cite-1)"
    );
    let citations: Vec<&Value> = events
        .iter()
        .filter(|(e, _)| e == "citation")
        .map(|(_, d)| d)
        .collect();
    assert_eq!(
        citations,
        [
            &json!({"n": 1, "cited_text": "We apply a softmax.", "start_page": 2, "end_page": 2}),
            &json!({"n": 2, "cited_text": "Again, softmax.", "start_page": 3, "end_page": 3}),
        ]
    );

    let (_, conv) = get_json(&app, &format!("/api/documents/{doc_id}/conversation")).await;
    assert_eq!(conv["turns"][1]["content"], text.as_str());
    assert_eq!(conv["turns"][1]["citations"].as_array().unwrap().len(), 2);

    // The next question sends the earlier Answer back without its markers.
    ask(&app, &doc_id).await;
    let rec = rec.lock().unwrap();
    assert_eq!(
        rec.body.as_ref().unwrap()["messages"][1]["content"],
        "Uses softmax twice"
    );
}

#[tokio::test]
async fn documents_from_older_versions_are_imported() {
    let dir = tempdir::Dir::new();
    let id = "0123456789abcdef0123456789abcdef";
    std::fs::write(dir.path.join(format!("{id}.pdf")), make_pdf(2)).unwrap();
    std::fs::write(
        dir.path.join(format!("{id}.json")),
        json!({"doc_id": id, "filename": "old.pdf", "page_count": 2, "size_bytes": 10, "uploaded_at": "2026-01-01T00:00:00+00:00"}).to_string(),
    )
    .unwrap();
    let (app, dir) = test_app_in(dir, "http://unused", "test-key");
    let (status, meta) = get_json(&app, &format!("/api/documents/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(meta["filename"], "old.pdf");
    assert!(!dir.path.join(format!("{id}.json")).exists());
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

    let long = json!({"doc_id": "f".repeat(32), "question": "hi", "current_page": 1, "selected_text": "x".repeat(20_001)});
    assert_eq!(
        send(&app, json_post("/api/chat", long)).await.0,
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

// ---------------------------------------------------------------------------
// OpenRouter fallback

/// A canned OpenRouter stream: a keep-alive comment, text with a citation tag split
/// across chunks, a finish reason and usage, then `[DONE]`.
fn openrouter_ok_stream() -> String {
    let chunk = |delta: Value, finish: Value| {
        format!(
            "data: {}\n\n",
            json!({"model": "google/gemini-test", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
        )
    };
    [
        ": OPENROUTER PROCESSING\n\n".to_string(),
        chunk(json!({"content": "{{cite p=1: Page "}), Value::Null),
        chunk(json!({"content": "1}}Gemini says hi "}), Value::Null),
        chunk(json!({"content": "(p. 1)."}), json!("stop")),
        format!(
            "data: {}\n\n",
            json!({"model": "google/gemini-test", "choices": [], "usage": {"prompt_tokens": 50, "completion_tokens": 9}})
        ),
        "data: [DONE]\n\n".to_string(),
    ]
    .concat()
}

/// Start a mock OpenRouter API; returns its base URL and every request body.
async fn mock_openrouter(
    respond: impl Fn(usize) -> (StatusCode, &'static str, String) + Send + Sync + 'static,
) -> (String, Arc<Mutex<Vec<(HeaderMap, Value)>>>) {
    let rec: Arc<Mutex<Vec<(HeaderMap, Value)>>> = Arc::default();
    let rec2 = rec.clone();
    let respond = Arc::new(respond);
    let router = Router::new().route(
        "/api/v1/chat/completions",
        post(move |headers: HeaderMap, body: String| {
            let rec = rec2.clone();
            let respond = respond.clone();
            async move {
                let n = {
                    let mut r = rec.lock().unwrap();
                    r.push((headers, serde_json::from_str(&body).unwrap()));
                    r.len()
                };
                let (status, ctype, body) = respond(n);
                (status, [("content-type", ctype)], body).into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api/v1", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, rec)
}

fn test_app_with_fallback(
    claude_url: &str,
    claude_key: &str,
    openrouter_url: &str,
) -> (Router, tempdir::Dir) {
    let dir = tempdir::Dir::new();
    let settings = Settings {
        anthropic_api_key: claude_key.into(),
        anthropic_base_url: claude_url.into(),
        openrouter_api_key: "sk-or-test".into(),
        openrouter_base_url: openrouter_url.into(),
        openrouter_models: vec!["google/gemini-test".into(), "openai/gpt-test".into()],
        storage_dir: dir.path.clone(),
        max_upload_mb: 1,
        ..Settings::default()
    };
    (app(AppState::new(settings).unwrap()), dir)
}

fn rate_limited() -> MockReply {
    (
        StatusCode::TOO_MANY_REQUESTS,
        "application/json",
        r#"{"type":"error","error":{"type":"rate_limit_error","message":"usage limit reached"}}"#
            .into(),
        vec![("retry-after", "3600")],
    )
}

#[tokio::test]
async fn rate_limited_claude_falls_back_to_openrouter() {
    let (claude, claude_rec) = mock_claude_with_headers(|_| rate_limited()).await;
    let (router, router_rec) =
        mock_openrouter(|_| (StatusCode::OK, "text/event-stream", openrouter_ok_stream())).await;
    let (app, _dir) = test_app_with_fallback(&claude, "test-key", &router);
    let doc_id = upload(&app, 2).await;

    let events = ask(&app, &doc_id).await;
    let names: Vec<&str> = events.iter().map(|(e, _)| e.as_str()).collect();
    assert!(!names.contains(&"error"), "{events:?}");
    assert_eq!(events[0], ("status".into(), json!({"state": "fallback"})));
    let citation = events.iter().find(|(e, _)| e == "citation").unwrap();
    assert_eq!(
        citation.1,
        json!({"n": 1, "cited_text": "Page 1", "start_page": 1, "end_page": 1})
    );
    let text: String = events
        .iter()
        .filter(|(e, _)| e == "delta")
        .map(|(_, d)| d["text"].as_str().unwrap())
        .collect();
    assert_eq!(text, "[1](#cite-1)Gemini says hi (p. 1).");
    let done = &events.last().unwrap().1;
    assert_eq!(done["model"], "google/gemini-test");
    assert_eq!(done["usage"]["output_tokens"], 9);

    // One Claude attempt (the hour-long retry-after fails fast), one OpenRouter request.
    assert_eq!(claude_rec.lock().unwrap().calls, 1);
    let requests = router_rec.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    let (headers, body) = &requests[0];
    assert_eq!(headers["authorization"], "Bearer sk-or-test");
    assert_eq!(
        body["models"],
        json!(["google/gemini-test", "openai/gpt-test"])
    );
    assert_eq!(body["stream"], true);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][1]["content"][0]["type"], "file");
    assert!(
        body["messages"][1]["content"][1]["text"]
            .as_str()
            .unwrap()
            .contains("Current page: 1 of 2")
    );

    // Saved like any Answer, marked as a fallback with the model that wrote it.
    let (_, conv) = get_json(&app, &format!("/api/documents/{doc_id}/conversation")).await;
    let answer = &conv["turns"][1];
    assert_eq!(answer["model"], "google/gemini-test");
    assert_eq!(answer["used_fallback"], true);
    assert_eq!(answer["citations"][0]["cited_text"], "Page 1");
}

#[tokio::test]
async fn out_of_credit_claude_falls_back_to_openrouter() {
    let (claude, _) = mock_claude(|_| {
        (StatusCode::BAD_REQUEST, "application/json", r#"{"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the Anthropic API."}}"#.into())
    })
    .await;
    let (router, router_rec) =
        mock_openrouter(|_| (StatusCode::OK, "text/event-stream", openrouter_ok_stream())).await;
    let (app, _dir) = test_app_with_fallback(&claude, "test-key", &router);
    let doc_id = upload(&app, 1).await;
    let events = ask(&app, &doc_id).await;
    assert_eq!(events.last().unwrap().0, "done");
    assert_eq!(router_rec.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn without_a_claude_key_openrouter_answers_directly() {
    let (claude, claude_rec) = mock_claude(|_| unreachable!("Claude has no key")).await;
    let (router, _) =
        mock_openrouter(|_| (StatusCode::OK, "text/event-stream", openrouter_ok_stream())).await;
    let (app, _dir) = test_app_with_fallback(&claude, "", &router);
    let doc_id = upload(&app, 1).await;
    let events = ask(&app, &doc_id).await;
    assert_eq!(events.last().unwrap().0, "done");
    let calls = {
        let rec = claude_rec.lock().unwrap();
        (rec.calls, rec.uploads)
    };
    assert_eq!(calls, (0, 0));

    let (_, health) = get_json(&app, "/api/health").await;
    assert_eq!(health["api_key_configured"], false);
    assert_eq!(health["fallback_configured"], true);
}

#[tokio::test]
async fn other_claude_errors_do_not_fall_back() {
    let (claude, _) = mock_claude(|_| {
        (StatusCode::UNAUTHORIZED, "application/json", r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#.into())
    })
    .await;
    let (router, router_rec) =
        mock_openrouter(|_| (StatusCode::OK, "text/event-stream", openrouter_ok_stream())).await;
    let (app, _dir) = test_app_with_fallback(&claude, "bad-key", &router);
    let doc_id = upload(&app, 1).await;
    let events = ask(&app, &doc_id).await;
    assert_eq!(events.last().unwrap().0, "error");
    assert!(router_rec.lock().unwrap().is_empty());
}

#[tokio::test]
async fn both_unavailable_reports_both_reasons() {
    let (claude, _) = mock_claude_with_headers(|_| rate_limited()).await;
    let (router, router_rec) = mock_openrouter(|_| {
        (
            StatusCode::PAYMENT_REQUIRED,
            "application/json",
            r#"{"error":{"code":402,"message":"Insufficient credits"}}"#.into(),
        )
    })
    .await;
    let (app, _dir) = test_app_with_fallback(&claude, "test-key", &router);
    let doc_id = upload(&app, 1).await;
    let events = ask(&app, &doc_id).await;
    let (event, data) = events.last().unwrap();
    assert_eq!(event, "error");
    let message = data["message"].as_str().unwrap();
    assert!(
        message.contains("Rate limited") && message.contains("OpenRouter account is out of credit"),
        "{message}"
    );
    assert_eq!(router_rec.lock().unwrap().len(), 1, "402 is not retried");
    let (_, conv) = get_json(&app, &format!("/api/documents/{doc_id}/conversation")).await;
    assert_eq!(conv["turns"], json!([]));
}

#[tokio::test]
async fn openrouter_mid_stream_error_is_reported() {
    let (claude, _) = mock_claude_with_headers(|_| rate_limited()).await;
    let stream = format!(
        "data: {}\n\ndata: {}\n\n",
        json!({"model": "google/gemini-test", "choices": [{"index": 0, "delta": {"content": "Part"}, "finish_reason": null}]}),
        json!({"model": "google/gemini-test", "error": {"code": 502, "message": "provider down"}, "choices": [{"index": 0, "delta": {"content": ""}, "finish_reason": "error"}]}),
    );
    let (router, _) =
        mock_openrouter(move |_| (StatusCode::OK, "text/event-stream", stream.clone())).await;
    let (app, _dir) = test_app_with_fallback(&claude, "test-key", &router);
    let doc_id = upload(&app, 1).await;
    let events = ask(&app, &doc_id).await;
    let (event, data) = events.last().unwrap();
    assert_eq!(event, "error");
    assert!(data["message"].as_str().unwrap().contains("provider down"));
}

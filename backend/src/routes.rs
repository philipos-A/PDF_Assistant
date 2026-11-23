//! HTTP handlers: health, the Document library, Conversations, and streaming chat.

use std::convert::Infallible;
use std::path::Path;
use std::time::Duration;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Multipart, Path as UrlPath, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::AppState;
use crate::claude::{self, Citation, DocumentSource, Role, Turn};
use crate::prompts::ReaderContext;
use crate::storage::{DocumentMeta, StoreError, StoredTurn};

/// Error responses use `{"detail": "..."}`, which the frontend displays.
#[derive(Debug)]
pub struct ApiError(pub StatusCode, pub String);

impl ApiError {
    fn new(status: StatusCode, msg: impl Into<String>) -> Self {
        Self(status, msg.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"detail": self.1}))).into_response()
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound => ApiError::new(StatusCode::NOT_FOUND, "Document not found."),
            StoreError::Io(e) => {
                tracing::error!(error = %e, "storage error");
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Could not access document storage.",
                )
            }
        }
    }
}

type ApiResult<T> = Result<T, ApiError>;

// ---------------------------------------------------------------------------
// Health

pub async fn health(State(state): State<AppState>) -> Json<Value> {
    let s = &state.settings;
    Json(json!({
        "status": "ok",
        "model": s.claude_model,
        "api_key_configured": s.api_key_configured(),
        "max_upload_mb": s.max_upload_mb,
        "max_pages": s.max_pages,
    }))
}

// ---------------------------------------------------------------------------
// Documents

const PDF_CONTENT_TYPES: [&str; 4] = [
    "application/pdf",
    "application/x-pdf",
    "application/octet-stream",
    "",
];

fn too_large(max_mb: u64) -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        format!("File is larger than the {max_mb} MB limit."),
    )
}

/// Count pages, rejecting encrypted or unreadable PDFs.
fn count_pages(data: &[u8]) -> Result<usize, ApiError> {
    let doc = lopdf::Document::load_mem(data).map_err(|e| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("Could not read this PDF: {e}"),
        )
    })?;
    if doc.is_encrypted() {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Password-protected PDFs are not supported.",
        ));
    }
    Ok(doc.get_pages().len())
}

pub async fn upload_document(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> ApiResult<(StatusCode, Json<DocumentMeta>)> {
    let max_mb = state.settings.max_upload_mb;
    let max_bytes = state.settings.max_upload_bytes();
    let map_multipart = |e: axum::extract::multipart::MultipartError| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            too_large(max_mb)
        } else {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("Invalid upload: {}", e.body_text()),
            )
        }
    };

    let mut file = None;
    while let Some(mut field) = multipart.next_field().await.map_err(map_multipart)? {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field
            .file_name()
            .and_then(|n| Path::new(n).file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("document.pdf")
            .to_string();
        let content_type = field.content_type().unwrap_or("").to_string();
        if !filename.to_lowercase().ends_with(".pdf")
            || !PDF_CONTENT_TYPES.contains(&content_type.as_str())
        {
            return Err(ApiError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Only PDF files are supported.",
            ));
        }
        // Read in chunks so an oversized upload is rejected without buffering it all.
        let mut data = Vec::new();
        while let Some(chunk) = field.chunk().await.map_err(map_multipart)? {
            data.extend_from_slice(&chunk);
            if data.len() > max_bytes {
                return Err(too_large(max_mb));
            }
        }
        file = Some((filename, data));
        break;
    }
    let (filename, data) = file.ok_or_else(|| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "No file was uploaded (expected a 'file' field).",
        )
    })?;

    if !data.starts_with(b"%PDF-") {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "This file is not a valid PDF.",
        ));
    }

    let (data, page_count) =
        tokio::task::spawn_blocking(move || count_pages(&data).map(|n| (data, n)))
            .await
            .map_err(|_| {
                ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "PDF parsing failed.")
            })??;

    if page_count == 0 {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "This PDF has no pages.",
        ));
    }
    let max_pages = state.settings.max_pages;
    if page_count > max_pages {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("This PDF has {page_count} pages; the limit is {max_pages}."),
        ));
    }

    let meta = state.store.save(&data, &filename, page_count).await?;
    Ok((StatusCode::CREATED, Json(meta)))
}

pub async fn list_documents(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({"documents": state.store.list().await?})))
}

pub async fn get_document(
    State(state): State<AppState>,
    UrlPath(doc_id): UrlPath<String>,
) -> ApiResult<Json<DocumentMeta>> {
    Ok(Json(state.store.get_meta(&doc_id).await?))
}

/// Delete a Document, its Conversation, and its copy in the Files API (in the background).
pub async fn delete_document(
    State(state): State<AppState>,
    UrlPath(doc_id): UrlPath<String>,
) -> ApiResult<StatusCode> {
    let file_id = state.store.delete(&doc_id).await?;
    if let Some(file_id) = file_id
        && state.settings.api_key_configured()
    {
        let (http, settings) = (state.http.clone(), state.settings.clone());
        tokio::spawn(async move {
            if let Err(e) = claude::delete_file(&http, &settings, &file_id).await {
                tracing::warn!(%file_id, error = %e, "could not delete the uploaded PDF");
            }
        });
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Conversations

pub async fn get_conversation(
    State(state): State<AppState>,
    UrlPath(doc_id): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    state.store.get_meta(&doc_id).await?;
    Ok(Json(json!({"turns": state.store.turns(&doc_id).await?})))
}

pub async fn clear_conversation(
    State(state): State<AppState>,
    UrlPath(doc_id): UrlPath<String>,
) -> ApiResult<StatusCode> {
    state.store.get_meta(&doc_id).await?;
    state.store.clear_conversation(&doc_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_document_file(
    State(state): State<AppState>,
    UrlPath(doc_id): UrlPath<String>,
) -> ApiResult<Response> {
    let meta = state.store.get_meta(&doc_id).await?;
    let bytes = state.store.get_pdf(&doc_id).await?;
    let safe_name: String = meta
        .filename
        .chars()
        .filter(|c| !matches!(c, '"' | '\\' | '\r' | '\n'))
        .collect();
    Ok((
        [
            (header::CONTENT_TYPE, "application/pdf".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("inline; filename=\"{safe_name}\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Chat

/// A Question. The Conversation so far is read from storage, not sent by the browser.
#[derive(Deserialize)]
pub struct ChatRequest {
    doc_id: String,
    question: String,
    #[serde(flatten)]
    context: ReaderContext,
}

fn unprocessable(msg: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, msg)
}

fn validate(req: &ChatRequest) -> ApiResult<()> {
    let q = req.question.trim();
    if q.is_empty() {
        return Err(unprocessable("question must not be empty."));
    }
    if q.chars().count() > 10_000 {
        return Err(unprocessable(
            "question is too long (max 10,000 characters).",
        ));
    }
    if req.context.current_page < 1 {
        return Err(unprocessable("current_page must be at least 1."));
    }
    if req
        .context
        .selected_text
        .as_ref()
        .is_some_and(|s| s.chars().count() > 20_000)
    {
        return Err(unprocessable(
            "selected_text is too long (max 20,000 characters).",
        ));
    }
    Ok(())
}

/// A saved Turn as Claude should see it: Answers lose their citation markers.
fn history_turn(t: StoredTurn) -> Turn {
    let content = match t.role {
        Role::Assistant => claude::strip_citation_markers(&t.content),
        Role::User => t.content,
    };
    Turn {
        role: t.role,
        content,
        context: t.context,
    }
}

/// What the browser has been sent of the Answer so far, to save once it is done.
#[derive(Default)]
struct AnswerSoFar {
    text: String,
    citations: Vec<Citation>,
    used_fallback: bool,
}

impl AnswerSoFar {
    fn record(&mut self, event: &claude::OutEvent) {
        match event.event {
            "delta" => {
                if let Some(t) = event.data["text"].as_str() {
                    self.text.push_str(t);
                }
            }
            "citation" => {
                if let Ok(c) = serde_json::from_value(event.data.clone()) {
                    self.citations.push(c);
                }
            }
            "status" if event.data["state"] == "fallback" => self.used_fallback = true,
            _ => {}
        }
    }

    fn into_turn(self, model: Option<String>) -> StoredTurn {
        StoredTurn {
            role: Role::Assistant,
            content: self.text,
            context: None,
            model,
            used_fallback: self.used_fallback,
            citations: self.citations,
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        }
    }
}

pub async fn chat(
    State(state): State<AppState>,
    body: Result<Json<ChatRequest>, JsonRejection>,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let settings = state.settings.clone();
    if !settings.api_key_configured() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "ANTHROPIC_API_KEY is not set. Copy backend/.env.example to backend/.env, add your key, and restart the backend.",
        ));
    }
    let Json(req) = body.map_err(|e| unprocessable(e.body_text()))?;
    validate(&req)?;

    let not_found = |e: StoreError| match e {
        StoreError::NotFound => ApiError::new(
            StatusCode::NOT_FOUND,
            "Document not found. Upload it again.",
        ),
        other => other.into(),
    };
    let meta = state.store.get_meta(&req.doc_id).await.map_err(not_found)?;
    let pdf = state.store.get_pdf(&req.doc_id).await.map_err(not_found)?;
    let history: Vec<Turn> = state
        .store
        .turns(&req.doc_id)
        .await?
        .into_iter()
        .map(history_turn)
        .collect();

    let question_text = req.question.trim().to_string();
    let question = Turn {
        role: Role::User,
        content: question_text.clone(),
        context: Some(req.context.clone()),
    };
    let saved_question = StoredTurn::question(question_text, req.context);

    let events = async_stream::stream! {
        let store = &state.store;
        let doc_id = meta.doc_id.as_str();
        let mut file_id = meta.file_id.clone();
        let mut reuploaded = false;
        'request: loop {
            if settings.use_files_api && file_id.is_none() {
                match claude::upload_file(&state.http, &settings, pdf.clone(), &meta.filename).await {
                    Ok(id) => {
                        if let Err(e) = store.set_file_id(doc_id, Some(&id)).await {
                            tracing::warn!(?e, "could not save the Files API id");
                        }
                        file_id = Some(id);
                    }
                    // Still answer: send the PDF inline this time.
                    Err(e) => tracing::warn!(error = %e, "Files API upload failed; sending the PDF inline"),
                }
            }
            let source = match &file_id {
                Some(id) => DocumentSource::File(id),
                None => DocumentSource::Base64(&pdf),
            };
            let messages = claude::build_messages(
                claude::document_block(&source, settings.enable_citations),
                history.clone(),
                question.clone(),
                Some(meta.page_count),
            );

            let answer = claude::stream_answer(state.http.clone(), (*settings).clone(), messages);
            futures_util::pin_mut!(answer);
            let mut so_far = AnswerSoFar::default();
            while let Some(event) = answer.next().await {
                if event.event == "file_missing" {
                    let _ = store.set_file_id(doc_id, None).await;
                    file_id = None;
                    if !reuploaded {
                        reuploaded = true;
                        tracing::info!(doc_id, "uploaded PDF is gone; uploading it again");
                        continue 'request;
                    }
                    yield claude::OutEvent::new("error", json!({"message": "Claude could not find the uploaded PDF. Try again."}));
                    break 'request;
                }
                so_far.record(&event);
                if event.event == "done" && !so_far.text.trim().is_empty() {
                    let model = event.data["model"].as_str().map(str::to_string);
                    if let Err(e) = store
                        .append_exchange(doc_id, saved_question.clone(), so_far.into_turn(model))
                        .await
                    {
                        tracing::error!(?e, "could not save the conversation");
                    }
                    so_far = AnswerSoFar::default();
                }
                yield event;
            }
            break;
        }
    }
    .map(|e| Ok(Event::default().event(e.event).data(e.data.to_string())));
    Ok(Sse::new(events).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

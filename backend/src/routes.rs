//! HTTP handlers: health, document upload/fetch, and streaming chat.

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
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::AppState;
use crate::claude::{self, Role, Turn};
use crate::prompts::ReaderContext;
use crate::storage::{DocumentMeta, StoreError};

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

#[derive(Serialize)]
pub struct DocumentOut {
    doc_id: String,
    filename: String,
    page_count: usize,
    size_bytes: usize,
}

impl From<DocumentMeta> for DocumentOut {
    fn from(m: DocumentMeta) -> Self {
        Self {
            doc_id: m.doc_id,
            filename: m.filename,
            page_count: m.page_count,
            size_bytes: m.size_bytes,
        }
    }
}

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
) -> ApiResult<(StatusCode, Json<DocumentOut>)> {
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
    Ok((StatusCode::CREATED, Json(meta.into())))
}

pub async fn get_document(
    State(state): State<AppState>,
    UrlPath(doc_id): UrlPath<String>,
) -> ApiResult<Json<DocumentOut>> {
    Ok(Json(state.store.get_meta(&doc_id).await?.into()))
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

#[derive(Deserialize)]
pub struct ChatRequest {
    doc_id: String,
    question: String,
    #[serde(flatten)]
    context: ReaderContext,
    #[serde(default)]
    history: Vec<Turn>,
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
    if req.history.len() > 200 {
        return Err(unprocessable("history is too long (max 200 turns)."));
    }
    let too_long_selection = |c: Option<&ReaderContext>| {
        c.and_then(|c| c.selected_text.as_ref())
            .is_some_and(|s| s.chars().count() > 20_000)
    };
    if too_long_selection(Some(&req.context))
        || req
            .history
            .iter()
            .any(|t| too_long_selection(t.context.as_ref()))
    {
        return Err(unprocessable(
            "selected_text is too long (max 20,000 characters).",
        ));
    }
    if req
        .history
        .iter()
        .any(|t| t.content.chars().count() > 100_000)
    {
        return Err(unprocessable(
            "A history turn is too long (max 100,000 characters).",
        ));
    }
    Ok(())
}

pub async fn chat(
    State(state): State<AppState>,
    body: Result<Json<ChatRequest>, JsonRejection>,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let settings = &state.settings;
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

    let question = Turn {
        role: Role::User,
        content: req.question.trim().to_string(),
        context: Some(req.context),
    };
    let messages = claude::build_messages(&pdf, req.history, question, Some(meta.page_count));

    let events = claude::stream_answer(state.http.clone(), (**settings).clone(), messages)
        .map(|e| Ok(Event::default().event(e.event).data(e.data.to_string())));
    Ok(Sse::new(events).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

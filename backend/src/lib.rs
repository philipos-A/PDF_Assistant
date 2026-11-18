//! PDF Assistant backend: HTTP API implemented with axum.

pub mod claude;
pub mod config;
pub mod prompts;
pub mod routes;
pub mod storage;

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderValue, Method, header};
use axum::routing::{get, post};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::config::Settings;
use crate::storage::DocumentStore;

#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<Settings>,
    pub store: DocumentStore,
    pub http: reqwest::Client,
}

impl AppState {
    pub fn new(settings: Settings) -> std::io::Result<Self> {
        let store = DocumentStore::new(&settings.storage_dir)?;
        Ok(Self {
            settings: Arc::new(settings),
            store,
            http: claude::http_client(),
        })
    }
}

pub fn app(state: AppState) -> Router {
    let origins: Vec<HeaderValue> = state
        .settings
        .cors_origins
        .iter()
        .filter_map(|o| o.parse().ok())
        .collect();
    let cors = CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::CONTENT_TYPE]);

    // Leave headroom over the file limit for multipart framing; the handler
    // enforces the exact limit with a clear error.
    let upload_limit = state
        .settings
        .max_upload_bytes()
        .saturating_add(1024 * 1024);

    Router::new()
        .route("/api/health", get(routes::health))
        .route(
            "/api/documents",
            post(routes::upload_document).layer(DefaultBodyLimit::max(upload_limit)),
        )
        .route("/api/documents/{doc_id}", get(routes::get_document))
        .route(
            "/api/documents/{doc_id}/file",
            get(routes::get_document_file),
        )
        // Chat bodies carry history and selections, not the PDF.
        .route(
            "/api/chat",
            post(routes::chat).layer(DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

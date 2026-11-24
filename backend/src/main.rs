use pdf_assistant_backend::config::Settings;
use pdf_assistant_backend::{AppState, app};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    let settings = match Settings::from_env() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Configuration error: {e}");
            std::process::exit(1);
        }
    };
    if settings.fallback_configured() {
        tracing::info!(models = ?settings.openrouter_models, "OpenRouter fallback is on");
    }
    if !settings.api_key_configured() {
        if settings.fallback_configured() {
            tracing::warn!("ANTHROPIC_API_KEY is not set; every answer will come from OpenRouter");
        } else {
            tracing::warn!("ANTHROPIC_API_KEY is not set; /api/chat will return 503 until it is");
        }
    }

    let addr = format!("{}:{}", settings.host, settings.port);
    tracing::info!(model = %settings.claude_model, storage = %settings.storage_dir.display(), "listening on http://{addr}");

    let state = AppState::new(settings).expect("could not create storage directory");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("Could not bind {addr}: {e}");
            std::process::exit(1);
        });
    axum::serve(listener, app(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("server error");
}

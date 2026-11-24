//! Settings loaded from environment variables (and `backend/.env`).

use std::env;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Settings {
    pub anthropic_api_key: String,
    pub anthropic_base_url: String,
    /// OpenRouter key for the fallback models; empty means no fallback.
    pub openrouter_api_key: String,
    pub openrouter_base_url: String,
    /// Models tried in order through OpenRouter when Claude is unavailable.
    pub openrouter_models: Vec<String>,
    pub claude_model: String,
    pub claude_effort: String,
    pub max_output_tokens: u32,
    pub enable_fallbacks: bool,
    pub use_files_api: bool,
    pub enable_citations: bool,
    pub max_upload_mb: u64,
    pub max_pages: usize,
    pub storage_dir: PathBuf,
    pub cors_origins: Vec<String>,
    pub host: String,
    pub port: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            anthropic_api_key: String::new(),
            anthropic_base_url: "https://api.anthropic.com".into(),
            openrouter_api_key: String::new(),
            openrouter_base_url: "https://openrouter.ai/api/v1".into(),
            openrouter_models: vec![
                "google/gemini-4-hydrogen".into(),
                "openai/gpt-6.2-sol".into(),
                "google/gemini-3.8-flash".into(),
            ],
            claude_model: "claude-sonnet-5-5".into(),
            claude_effort: "medium".into(),
            max_output_tokens: 16_000,
            enable_fallbacks: true,
            use_files_api: true,
            enable_citations: true,
            max_upload_mb: 24,
            max_pages: 600,
            storage_dir: backend_dir().join("storage"),
            cors_origins: vec![
                "http://localhost:5173".into(),
                "http://127.0.0.1:5173".into(),
            ],
            host: "127.0.0.1".into(),
            port: 8000,
        }
    }
}

/// The crate root (`backend/`), so relative paths work from any working directory.
pub fn backend_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn var(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn parse<T: std::str::FromStr>(name: &str, default: T) -> Result<T, String> {
    match var(name) {
        None => Ok(default),
        Some(v) => v
            .parse()
            .map_err(|_| format!("{name} has an invalid value: {v:?}")),
    }
}

fn parse_bool(name: &str, default: bool) -> Result<bool, String> {
    match var(name).map(|v| v.to_ascii_lowercase()) {
        None => Ok(default),
        Some(v) if ["1", "true", "yes", "on"].contains(&v.as_str()) => Ok(true),
        Some(v) if ["0", "false", "no", "off"].contains(&v.as_str()) => Ok(false),
        Some(v) => Err(format!("{name} must be true or false, got {v:?}")),
    }
}

impl Settings {
    /// Load `backend/.env` (if present) and read settings from the environment.
    ///
    /// Values in `backend/.env` win over the shell's. A shell set up for Claude Code
    /// through OpenRouter exports `ANTHROPIC_API_KEY=""`, which would otherwise hide
    /// the key in `.env`. For the same reason the API address is read from
    /// `CLAUDE_API_BASE_URL`, not from the shell's `ANTHROPIC_BASE_URL`.
    pub fn from_env() -> Result<Self, String> {
        let _ = dotenvy::from_path_override(backend_dir().join(".env"));
        let d = Settings::default();

        let storage_dir = match var("STORAGE_DIR") {
            None => d.storage_dir,
            Some(p) => {
                let p = PathBuf::from(p);
                if p.is_absolute() {
                    p
                } else {
                    backend_dir().join(p)
                }
            }
        };
        let cors_origins = match var("CORS_ORIGINS") {
            None => d.cors_origins,
            Some(v) => v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        };

        Ok(Self {
            anthropic_api_key: var("ANTHROPIC_API_KEY").unwrap_or_default(),
            anthropic_base_url: var("CLAUDE_API_BASE_URL").unwrap_or(d.anthropic_base_url),
            openrouter_api_key: var("OPENROUTER_API_KEY").unwrap_or_default(),
            openrouter_base_url: var("OPENROUTER_BASE_URL").unwrap_or(d.openrouter_base_url),
            openrouter_models: match var("OPENROUTER_MODELS") {
                None => d.openrouter_models,
                Some(v) => v
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
            },
            claude_model: var("CLAUDE_MODEL").unwrap_or(d.claude_model),
            claude_effort: var("CLAUDE_EFFORT").unwrap_or(d.claude_effort),
            max_output_tokens: parse("MAX_OUTPUT_TOKENS", d.max_output_tokens)?,
            enable_fallbacks: parse_bool("ENABLE_FALLBACKS", d.enable_fallbacks)?,
            use_files_api: parse_bool("USE_FILES_API", d.use_files_api)?,
            enable_citations: parse_bool("ENABLE_CITATIONS", d.enable_citations)?,
            max_upload_mb: parse("MAX_UPLOAD_MB", d.max_upload_mb)?,
            max_pages: parse("MAX_PAGES", d.max_pages)?,
            storage_dir,
            cors_origins,
            host: var("HOST").unwrap_or(d.host),
            port: parse("PORT", d.port)?,
        })
    }

    pub fn api_key_configured(&self) -> bool {
        !self.anthropic_api_key.trim().is_empty()
    }

    /// An OpenRouter key and at least one model are set (the `INSERT KEY` placeholder doesn't count).
    pub fn fallback_configured(&self) -> bool {
        let key = self.openrouter_api_key.trim();
        !key.is_empty()
            && !key.eq_ignore_ascii_case("INSERT KEY")
            && !self.openrouter_models.is_empty()
    }

    pub fn max_upload_bytes(&self) -> usize {
        usize::try_from(self.max_upload_mb.saturating_mul(1024 * 1024)).unwrap_or(usize::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huge_upload_limit_does_not_overflow() {
        let s = Settings {
            max_upload_mb: u64::MAX,
            ..Settings::default()
        };
        assert_eq!(s.max_upload_bytes(), usize::MAX);
    }

    #[test]
    fn placeholder_openrouter_key_is_not_a_fallback() {
        let with = |key: &str| Settings {
            openrouter_api_key: key.into(),
            ..Settings::default()
        };
        assert!(!with("").fallback_configured());
        assert!(!with("INSERT KEY").fallback_configured());
        assert!(with("sk-or-v1-abc").fallback_configured());
    }
}

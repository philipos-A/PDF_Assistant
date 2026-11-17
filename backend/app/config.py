"""Application settings, loaded from environment variables and backend/.env."""

from functools import lru_cache
from pathlib import Path

from pydantic import field_validator
from pydantic_settings import BaseSettings, SettingsConfigDict

BACKEND_DIR = Path(__file__).resolve().parent.parent


class Settings(BaseSettings):
    model_config = SettingsConfigDict(
        env_file=BACKEND_DIR / ".env",
        env_file_encoding="utf-8",
        extra="ignore",
    )

    anthropic_api_key: str = ""
    claude_model: str = "claude-sonnet-5-5"
    claude_effort: str = "medium"
    max_output_tokens: int = 16000
    enable_fallbacks: bool = True

    max_upload_mb: int = 24
    max_pages: int = 600
    storage_dir: Path = BACKEND_DIR / "storage"

    cors_origins: str = "http://localhost:5173,http://127.0.0.1:5173"

    @field_validator("storage_dir")
    @classmethod
    def _resolve_storage(cls, v: Path) -> Path:
        return v if v.is_absolute() else (BACKEND_DIR / v).resolve()

    @property
    def cors_origin_list(self) -> list[str]:
        return [o.strip() for o in self.cors_origins.split(",") if o.strip()]

    @property
    def max_upload_bytes(self) -> int:
        return self.max_upload_mb * 1024 * 1024

    @property
    def api_key_configured(self) -> bool:
        return bool(self.anthropic_api_key.strip())


@lru_cache
def get_settings() -> Settings:
    return Settings()

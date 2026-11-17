"""Shared FastAPI dependencies (overridable in tests)."""

from functools import lru_cache

from anthropic import AsyncAnthropic
from fastapi import Depends, HTTPException

from .claude import get_client
from .config import Settings, get_settings
from .storage import DocumentStore


@lru_cache
def _store_for(root: str) -> DocumentStore:
    from pathlib import Path

    return DocumentStore(Path(root))


def get_store(settings: Settings = Depends(get_settings)) -> DocumentStore:
    return _store_for(str(settings.storage_dir))


def get_anthropic(settings: Settings = Depends(get_settings)) -> AsyncAnthropic:
    if not settings.api_key_configured:
        raise HTTPException(
            status_code=503,
            detail=(
                "ANTHROPIC_API_KEY is not set. Copy backend/.env.example to "
                "backend/.env, add your key, and restart the backend."
            ),
        )
    return get_client(settings)

"""FastAPI entry point: `uvicorn app.main:app --reload` from backend/."""

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware

from .config import get_settings
from .routes import chat, documents, health


def create_app() -> FastAPI:
    settings = get_settings()
    app = FastAPI(title="PDF Assistant API", version="0.1.0")
    app.add_middleware(
        CORSMiddleware,
        allow_origins=settings.cors_origin_list,
        allow_credentials=False,
        allow_methods=["GET", "POST"],
        allow_headers=["Content-Type"],
    )
    app.include_router(health.router)
    app.include_router(documents.router)
    app.include_router(chat.router)
    return app


app = create_app()

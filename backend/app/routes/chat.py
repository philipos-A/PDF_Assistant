from typing import Literal

from anthropic import AsyncAnthropic
from fastapi import APIRouter, Depends, HTTPException
from fastapi.responses import StreamingResponse
from pydantic import BaseModel, Field

from ..claude import Turn, build_messages, stream_answer
from ..config import Settings, get_settings
from ..deps import get_anthropic, get_store
from ..storage import DocumentNotFound, DocumentStore

router = APIRouter()


class HistoryTurn(BaseModel):
    role: Literal["user", "assistant"]
    content: str = Field(max_length=100_000)
    # Reader context the user turn was asked with (ignored for assistant turns).
    current_page: int | None = Field(default=None, ge=1)
    visible_pages: list[int] | None = None
    selected_text: str | None = Field(default=None, max_length=20_000)


class ChatRequest(BaseModel):
    doc_id: str
    question: str = Field(min_length=1, max_length=10_000)
    current_page: int = Field(ge=1)
    visible_pages: list[int] = Field(default_factory=list)
    selected_text: str | None = Field(default=None, max_length=20_000)
    history: list[HistoryTurn] = Field(default_factory=list, max_length=200)


@router.post("/api/chat")
async def chat(
    req: ChatRequest,
    settings: Settings = Depends(get_settings),
    store: DocumentStore = Depends(get_store),
    client: AsyncAnthropic = Depends(get_anthropic),
) -> StreamingResponse:
    try:
        meta = store.get_meta(req.doc_id)
        pdf = store.get_pdf_bytes(req.doc_id)
    except DocumentNotFound:
        raise HTTPException(status_code=404, detail="Document not found. Upload it again.") from None

    history = [
        Turn(
            role=h.role,
            content=h.content,
            current_page=h.current_page if h.role == "user" else None,
            visible_pages=h.visible_pages if h.role == "user" else None,
            selected_text=h.selected_text if h.role == "user" else None,
        )
        for h in req.history
    ]
    question = Turn(
        role="user",
        content=req.question.strip(),
        current_page=req.current_page,
        visible_pages=req.visible_pages,
        selected_text=req.selected_text,
    )
    messages = build_messages(pdf, history, question, meta.page_count)

    return StreamingResponse(
        stream_answer(client, settings, messages),
        media_type="text/event-stream",
        headers={"Cache-Control": "no-cache", "X-Accel-Buffering": "no"},
    )

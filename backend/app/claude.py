"""Claude client factory and the streaming chat call."""

from __future__ import annotations

import base64
import json
from collections.abc import AsyncIterator
from dataclasses import dataclass
from functools import lru_cache
from typing import Any, Literal

import anthropic
from anthropic import AsyncAnthropic

from .config import Settings
from .prompts import SYSTEM_PROMPT, build_reader_context

FALLBACK_BETA = "server-side-fallback-2026-07-01"


@lru_cache
def _client_for_key(api_key: str) -> AsyncAnthropic:
    return AsyncAnthropic(api_key=api_key)


def get_client(settings: Settings) -> AsyncAnthropic:
    return _client_for_key(settings.anthropic_api_key)


@dataclass
class Turn:
    role: Literal["user", "assistant"]
    content: str
    current_page: int | None = None
    visible_pages: list[int] | None = None
    selected_text: str | None = None


def _normalize_history(history: list[Turn]) -> list[Turn]:
    """Drop empty turns and enforce user/assistant alternation.

    The result starts with a user turn and ends with an assistant turn, so the
    new question can be appended as the next user turn.
    """
    out: list[Turn] = []
    for turn in history:
        if not turn.content.strip():
            continue
        if not out and turn.role != "user":
            continue
        if out and out[-1].role == turn.role:
            out[-1] = turn  # keep the later of two same-role turns
            continue
        out.append(turn)
    if out and out[-1].role == "user":
        out.pop()
    return out


def _user_text(turn: Turn, page_count: int | None) -> str:
    if turn.current_page is None:
        return turn.content
    ctx = build_reader_context(
        turn.current_page, turn.visible_pages or [], turn.selected_text, page_count
    )
    return f"{ctx}\n\n{turn.content}"


def build_messages(
    pdf_bytes: bytes,
    history: list[Turn],
    question: Turn,
    page_count: int | None = None,
) -> list[dict[str, Any]]:
    """Build the Messages API conversation.

    The PDF is the first block of the first user turn, with a cache breakpoint,
    so system prompt + PDF form a stable prefix that is cached across questions.
    Earlier turns are rebuilt deterministically from what the client sends, so
    the conversation prefix stays byte-identical from one request to the next.
    """
    turns = _normalize_history(history) + [question]
    document_block = {
        "type": "document",
        "source": {
            "type": "base64",
            "media_type": "application/pdf",
            "data": base64.b64encode(pdf_bytes).decode("ascii"),
        },
        "cache_control": {"type": "ephemeral"},
    }
    messages: list[dict[str, Any]] = []
    for i, turn in enumerate(turns):
        if turn.role == "user":
            text_block = {"type": "text", "text": _user_text(turn, page_count)}
            content = [document_block, text_block] if i == 0 else [text_block]
            messages.append({"role": "user", "content": content})
        else:
            messages.append({"role": "assistant", "content": turn.content})
    return messages


def sse(event: str, data: dict[str, Any]) -> str:
    return f"event: {event}\ndata: {json.dumps(data)}\n\n"


async def stream_answer(
    client: AsyncAnthropic,
    settings: Settings,
    messages: list[dict[str, Any]],
) -> AsyncIterator[str]:
    """Stream Claude's answer as Server-Sent Events.

    Events: ``status`` (thinking / fallback), ``delta`` (text), ``done`` (stop
    reason + usage), ``error`` (message).
    """
    extra: dict[str, Any] = {}
    if settings.enable_fallbacks:
        # On a safety-classifier decline, the API re-runs the request on a
        # fallback model inside the same stream.
        extra["betas"] = [FALLBACK_BETA]
        extra["fallbacks"] = "default"

    try:
        async with client.beta.messages.stream(
            model=settings.claude_model,
            max_tokens=settings.max_output_tokens,
            system=SYSTEM_PROMPT,
            # Top-level breakpoint caches the growing conversation as well;
            # the PDF block carries its own breakpoint.
            cache_control={"type": "ephemeral"},
            output_config={"effort": settings.claude_effort},
            messages=messages,
            **extra,
        ) as stream:
            async for event in stream:
                if event.type == "content_block_start":
                    block_type = event.content_block.type
                    if block_type == "thinking":
                        yield sse("status", {"state": "thinking"})
                    elif block_type == "fallback":
                        yield sse("status", {"state": "fallback"})
                    elif block_type == "text":
                        yield sse("status", {"state": "writing"})
                elif event.type == "content_block_delta" and event.delta.type == "text_delta":
                    yield sse("delta", {"text": event.delta.text})

            final = await stream.get_final_message()

        if final.stop_reason == "refusal":
            yield sse(
                "error",
                {"message": "Claude declined to answer this question. Try rephrasing it."},
            )
            return

        usage = final.usage
        yield sse(
            "done",
            {
                "stop_reason": final.stop_reason,
                "model": final.model,
                "usage": {
                    "input_tokens": usage.input_tokens,
                    "output_tokens": usage.output_tokens,
                    "cache_read_input_tokens": usage.cache_read_input_tokens or 0,
                    "cache_creation_input_tokens": usage.cache_creation_input_tokens or 0,
                },
            },
        )
    except anthropic.AuthenticationError:
        yield sse("error", {"message": "The Anthropic API key was rejected. Check ANTHROPIC_API_KEY in backend/.env."})
    except anthropic.PermissionDeniedError:
        yield sse("error", {"message": "This API key does not have access to the configured model."})
    except anthropic.NotFoundError:
        yield sse("error", {"message": f"Model '{settings.claude_model}' was not found. Check CLAUDE_MODEL in backend/.env."})
    except anthropic.RateLimitError:
        yield sse("error", {"message": "Rate limited by the Anthropic API. Wait a moment and try again."})
    except anthropic.BadRequestError as e:
        yield sse("error", {"message": f"The request was rejected: {_api_message(e)}"})
    except anthropic.APIStatusError as e:
        yield sse("error", {"message": f"Anthropic API error ({e.status_code}): {_api_message(e)}"})
    except anthropic.APIConnectionError:
        yield sse("error", {"message": "Could not reach the Anthropic API. Check your network connection."})


def _api_message(e: anthropic.APIStatusError) -> str:
    body = e.body if isinstance(e.body, dict) else {}
    err = body.get("error") if isinstance(body.get("error"), dict) else {}
    return err.get("message") or str(e)

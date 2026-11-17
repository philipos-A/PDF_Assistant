import json

from fastapi.testclient import TestClient

from app.config import Settings, get_settings
from app.main import create_app

from .conftest import FakeAnthropic, make_pdf


def parse_sse(text: str) -> list[tuple[str, dict]]:
    events = []
    for block in text.strip().split("\n\n"):
        lines = dict(line.split(": ", 1) for line in block.splitlines())
        events.append((lines["event"], json.loads(lines["data"])))
    return events


def upload(client) -> str:
    r = client.post("/api/documents", files={"file": ("p.pdf", make_pdf(4), "application/pdf")})
    return r.json()["doc_id"]


def test_chat_streams_and_builds_cached_request(client, fake_anthropic, settings):
    doc_id = upload(client)
    payload = {
        "doc_id": doc_id,
        "question": "What does this mean?",
        "current_page": 2,
        "visible_pages": [2, 3],
        "selected_text": "attention is all you need",
        "history": [
            {"role": "user", "content": "Summarize p. 1", "current_page": 1, "visible_pages": [1]},
            {"role": "assistant", "content": "It introduces..."},
        ],
    }
    r = client.post("/api/chat", json=payload)
    assert r.status_code == 200
    assert r.headers["content-type"].startswith("text/event-stream")

    events = parse_sse(r.text)
    text = "".join(d["text"] for e, d in events if e == "delta")
    assert text == "Hello from p. 2"
    assert events[-1][0] == "done"
    assert events[-1][1]["usage"]["cache_read_input_tokens"] == 1000

    call = fake_anthropic.calls[0]
    assert call["model"] == settings.claude_model
    msgs = call["messages"]
    assert [m["role"] for m in msgs] == ["user", "assistant", "user"]

    # PDF goes first, as a cached base64 document block, only in the first turn.
    doc_block = msgs[0]["content"][0]
    assert doc_block["type"] == "document"
    assert doc_block["source"]["media_type"] == "application/pdf"
    assert doc_block["cache_control"] == {"type": "ephemeral"}
    assert all(b["type"] == "text" for b in msgs[2]["content"])

    # Reader context is attached to the new question.
    last = msgs[2]["content"][0]["text"]
    assert "Current page: 2 of 4" in last
    assert "Pages visible on screen: 2, 3" in last
    assert "attention is all you need" in last
    assert last.endswith("What does this mean?")

    # Earlier user turns are rebuilt with their own context (stable cache prefix).
    assert "Current page: 1 of 4" in msgs[0]["content"][1]["text"]

    # Refusal fallback is enabled by default.
    assert call["fallbacks"] == "default"


def test_chat_refusal_returns_error_event(client, fake_anthropic):
    fake_anthropic.stop_reason = "refusal"
    fake_anthropic.chunks = ()
    doc_id = upload(client)
    r = client.post("/api/chat", json={"doc_id": doc_id, "question": "hi", "current_page": 1})
    events = parse_sse(r.text)
    assert events[-1][0] == "error"


def test_chat_unknown_document(client):
    r = client.post("/api/chat", json={"doc_id": "f" * 32, "question": "hi", "current_page": 1})
    assert r.status_code == 404


def test_chat_missing_api_key(tmp_path):
    app = create_app()
    app.dependency_overrides[get_settings] = lambda: Settings(
        _env_file=None, anthropic_api_key="", storage_dir=tmp_path
    )
    with TestClient(app) as c:
        r = c.post("/api/chat", json={"doc_id": "f" * 32, "question": "hi", "current_page": 1})
        assert r.status_code == 503
        assert "ANTHROPIC_API_KEY" in r.json()["detail"]
        assert c.get("/api/health").json()["api_key_configured"] is False

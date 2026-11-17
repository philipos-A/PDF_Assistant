import io
from types import SimpleNamespace

import pytest
from fastapi.testclient import TestClient
from pypdf import PdfWriter

from app.config import Settings, get_settings
from app.deps import get_anthropic
from app.main import create_app


def make_pdf(pages: int = 2) -> bytes:
    writer = PdfWriter()
    for _ in range(pages):
        writer.add_blank_page(width=612, height=792)
    out = io.BytesIO()
    writer.write(out)
    return out.getvalue()


class FakeStream:
    """Mimics the async context manager returned by client.beta.messages.stream()."""

    def __init__(self, events, final):
        self._events = events
        self._final = final

    async def __aenter__(self):
        return self

    async def __aexit__(self, *exc):
        return False

    def __aiter__(self):
        async def gen():
            for e in self._events:
                yield e

        return gen()

    async def get_final_message(self):
        return self._final


class FakeAnthropic:
    """Records stream() kwargs and replays a canned answer."""

    def __init__(self, chunks=("Hello ", "from p. 2"), stop_reason="end_turn"):
        self.calls: list[dict] = []
        self.chunks = chunks
        self.stop_reason = stop_reason
        self.beta = SimpleNamespace(messages=SimpleNamespace(stream=self._stream))

    def _stream(self, **kwargs):
        self.calls.append(kwargs)
        events = [SimpleNamespace(type="content_block_start", content_block=SimpleNamespace(type="text"))]
        events += [
            SimpleNamespace(type="content_block_delta", delta=SimpleNamespace(type="text_delta", text=c))
            for c in self.chunks
        ]
        final = SimpleNamespace(
            stop_reason=self.stop_reason,
            model=kwargs["model"],
            usage=SimpleNamespace(
                input_tokens=10,
                output_tokens=5,
                cache_read_input_tokens=1000,
                cache_creation_input_tokens=0,
            ),
        )
        return FakeStream(events, final)


@pytest.fixture
def settings(tmp_path):
    return Settings(
        _env_file=None,
        anthropic_api_key="test-key",
        storage_dir=tmp_path / "storage",
        max_upload_mb=1,
    )


@pytest.fixture
def fake_anthropic():
    return FakeAnthropic()


@pytest.fixture
def client(settings, fake_anthropic):
    app = create_app()
    app.dependency_overrides[get_settings] = lambda: settings
    app.dependency_overrides[get_anthropic] = lambda: fake_anthropic
    with TestClient(app) as c:
        yield c

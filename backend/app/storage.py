"""On-disk storage for uploaded PDFs.

Each document is stored as ``<doc_id>.pdf`` with a ``<doc_id>.json`` metadata
file next to it.
"""

from __future__ import annotations

import json
import re
import uuid
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from pathlib import Path

DOC_ID_RE = re.compile(r"^[0-9a-f]{32}$")


@dataclass
class DocumentMeta:
    doc_id: str
    filename: str
    page_count: int
    size_bytes: int
    uploaded_at: str


class DocumentNotFound(Exception):
    pass


class DocumentStore:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.root.mkdir(parents=True, exist_ok=True)

    def _pdf_path(self, doc_id: str) -> Path:
        if not DOC_ID_RE.match(doc_id):
            # Also guards against path traversal via doc_id.
            raise DocumentNotFound(doc_id)
        return self.root / f"{doc_id}.pdf"

    def _meta_path(self, doc_id: str) -> Path:
        return self._pdf_path(doc_id).with_suffix(".json")

    def save(self, data: bytes, filename: str, page_count: int) -> DocumentMeta:
        doc_id = uuid.uuid4().hex
        meta = DocumentMeta(
            doc_id=doc_id,
            filename=filename,
            page_count=page_count,
            size_bytes=len(data),
            uploaded_at=datetime.now(UTC).isoformat(),
        )
        self._pdf_path(doc_id).write_bytes(data)
        self._meta_path(doc_id).write_text(json.dumps(asdict(meta)))
        return meta

    def get_meta(self, doc_id: str) -> DocumentMeta:
        path = self._meta_path(doc_id)
        if not path.exists():
            raise DocumentNotFound(doc_id)
        return DocumentMeta(**json.loads(path.read_text()))

    def get_pdf_bytes(self, doc_id: str) -> bytes:
        path = self._pdf_path(doc_id)
        if not path.exists():
            raise DocumentNotFound(doc_id)
        return path.read_bytes()

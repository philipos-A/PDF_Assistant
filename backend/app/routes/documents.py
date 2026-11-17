import io
from pathlib import PurePath

from fastapi import APIRouter, Depends, File, HTTPException, UploadFile
from fastapi.responses import FileResponse
from pydantic import BaseModel
from pypdf import PdfReader
from pypdf.errors import PdfReadError

from ..config import Settings, get_settings
from ..deps import get_store
from ..storage import DocumentNotFound, DocumentStore

router = APIRouter()

PDF_CONTENT_TYPES = {"application/pdf", "application/x-pdf", "application/octet-stream", ""}
CHUNK = 1024 * 1024


class DocumentOut(BaseModel):
    doc_id: str
    filename: str
    page_count: int
    size_bytes: int


@router.post("/api/documents", response_model=DocumentOut, status_code=201)
async def upload_document(
    file: UploadFile = File(...),
    settings: Settings = Depends(get_settings),
    store: DocumentStore = Depends(get_store),
) -> DocumentOut:
    filename = PurePath(file.filename or "document.pdf").name
    if not filename.lower().endswith(".pdf") or (file.content_type or "") not in PDF_CONTENT_TYPES:
        raise HTTPException(status_code=415, detail="Only PDF files are supported.")

    # Read in chunks so an oversized upload is rejected without buffering it all.
    buf = bytearray()
    while chunk := await file.read(CHUNK):
        buf.extend(chunk)
        if len(buf) > settings.max_upload_bytes:
            raise HTTPException(
                status_code=413,
                detail=f"File is larger than the {settings.max_upload_mb} MB limit.",
            )
    data = bytes(buf)

    if not data.startswith(b"%PDF-"):
        raise HTTPException(status_code=415, detail="This file is not a valid PDF.")

    try:
        reader = PdfReader(io.BytesIO(data))
        if reader.is_encrypted:
            raise HTTPException(status_code=422, detail="Password-protected PDFs are not supported.")
        page_count = len(reader.pages)
    except PdfReadError as e:
        raise HTTPException(status_code=422, detail=f"Could not read this PDF: {e}") from e

    if page_count == 0:
        raise HTTPException(status_code=422, detail="This PDF has no pages.")
    if page_count > settings.max_pages:
        raise HTTPException(
            status_code=422,
            detail=f"This PDF has {page_count} pages; the limit is {settings.max_pages}.",
        )

    meta = store.save(data, filename, page_count)
    return DocumentOut(
        doc_id=meta.doc_id, filename=meta.filename, page_count=meta.page_count, size_bytes=meta.size_bytes
    )


@router.get("/api/documents/{doc_id}", response_model=DocumentOut)
def get_document(doc_id: str, store: DocumentStore = Depends(get_store)) -> DocumentOut:
    try:
        meta = store.get_meta(doc_id)
    except DocumentNotFound:
        raise HTTPException(status_code=404, detail="Document not found.") from None
    return DocumentOut(
        doc_id=meta.doc_id, filename=meta.filename, page_count=meta.page_count, size_bytes=meta.size_bytes
    )


@router.get("/api/documents/{doc_id}/file")
def get_document_file(doc_id: str, store: DocumentStore = Depends(get_store)) -> FileResponse:
    try:
        meta = store.get_meta(doc_id)
    except DocumentNotFound:
        raise HTTPException(status_code=404, detail="Document not found.") from None
    return FileResponse(
        store.root / f"{doc_id}.pdf", media_type="application/pdf", filename=meta.filename
    )

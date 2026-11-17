
from .conftest import make_pdf


def test_health(client):
    r = client.get("/api/health")
    assert r.status_code == 200
    body = r.json()
    assert body["status"] == "ok"
    assert body["api_key_configured"] is True


def test_upload_pdf(client, settings):
    r = client.post("/api/documents", files={"file": ("paper.pdf", make_pdf(3), "application/pdf")})
    assert r.status_code == 201, r.text
    body = r.json()
    assert body["filename"] == "paper.pdf"
    assert body["page_count"] == 3
    assert (settings.storage_dir / f"{body['doc_id']}.pdf").exists()

    meta = client.get(f"/api/documents/{body['doc_id']}")
    assert meta.status_code == 200
    f = client.get(f"/api/documents/{body['doc_id']}/file")
    assert f.status_code == 200
    assert f.content.startswith(b"%PDF-")


def test_rejects_non_pdf(client):
    r = client.post("/api/documents", files={"file": ("notes.txt", b"hello", "text/plain")})
    assert r.status_code == 415


def test_rejects_fake_pdf(client):
    r = client.post("/api/documents", files={"file": ("fake.pdf", b"not really a pdf", "application/pdf")})
    assert r.status_code == 415


def test_rejects_oversized(client):
    big = b"%PDF-1.7\n" + b"0" * (1024 * 1024 + 10)  # max_upload_mb=1 in tests
    r = client.post("/api/documents", files={"file": ("big.pdf", big, "application/pdf")})
    assert r.status_code == 413


def test_unknown_or_malicious_doc_id(client):
    assert client.get("/api/documents/" + "0" * 32).status_code == 404
    assert client.get("/api/documents/..%2F..%2Fetc").status_code == 404

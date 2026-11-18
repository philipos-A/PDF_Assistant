# PDF Assistant

A reading companion for dense academic papers. Open a PDF, scroll through it, and
ask about terms, equations, figures or anything else by **typing or speaking**.
Answers know which page you're on and what you've highlighted, cite page numbers
you can click to jump to, and render math with KaTeX.

## Features

- **Continuous-scroll PDF viewer** (PDF.js via `react-pdf`) with zoom, a page
  indicator and jump-to-page. Only pages near the viewport are rendered, so long
  papers stay fast.
- **Position-aware answers.** The viewer tracks the current and visible pages
  with an `IntersectionObserver`, and every question is sent with that context.
- **Ask about a selection.** Highlight text and click **Ask about this** to
  attach it to your next question. Sending with no text asks for a plain-language
  explanation.
- **Streaming chat** with Markdown, LaTeX math (`$...$`, `$$...$$`) and tables.
- **Clickable page references.** "p. 4", "pp. 5–6" and "page 12" in answers
  scroll the viewer to that page. Each answer also shows which page it was asked from.
- **Push-to-talk voice input** (Web Speech API). Hold the mic and release to
  send, or tap once for hands-free and tap again to send. Shows a live transcript.
- **Read-aloud answers** (`speechSynthesis`), switched on with the **Voice** toggle.
- **Cheap follow-up questions.** The PDF is sent to Claude as a native document
  block with prompt caching, so later questions on the same paper reuse the cached copy.

## Architecture

```
┌──────────────────────────── Browser (localhost:5173) ────────────────────────────┐
│  React + Vite + TS + Tailwind                                                     │
│  ┌──────────────────────────────┐   ┌───────────────────────────────────────────┐ │
│  │ PdfViewer (react-pdf/PDF.js) │   │ ChatPanel                                 │ │
│  │  • IntersectionObserver ─────┼──▶│  • streaming markdown + KaTeX             │ │
│  │    current/visible pages     │   │  • mic (SpeechRecognition), read-aloud    │ │
│  │  • text selection popover ───┼──▶│  • "p. 4" links ──▶ viewer.scrollToPage() │ │
│  └──────────────────────────────┘   └───────────────────────────────────────────┘ │
└──────────────────────────────────────┬────────────────────────────────────────────┘
                                       │  /api/*  (Vite dev proxy, same origin)
                                       ▼
┌──────────────────── Rust backend: axum (localhost:8000) ──────────────────────────┐
│  POST /api/documents  → validate → backend/storage/<doc_id>.pdf (+ .json meta)    │
│  GET  /api/documents/{id}[/file]                                                  │
│  POST /api/chat       → system prompt + [PDF document block, cache_control]       │
│                          + history + <reader_context> + question                  │
│                       ← Server-Sent Events: status / delta / done / error         │
│  GET  /api/health                                                                 │
│                                                                                   │
│  ANTHROPIC_API_KEY lives only in backend/.env                                     │
└──────────────────────────────────────┬────────────────────────────────────────────┘
                                       │  reqwest → POST /v1/messages (stream: true)
                                       ▼
                               Claude API (Messages)
```

### How a question is sent

1. The PDF is the **first block of the first user turn**, as a base64 `document`
   block with `cache_control: {"type": "ephemeral"}`. The system prompt plus the
   PDF form a stable prefix that is cached for about 5 minutes after each use.
2. Earlier turns follow. Each earlier user turn is rebuilt from the page and
   selection it was originally asked with, so the conversation prefix stays
   byte-identical from one request to the next and can be cached too.
3. The new question is prefixed with a `<reader_context>` block (current page,
   visible pages, selected text).
4. The answer streams back as SSE. The `done` event includes token usage,
   including `cache_read_input_tokens`, so you can confirm caching is working.

Anthropic has no official Rust SDK, so the backend calls the Messages API
directly over HTTP (`anthropic-version: 2026-10-21`) and parses the
streamed response itself. Rate-limit, overload and server errors are retried
up to twice before any output has streamed, honouring `retry-after`.

The default model is `claude-sonnet-5-5` with effort `medium`. If a safety
classifier declines a request, the server-side fallback (`fallbacks: "default"`)
re-runs it on a suitable fallback model in the same stream. Answers served by
the fallback are labelled in the UI. Set `ENABLE_FALLBACKS=false` to turn this off.

## Setup

**Requirements:** Rust 1.88+ ([rustup.rs](https://rustup.rs)), Node.js 20+, and an Anthropic API key.

### 1. Get an Anthropic API key

1. Sign in at [console.anthropic.com](https://console.anthropic.com/).
2. Add billing under **Settings → Billing**.
3. Create a key under **Settings → API keys** and copy it. It is shown only once.

### 2. Install

```bash
make setup                # builds the Rust backend, installs npm deps,
                          # and copies backend/.env.example to backend/.env
```

Then edit `backend/.env`:

```bash
ANTHROPIC_API_KEY=sk-ant-...
```

<details>
<summary>Manual setup (without make)</summary>

```bash
# Backend
cd backend
cargo build --release
cp .env.example .env      # then add your key

# Frontend
cd ../frontend
npm install
```
</details>

### 3. Run

```bash
make dev                  # or ./dev.sh
```

Open <http://localhost:5173>. The backend runs on <http://localhost:8000>
(check <http://localhost:8000/api/health>). To run each half on its own, use
`make backend` and `make frontend`.

### Tests and build

```bash
make test                 # cargo test: unit tests + API tests against a mock Claude server
make lint                 # clippy + rustfmt check
make build                # release backend binary + frontend production build
```

## Configuration (`backend/.env`)

| Variable            | Default                                       | Purpose                                                      |
| ------------------- | --------------------------------------------- | ------------------------------------------------------------ |
| `ANTHROPIC_API_KEY` | *(required)*                                  | Your API key. Never sent to the browser.                     |
| `CLAUDE_MODEL`      | `claude-sonnet-5-5`                           | Any current Claude model ID.                                 |
| `CLAUDE_EFFORT`     | `medium`                                      | `low` / `medium` / `high` / `xhigh` / `max`.                 |
| `MAX_OUTPUT_TOKENS` | `16000`                                       | Cap on answer length.                                        |
| `ENABLE_FALLBACKS`  | `true`                                        | Server-side refusal fallback.                                |
| `MAX_UPLOAD_MB`     | `24`                                          | Upload size limit (see limitations).                         |
| `MAX_PAGES`         | `600`                                         | Page limit (Claude's maximum for PDFs).                      |
| `STORAGE_DIR`       | `storage`                                     | Where uploaded PDFs are kept, relative to `backend/`.        |
| `CORS_ORIGINS`      | `http://localhost:5173,http://127.0.0.1:5173` | Allowed browser origins if you call the API directly.        |
| `HOST` / `PORT`     | `127.0.0.1` / `8000`                          | Address the backend listens on.                              |
| `ANTHROPIC_BASE_URL`| `https://api.anthropic.com`                   | Send requests to a proxy or mock server instead.             |
| `RUST_LOG`          | `info`                                        | Log filter, e.g. `debug`.                                    |

## Project layout

```
backend/                  Rust crate (axum + tokio + reqwest)
  src/main.rs             entry point: load settings, bind, serve
  src/lib.rs              router, CORS, body limits, shared state
  src/config.rs           settings from env / .env
  src/storage.rs          PDF + metadata storage on disk
  src/prompts.rs          system prompt and reader-context builder
  src/claude.rs           message building, HTTP call, SSE parsing, retries, error mapping
  src/routes.rs           health, documents, chat handlers
  tests/api.rs            API tests against a local mock Claude server
frontend/
  src/App.tsx             layout, resizable/collapsible chat, doc restore
  src/components/         PdfViewer, ChatPanel, Message, MicButton, UploadView, …
  src/hooks/              speech recognition, speech synthesis, text selection
  src/lib/                page-reference linking, text-to-speech cleanup
```

## Known limitations

- **Claude's PDF limits:** a request can be at most **128 MB** and a PDF at most
  **600 pages** (100 on models with smaller context windows). Base64 encoding adds
  about 33%, so the default 24 MB upload limit becomes about 32 MB per request;
  raise `MAX_UPLOAD_MB` if you need larger files. Very long or image-heavy papers also use many input tokens per question,
  and only the first one within a cache window pays full price.
- **Scanned PDFs** without a text layer can't be highlighted in the viewer.
  Claude can still read them, because it sees each page as an image.
- **Prompt cache lifetime** is about 5 minutes after the last use. After a longer
  pause, the next question re-sends the PDF at full input price.
- **Voice input** uses the Web Speech API: supported in Chrome, Edge and Safari,
  but **not Firefox**, where the mic is shown disabled with a tooltip. Chrome sends
  audio to Google's speech service, so it needs a network connection. Microphone
  access requires `localhost` or HTTPS.
- **Read-aloud** uses the browser's built-in voices. Equations are announced as
  "equation shown on screen" rather than read symbol by symbol.
- **No official Rust SDK:** new API features have to be added by hand in
  `src/claude.rs`, rather than arriving with an SDK update.
- **Password-protected PDFs** are rejected at upload, including ones that only
  restrict printing or copying.
- **No hot reload** for the backend: restart `make dev` after changing Rust
  code, or install `cargo-watch` and run `cargo watch -x run` in `backend/`.
- **Storage** is a local folder with no authentication or cleanup, so it's meant
  for single-user local use. Chat history lives in the browser tab and is cleared
  on reload. The open document is restored after a reload.

## Suggested next steps

- Persist chat history per document (SQLite) and add a document library.
- Use the Files API to upload each PDF once instead of re-sending base64.
- Enable Claude's citations on the document block for exact quoted passages.
- Highlight cited passages in the viewer, and add search inside the PDF.
- Add frontend tests (Vitest + Testing Library) and CI.

## License

[MIT](LICENSE)

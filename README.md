# PDF Assistant

A reading companion for dense academic papers. Open a PDF, scroll through it, and
ask about terms, equations, figures or anything else by **typing or speaking**.
Answers know which page you're on and what you've highlighted, cite page numbers
you can click to jump to, and render math with KaTeX.

## Features

- **Library.** Every uploaded paper is kept, with its questions and answers.
  The start page lists your papers, most recently used first; open one to carry
  on where you left off, or delete it (and its conversation).
- **Continuous-scroll PDF viewer** (PDF.js via `react-pdf`) with zoom, a page
  indicator and jump-to-page. Only pages near the viewport are rendered, so long
  papers stay fast.
- **Position-aware answers.** The viewer tracks the current and visible pages
  with an `IntersectionObserver`, and every question is sent with that context.
- **Search inside the PDF.** Press Ctrl+F / Cmd+F (or the search button) to find
  text across every page. Matches are highlighted; Enter and Shift+Enter step
  through them. Matching ignores case, line breaks and hyphenation.
- **Ask about a selection.** Highlight text and click **Ask about this** to
  attach it to your next question. Sending with no text asks for a plain-language
  explanation.
- **Streaming chat** with Markdown, LaTeX math (`$...$`, `$$...$$`) and tables.
- **Cited answers.** Claude's citations come back as numbered chips in the answer,
  with a **Sources** list underneath. Click one to scroll to the passage and
  highlight it in the PDF (press Escape to clear the highlight).
- **Live highlights.** While an answer streams in, each passage it cites is
  highlighted in the PDF as the citation arrives, with a highlighter that sweeps
  across the text line by line. With **Voice** on, the highlight follows the voice
  instead: each passage is swept as the sentence that relies on it is spoken.
- **Answered passages stay marked.** Text you asked about (indigo) and passages
  that answers cited (amber) stay underlined in the PDF. Click one to open a card
  with every saved answer about it: listen to it, follow its citations, or jump to
  it in the chat (**In chat**). The highlighter button in the viewer toolbar hides
  or shows these marks.
- **Clickable page references.** "p. 4", "pp. 5–6" and "page 12" in answers
  scroll the viewer to that page. Each answer also shows which page it was asked from.
- **Push-to-talk voice input** (Web Speech API). Hold the mic and release to
  send, or tap once for hands-free and tap again to send. Shows a live transcript.
- **Natural read-aloud voice.** Switch on **Voice** to have answers read aloud by
  [Kokoro](https://huggingface.co/hexgrad/Kokoro-82M), an open text-to-speech model
  that runs in your browser (via `kokoro-js`, on WebGPU where available), so it's
  free and the answer text never leaves your machine. Reading starts at the first
  finished sentence while the answer is still streaming; **Stop** in the chat header
  stops it. Any answer can also be replayed with **Read aloud**. If Kokoro can't run,
  the browser's built-in voice (`speechSynthesis`) is used instead.
- **Saved conversations.** Questions and answers are stored per paper in SQLite,
  so they survive a reload or a restart. **Clear** in the chat header deletes them.
- **Cheap follow-up questions.** Each PDF is uploaded to Claude's Files API once
  and referenced by id, and the document block is prompt-cached, so later
  questions on the same paper reuse the cached copy.
- **Other models when Claude is unavailable.** If Claude can't answer because
  its usage or credit has run out, it's rate limited or overloaded, or it can't
  be reached, the same question goes to other models through
  [OpenRouter](https://openrouter.ai) (by default a Gemini model, then a GPT
  model, then a smaller Gemini model, tried in that order by OpenRouter). They
  get the same instructions, the PDF, the saved conversation and your reader
  context, and they tag the passages they rely on, so answers still come with
  citation chips and live highlights. The answer shows which model wrote it
  ("answered by gemini-…"). This switch happens only before any of the answer has
  streamed; an answer that fails part-way through still reports an error. Set
  `OPENROUTER_API_KEY` to turn it on; without an Anthropic key, OpenRouter
  answers every question.

## Architecture

```
┌──────────────────────────── Browser (localhost:5173) ────────────────────────────┐
│  React + Vite + TS + Tailwind                                                     │
│  ┌──────────────────────────────┐   ┌───────────────────────────────────────────┐ │
│  │ PdfViewer (react-pdf/PDF.js) │   │ ChatPanel                                 │ │
│  │  • IntersectionObserver ─────┼──▶│  • streaming markdown + KaTeX             │ │
│  │    current/visible pages     │   │  • mic (SpeechRecognition), read-aloud    │ │
│  │  • text selection popover ───┼──▶│  • "p. 4" links ──▶ viewer.scrollToPage() │ │
│  │  • search + highlights ◀─────┼───│  • citation chips ─▶ viewer.showCitation()│ │
│  └──────────────────────────────┘   └───────────────────────────────────────────┘ │
└──────────────────────────────────────┬────────────────────────────────────────────┘
                                       │  /api/*  (Vite dev proxy, same origin)
                                       ▼
┌──────────────────── Rust backend: axum (localhost:8000) ──────────────────────────┐
│  POST   /api/documents → validate → storage/<doc_id>.pdf + row in library.db     │
│  GET    /api/documents          library, most recently active first              │
│  GET    /api/documents/{id}[/file]     DELETE /api/documents/{id}                 │
│  GET    /api/documents/{id}/conversation   DELETE …/conversation (clear)          │
│  POST   /api/chat → system prompt + [PDF by Files API id, cache_control,          │
│                      citations] + saved Turns + <reader_context> + question       │
│                   ← Server-Sent Events: status / delta / citation / done / error │
│                   → on done, the Question and Answer are saved to library.db      │
│  GET    /api/health                                                               │
│                                                                                   │
│  ANTHROPIC_API_KEY lives only in backend/.env                                     │
└──────────────────────────────────────┬────────────────────────────────────────────┘
                                       │  reqwest → POST /v1/files (once per PDF)
                                       │            POST /v1/messages (stream: true)
                                       ▼
                               Claude API (Files, Messages)
```

### How a question is sent

1. The first time a paper is asked about, the backend uploads it to the
   [Files API](https://platform.claude.com/docs/en/build-with-claude/files) and
   stores the returned `file_id`. The PDF is then the **first block of the first
   user turn**, as a `document` block whose source is that file id, with
   `cache_control: {"type": "ephemeral"}` and `citations: {"enabled": true}`.
   The system prompt plus the PDF form a stable prefix that is cached for about
   5 minutes after each use. If the upload fails, the PDF is sent inline as base64
   for that question instead. If Claude reports the file is gone (for example
   after switching API keys), the backend uploads it again and retries once.
2. Earlier turns follow, read from the saved conversation, not sent by the
   browser. Each earlier user turn is rebuilt from the page and selection it was
   originally asked with, and earlier answers are sent without their citation
   markers, so the conversation prefix stays byte-identical from one request to
   the next and can be cached too.
3. The new question is prefixed with a `<reader_context>` block (current page,
   visible pages, selected text and the page it was selected on).
4. The answer streams back as SSE. When Claude cites a passage
   (`citations_delta` with a `page_location`), the backend sends a `citation`
   event (number, cited text, first and last page) and then writes a
   `[n](#cite-n)` marker into the answer text, which the UI shows as a chip.
   The same passage cited again reuses its number. The `done` event includes
   token usage, including `cache_read_input_tokens`, so you can confirm caching
   is working.
5. After `done`, the question (with its reader context) and the answer (with its
   citations and the model that wrote it) are saved together. Answers that fail,
   are declined or are stopped are not saved.
6. If Claude returns an error meaning it can't serve right now (HTTP 402, 408,
   429, 5xx or a "credit balance is too low" 400, a connection failure, or an
   overload or rate-limit error in the stream) before any text, and
   `OPENROUTER_API_KEY` is set, the backend sends a `status: fallback` event and
   asks OpenRouter instead (`POST /api/v1/chat/completions` with a `models` list,
   so OpenRouter moves on to the next model if one is down). The PDF goes inline
   as a base64 `file` part. The system prompt gains one instruction: write
   `{{cite p=N: exact quote}}` before a sentence that uses a passage. The backend
   turns each tag into the same `citation` event and `[n](#cite-n)` marker as a
   Claude citation. If OpenRouter fails too, the error names both reasons.

Anthropic has no official Rust SDK, so the backend calls the Messages API
directly over HTTP (`anthropic-version: 2026-10-21`) and parses the
streamed response itself. Rate-limit, overload and server errors are retried
up to twice before any output has streamed, whether they arrive as an HTTP
status or as an `error` event in the stream. Retries honour `retry-after-ms`,
`retry-after` and `x-should-retry`; if the server asks for a wait longer than
10 seconds, the error is shown right away instead.

If Claude declines a question partway through an answer, the partial answer is
discarded and only the error is shown.

The default model is `claude-sonnet-5-5` with effort `medium`. If a safety
classifier declines a request, the server-side fallback (`fallbacks: "default"`)
re-runs it on a suitable fallback model in the same stream. Answers served by
the fallback are labelled in the UI. Set `ENABLE_FALLBACKS=false` to turn this off.

## Setup

**Requirements:** Rust 1.89+ ([rustup.rs](https://rustup.rs)), Node.js 20+, and an Anthropic API key.

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
make test                 # cargo test (unit + API tests against a mock Claude server), then Vitest
                          # (helpers in src/lib and component tests with Testing Library)
make lint                 # clippy + rustfmt check
make build                # release backend binary + frontend production build
```

## Configuration (`backend/.env`)

Values in `backend/.env` take priority over the same variables set in your shell,
so a shell set up for Claude Code (for example with `ANTHROPIC_API_KEY=""`) doesn't
hide the key in `.env`.

| Variable            | Default                                       | Purpose                                                      |
| ------------------- | --------------------------------------------- | ------------------------------------------------------------ |
| `ANTHROPIC_API_KEY` | *(required)*                                  | Your API key. Never sent to the browser.                     |
| `CLAUDE_MODEL`      | `claude-sonnet-5-5`                           | Any current Claude model ID.                                 |
| `CLAUDE_EFFORT`     | `medium`                                      | `low` / `medium` / `high` / `xhigh` / `max`.                 |
| `MAX_OUTPUT_TOKENS` | `16000`                                       | Cap on answer length.                                        |
| `ENABLE_FALLBACKS`  | `true`                                        | Server-side refusal fallback.                                |
| `USE_FILES_API`     | `true`                                        | Upload each PDF once and send its file id, instead of base64. |
| `ENABLE_CITATIONS`  | `true`                                        | Ask Claude for citations to the passages it uses.            |
| `MAX_UPLOAD_MB`     | `24`                                          | Upload size limit (see limitations).                         |
| `MAX_PAGES`         | `600`                                         | Page limit (Claude's maximum for PDFs).                      |
| `STORAGE_DIR`       | `storage`                                     | Where PDFs and `library.db` are kept, relative to `backend/`. |
| `CORS_ORIGINS`      | `http://localhost:5173,http://127.0.0.1:5173` | Allowed browser origins if you call the API directly.        |
| `HOST` / `PORT`     | `127.0.0.1` / `8000`                          | Address the backend listens on.                              |
| `CLAUDE_API_BASE_URL`| `https://api.anthropic.com`                  | Send Claude requests to a proxy or mock server instead. (Named so a shell's `ANTHROPIC_BASE_URL`, used by Claude Code, doesn't apply here.) |
| `OPENROUTER_API_KEY` | *(none)*                                     | Turns on the fallback models. Read from `backend/.env` or the shell; `INSERT KEY` counts as unset. |
| `OPENROUTER_MODELS`  | `google/gemini-4-hydrogen,openai/gpt-6.2-sol,google/gemini-3.8-flash` | Models OpenRouter tries in order. Pick ones that accept PDF input. |
| `OPENROUTER_BASE_URL`| `https://openrouter.ai/api/v1`               | Send OpenRouter requests elsewhere (tests use a mock).       |
| `RUST_LOG`          | `info`                                        | Log filter, e.g. `debug`.                                    |

## Project layout

```
backend/                  Rust crate (axum + tokio + reqwest)
  src/main.rs             entry point: load settings, bind, serve
  src/lib.rs              router, CORS, body limits, shared state
  src/config.rs           settings from env / .env
  src/storage.rs          PDFs on disk; documents + conversations in SQLite
  src/prompts.rs          system prompt and reader-context builder
  src/claude.rs           Files API, message building, HTTP call, SSE parsing,
                          citations, retries, error mapping
  src/routes.rs           health, library, conversation and chat handlers
  tests/api.rs            API tests against a local mock Claude server
frontend/
  src/App.tsx             layout, resizable/collapsible chat, doc restore
  src/components/         PdfViewer, ChatPanel, Message, Library, UploadView, …
                          (*.test.tsx: component tests)
  src/hooks/              PDF search, speech recognition, the read-aloud voice, text selection
  src/lib/                page and citation links, text matching for search and
                          highlights, Marked Passages, text-to-speech cleanup (*.test.ts)
  src/voice/              Kokoro voice in a Web Worker, and the player that speaks
                          answers chunk by chunk and reports their citations
.github/workflows/ci.yml  runs fmt, clippy, cargo test, Vitest and the frontend build
```

## Known limitations

- **Claude's PDF limits:** a request can be at most **128 MB** and a PDF at most
  **600 pages** (100 on models with smaller context windows). With the Files API
  (the default) the PDF isn't in the request body at all; files can be up to
  500 MB. When the PDF is sent inline instead, base64 adds about 33%, so the
  default 24 MB upload limit becomes about 32 MB per request. Raise
  `MAX_UPLOAD_MB` if you need larger files. Very long or image-heavy papers also use many input tokens per question,
  and only the first one within a cache window pays full price.
- **Scanned PDFs** without a text layer can't be highlighted or searched in the
  viewer, and Claude can't cite them. Claude can still read them, because it sees
  each page as an image; clicking a citation then just scrolls to its page.
- **Citation highlights** match Claude's `cited_text` against the PDF's text
  layer. If a passage was extracted differently (unusual fonts, columns), the
  first words are matched instead, or, failing that, only the page is shown.
- **Prompt cache lifetime** is about 5 minutes after the last use. After a longer
  pause, the next question re-sends the PDF at full input price.
- **Voice input** uses the Web Speech API: supported in Chrome, Edge and Safari,
  but **not Firefox**, where the mic is shown disabled with a tooltip. Chrome sends
  audio to Google's speech service, so it needs a network connection. Microphone
  access requires `localhost` or HTTPS.
- **Read-aloud voice:** the first time you use it, the Kokoro model is downloaded
  from Hugging Face (about 90 MB, or about 330 MB with WebGPU, which uses the
  full-precision weights) and then cached by the browser. Speech is generated
  sentence by sentence; without WebGPU (on WebAssembly) it can be slower than
  real time, so there may be pauses. The voice is English only. Equations are announced as
  "equation shown on screen" rather than read symbol by symbol.
- **Live highlights** highlight one passage at a time: when a spoken chunk relies on
  several citations, the first one is shown.
- **No official Rust SDK:** new API features have to be added by hand in
  `src/claude.rs`, rather than arriving with an SDK update.
- **Password-protected PDFs** are rejected at upload, including ones that only
  restrict printing or copying.
- **No hot reload** for the backend: restart `make dev` after changing Rust
  code, or install `cargo-watch` and run `cargo watch -x run` in `backend/`.
- **Storage** is a local folder with no authentication, so it's meant for
  single-user local use. Documents saved by earlier versions (`<doc_id>.json`
  files) are imported into `library.db` on startup.
- **Files API copies** are deleted when you delete a paper from the library.
  Deleting files from `STORAGE_DIR` by hand leaves the uploaded copy in your
  Anthropic workspace.
- **Fallback answers** are less tightly grounded than Claude's. Their citations
  are quotes the model writes itself, not passages the API checks against the
  PDF, so a highlight may land on the right page but not the exact words, or
  only on the page. They don't use the Files API or prompt caching, so every
  question re-sends the whole PDF (billed as input tokens by OpenRouter).
  Models, prices and PDF support on OpenRouter change often; set
  `OPENROUTER_MODELS` to choose your own.
- **Long conversations** are sent in full with every question. Very long ones
  can eventually exceed the model's context window; **Clear** starts afresh.

## Suggested next steps

- Export a conversation (Markdown with quoted citations).
- Show the PDF outline (table of contents) in the viewer.
- Search across every paper in the library.

## License

[MIT](LICENSE)

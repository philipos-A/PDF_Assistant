import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { documentFileUrl, getDocument, getHealth } from "./api";
import { ChatPanel, type SelectionRequest } from "./components/ChatPanel";
import { ChatIcon, FileIcon } from "./components/icons";
import { PassageCard } from "./components/PassageCard";
import { PdfViewer, type PdfViewerHandle } from "./components/PdfViewer";
import { UploadView } from "./components/UploadView";
import { useVoice } from "./hooks/useVoice";
import { buildPassages } from "./lib/passages";
import type { ChatMessage, Citation, DocInfo, Health } from "./types";

const DOC_KEY = "pdf-assistant:doc";
const WIDTH_KEY = "pdf-assistant:chat-width";
const MIN_CHAT = 320;

function load(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}
function save(key: string, value: string | null) {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    /* storage unavailable */
  }
}

export default function App() {
  const [health, setHealth] = useState<Health | null>(null);
  const [backendDown, setBackendDown] = useState(false);
  const [doc, setDoc] = useState<DocInfo | null>(null);
  const [restoring, setRestoring] = useState(() => !!load(DOC_KEY));
  const [currentPage, setCurrentPage] = useState(1);
  const [visiblePages, setVisiblePages] = useState<number[]>([1]);
  const [selectionRequest, setSelectionRequest] = useState<SelectionRequest | null>(null);
  const [chatOpen, setChatOpen] = useState(true);
  const [chatWidth, setChatWidth] = useState(() => Number(load(WIDTH_KEY)) || 420);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [openPassage, setOpenPassage] = useState<{ key: string; anchor: DOMRect } | null>(null);
  const viewerRef = useRef<PdfViewerHandle>(null);
  const { voice, readingId } = useVoice();

  const passages = useMemo(() => buildPassages(messages), [messages]);
  // The viewer only needs a new list when passages are added or change kind, not on every
  // streamed word; redrawing its highlights is the expensive part.
  const passageSig = passages.map((p) => `${p.key}|${p.kind}`).join("\n");
  const viewerPassages = useMemo(() => passages, [passageSig]);
  const shownPassage = openPassage && passages.find((p) => p.key === openPassage.key);

  useEffect(() => {
    getHealth()
      .then(setHealth)
      .catch(() => setBackendDown(true));
    // Reopen the last document after a refresh.
    const saved = load(DOC_KEY);
    if (saved) {
      getDocument(saved)
        .then((d) => {
          if (d) setDoc(d);
          else save(DOC_KEY, null);
        })
        .catch(() => undefined)
        .finally(() => setRestoring(false));
    }
  }, []);

  const openDoc = (d: DocInfo | null) => {
    setDoc(d);
    save(DOC_KEY, d?.doc_id ?? null);
    setCurrentPage(1);
    setVisiblePages([1]);
    setSelectionRequest(null);
    setMessages([]);
    setOpenPassage(null);
    voice.cancel();
  };

  const onPageChange = useCallback((current: number, visible: number[]) => {
    setCurrentPage(current);
    setVisiblePages(visible);
  }, []);

  const goToPage = useCallback((page: number) => viewerRef.current?.scrollToPage(page), []);
  const showCitation = useCallback((c: Citation) => viewerRef.current?.showCitation(c), []);
  const onPassageClick = useCallback((key: string, anchor: DOMRect) => setOpenPassage({ key, anchor }), []);
  const closePassage = useCallback(() => setOpenPassage(null), []);

  const speakMessage = useCallback(
    (m: ChatMessage) =>
      voice.speak(m.id, m.content, (segment) => {
        const c = m.citations?.find((x) => x.n === segment.cites[0]);
        if (c) showCitation(c);
      }),
    [voice, showCitation],
  );

  /** Open the chat and bring an Answer into view with a brief flash. */
  const showInChat = useCallback((answerId: string) => {
    setOpenPassage(null);
    setChatOpen(true);
    // If the chat was closed it reloads the Conversation first, so wait for the Answer to appear.
    const started = performance.now();
    requestAnimationFrame(function find() {
      const el = document.getElementById(`message-${answerId}`);
      if (!el) {
        if (performance.now() - started < 2000) requestAnimationFrame(find);
        return;
      }
      el.scrollIntoView({ behavior: "smooth", block: "start" });
      el.classList.remove("message-flash");
      void el.offsetWidth; // restart the animation
      el.classList.add("message-flash");
    });
  }, []);

  const onAskAboutSelection = useCallback((text: string, page: number | null) => {
    setChatOpen(true);
    setSelectionRequest({ text, page, nonce: Date.now() });
  }, []);

  const startResize = (e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const onMove = (ev: PointerEvent) => {
      const max = Math.max(MIN_CHAT, window.innerWidth * 0.65);
      setChatWidth(Math.round(Math.min(max, Math.max(MIN_CHAT, window.innerWidth - ev.clientX))));
    };
    const onUp = () => {
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      setChatWidth((w) => {
        save(WIDTH_KEY, String(w));
        return w;
      });
    };
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
  };

  const banner = backendDown ? (
    <div className="bg-red-600 px-4 py-2 text-center text-sm text-white">
      Can't reach the backend on port 8000. Start it with <code className="font-mono">make dev</code>.
    </div>
  ) : health && !health.api_key_configured ? (
    <div className="bg-amber-500 px-4 py-2 text-center text-sm text-amber-950">
      ANTHROPIC_API_KEY is not set. Add it to <code className="font-mono">backend/.env</code> and restart the backend to
      ask questions.
    </div>
  ) : null;

  if (!doc) {
    return (
      <div className="flex h-full flex-col">
        {banner}
        <main className="min-h-0 flex-1 overflow-auto">
          {restoring ? null : <UploadView health={health} onOpen={openDoc} />}
        </main>
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      {banner}
      <header className="flex h-12 shrink-0 items-center justify-between gap-3 border-b border-slate-200 bg-white px-4 dark:border-slate-800 dark:bg-slate-900">
        <div className="flex min-w-0 items-center gap-2.5">
          <div className="grid h-7 w-7 shrink-0 place-items-center rounded-lg bg-indigo-600 text-white">
            <FileIcon width={15} height={15} />
          </div>
          <span className="truncate text-sm font-medium" title={doc.filename}>
            {doc.filename}
          </span>
          <span className="shrink-0 text-xs text-slate-500">{doc.page_count} pages</span>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {!chatOpen && (
            <button
              type="button"
              onClick={() => setChatOpen(true)}
              className="flex items-center gap-1.5 rounded-md bg-indigo-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-indigo-500"
            >
              <ChatIcon width={16} height={16} /> Chat
            </button>
          )}
          <button
            type="button"
            onClick={() => openDoc(null)}
            className="rounded-md px-3 py-1.5 text-sm text-slate-600 hover:bg-slate-100 dark:text-slate-300 dark:hover:bg-slate-800"
          >
            Library
          </button>
        </div>
      </header>

      <main className="flex min-h-0 flex-1 flex-col md:flex-row">
        <section className="min-h-0 min-w-0 flex-1">
          <PdfViewer
            key={doc.doc_id}
            ref={viewerRef}
            fileUrl={documentFileUrl(doc.doc_id)}
            onPageChange={onPageChange}
            onAskAboutSelection={onAskAboutSelection}
            passages={viewerPassages}
            onPassageClick={onPassageClick}
          />
        </section>
        {chatOpen && (
          <>
            <div
              role="separator"
              aria-orientation="vertical"
              aria-label="Resize chat panel"
              onPointerDown={startResize}
              className="hidden w-1.5 shrink-0 cursor-col-resize bg-slate-200 transition hover:bg-indigo-400 active:bg-indigo-500 md:block dark:bg-slate-800"
            />
            <aside
              className="h-[50%] min-h-0 shrink-0 border-t border-slate-200 md:h-auto md:w-[var(--chat-w)] md:border-t-0 dark:border-slate-800"
              style={{ "--chat-w": `${chatWidth}px` } as React.CSSProperties}
            >
              <ChatPanel
                key={doc.doc_id}
                doc={doc}
                currentPage={currentPage}
                visiblePages={visiblePages}
                selectionRequest={selectionRequest}
                onPageClick={goToPage}
                onCitationClick={showCitation}
                onLiveCitation={showCitation}
                onMessagesChange={setMessages}
                onCollapse={() => setChatOpen(false)}
              />
            </aside>
          </>
        )}
      </main>
      {shownPassage && openPassage && (
        <PassageCard
          key={shownPassage.key}
          passage={shownPassage}
          anchor={openPassage.anchor}
          pageCount={doc.page_count}
          readingId={readingId}
          onClose={closePassage}
          onPageClick={goToPage}
          onCitationClick={showCitation}
          onSpeak={voice.supported ? speakMessage : undefined}
          onShowInChat={showInChat}
        />
      )}
    </div>
  );
}

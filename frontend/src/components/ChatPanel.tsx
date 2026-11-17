import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { streamChat } from "../api";
import { useSpeechRecognition } from "../hooks/useSpeechRecognition";
import { useSpeechSynthesis } from "../hooks/useSpeechSynthesis";
import type { ChatMessage, DocInfo, HistoryTurn } from "../types";
import { CloseIcon, SendIcon, SpeakerIcon, SpeakerOffIcon, StopIcon } from "./icons";
import { Message } from "./Message";
import { MicButton } from "./MicButton";

export interface SelectionRequest {
  text: string;
  page: number | null;
  /** Changes on every "Ask about this" click, so the same text can be re-asked. */
  nonce: number;
}

interface Props {
  doc: DocInfo;
  currentPage: number;
  visiblePages: number[];
  selectionRequest: SelectionRequest | null;
  onPageClick: (page: number) => void;
  onCollapse: () => void;
}

const SUGGESTIONS = [
  "What is this paper's main contribution?",
  "Explain the key terms on this page.",
  "Walk me through the main equation on this page.",
];

const uid = () => Math.random().toString(36).slice(2);

function toHistory(messages: ChatMessage[]): HistoryTurn[] {
  return messages
    .filter((m) => m.status === "done" && m.content.trim())
    .map((m) =>
      m.role === "user"
        ? {
            role: "user",
            content: m.content,
            current_page: m.page,
            visible_pages: m.visiblePages,
            selected_text: m.selectedText,
          }
        : { role: "assistant", content: m.content },
    );
}

export function ChatPanel({ doc, currentPage, visiblePages, selectionRequest, onPageClick, onCollapse }: Props) {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState("");
  const [attached, setAttached] = useState<{ text: string; page: number | null } | null>(null);
  const [readAloud, setReadAloud] = useState(() => {
    try {
      return localStorage.getItem("pdf-assistant:read-aloud") === "1";
    } catch {
      return false;
    }
  });

  const abortRef = useRef<AbortController | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const stickToBottom = useRef(true);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const tts = useSpeechSynthesis();

  const busy = messages.some((m) => m.role === "assistant" && ["pending", "thinking", "streaming"].includes(m.status));

  const update = (id: string, patch: Partial<ChatMessage> | ((m: ChatMessage) => Partial<ChatMessage>)) =>
    setMessages((ms) => ms.map((m) => (m.id === id ? { ...m, ...(typeof patch === "function" ? patch(m) : patch) } : m)));

  const send = useCallback(
    async (raw: string) => {
      if (busy) return;
      let question = raw.trim();
      if (!question && attached) question = "Explain this passage in plain language.";
      if (!question) return;

      const selected = attached?.text;
      const userMsg: ChatMessage = {
        id: uid(),
        role: "user",
        content: question,
        page: currentPage,
        visiblePages,
        selectedText: selected,
        status: "done",
      };
      const assistantId = uid();
      const assistantMsg: ChatMessage = {
        id: assistantId,
        role: "assistant",
        content: "",
        page: currentPage,
        status: "pending",
      };
      const history = toHistory(messages);
      setMessages((ms) => [...ms, userMsg, assistantMsg]);
      setInput("");
      setAttached(null);
      stickToBottom.current = true;
      tts.cancel();

      const controller = new AbortController();
      abortRef.current = controller;
      let answer = "";
      await streamChat(
        {
          doc_id: doc.doc_id,
          question,
          current_page: currentPage,
          visible_pages: visiblePages,
          selected_text: selected,
          history,
        },
        {
          onStatus: (state) => {
            if (state === "thinking") update(assistantId, (m) => (m.content ? {} : { status: "thinking" }));
            if (state === "fallback") update(assistantId, { usedFallback: true });
          },
          onDelta: (text) => {
            answer += text;
            update(assistantId, { content: answer, status: "streaming" });
          },
          onDone: () => {
            update(assistantId, { status: "done" });
            if (readAloud && answer) tts.speak(answer);
          },
          onError: (message) => update(assistantId, { status: "error", error: message }),
        },
        controller.signal,
      );
      if (controller.signal.aborted) {
        update(assistantId, (m) => ({ status: m.content ? "done" : "error", error: m.content ? undefined : "Stopped." }));
      }
      abortRef.current = null;
    },
    [busy, attached, currentPage, visiblePages, messages, doc.doc_id, readAloud],
  );

  const speech = useSpeechRecognition((transcript) => {
    const combined = `${input} ${transcript}`.trim();
    void send(combined);
  });

  // "Ask about this" from the PDF: attach the selection and focus the input.
  useEffect(() => {
    if (!selectionRequest) return;
    setAttached({ text: selectionRequest.text, page: selectionRequest.page });
    inputRef.current?.focus();
  }, [selectionRequest]);

  useEffect(() => () => abortRef.current?.abort(), []);

  // Auto-scroll while streaming, unless the reader scrolled up.
  useLayoutEffect(() => {
    const el = listRef.current;
    if (el && stickToBottom.current) el.scrollTop = el.scrollHeight;
  }, [messages]);

  // Auto-grow the textarea.
  useLayoutEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 160)}px`;
  }, [input, speech.transcript]);

  const toggleReadAloud = () => {
    const next = !readAloud;
    setReadAloud(next);
    if (!next) tts.cancel();
    try {
      localStorage.setItem("pdf-assistant:read-aloud", next ? "1" : "0");
    } catch {
      /* storage unavailable */
    }
  };

  const shownInput = speech.listening ? `${input} ${speech.transcript}`.trimStart() : input;

  return (
    <div className="flex h-full min-h-0 flex-col bg-slate-50 dark:bg-slate-950">
      <div className="flex h-11 shrink-0 items-center justify-between border-b border-slate-200 bg-white px-3 dark:border-slate-800 dark:bg-slate-900">
        <div className="text-sm font-semibold">Ask about the paper</div>
        <div className="flex items-center gap-1">
          {tts.supported && (
            <button
              type="button"
              onClick={toggleReadAloud}
              aria-pressed={readAloud}
              title={readAloud ? "Stop reading answers aloud" : "Read answers aloud"}
              className={`flex h-8 items-center gap-1.5 rounded-md px-2 text-xs font-medium transition ${
                readAloud
                  ? "bg-indigo-50 text-indigo-700 dark:bg-indigo-950 dark:text-indigo-300"
                  : "text-slate-500 hover:bg-slate-100 dark:hover:bg-slate-800"
              }`}
            >
              {readAloud ? <SpeakerIcon width={16} height={16} /> : <SpeakerOffIcon width={16} height={16} />}
              Voice
            </button>
          )}
          {tts.speaking && (
            <button
              type="button"
              onClick={tts.cancel}
              className="h-8 rounded-md px-2 text-xs text-slate-500 hover:bg-slate-100 dark:hover:bg-slate-800"
            >
              Stop reading
            </button>
          )}
          <button
            type="button"
            onClick={onCollapse}
            title="Hide chat"
            className="grid h-8 w-8 place-items-center rounded-md text-slate-500 hover:bg-slate-100 dark:hover:bg-slate-800"
          >
            <CloseIcon width={16} height={16} />
          </button>
        </div>
      </div>

      <div
        ref={listRef}
        onScroll={(e) => {
          const el = e.currentTarget;
          stickToBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
        }}
        className="min-h-0 flex-1 space-y-5 overflow-y-auto px-4 py-4"
      >
        {messages.length === 0 && (
          <div className="mt-6 space-y-4 text-center">
            <p className="text-sm text-slate-500">
              Ask anything about <span className="font-medium text-slate-700 dark:text-slate-300">{doc.filename}</span>.
              Highlight text in the paper to ask about it, or hold the mic to speak.
            </p>
            <div className="flex flex-col items-center gap-2">
              {SUGGESTIONS.map((s) => (
                <button
                  key={s}
                  type="button"
                  onClick={() => void send(s)}
                  className="rounded-full border border-slate-200 bg-white px-3 py-1.5 text-sm text-slate-600 transition hover:border-indigo-300 hover:text-indigo-700 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300 dark:hover:border-indigo-700"
                >
                  {s}
                </button>
              ))}
            </div>
          </div>
        )}
        {messages.map((m) => (
          <Message
            key={m.id}
            message={m}
            pageCount={doc.page_count}
            onPageClick={onPageClick}
            onSpeak={tts.supported ? tts.speak : undefined}
          />
        ))}
      </div>

      <div className="shrink-0 border-t border-slate-200 bg-white p-3 dark:border-slate-800 dark:bg-slate-900">
        {attached && (
          <div className="mb-2 flex items-start gap-2 rounded-lg bg-indigo-50 px-3 py-2 text-xs text-indigo-900 dark:bg-indigo-950/60 dark:text-indigo-200">
            <div className="min-w-0 flex-1">
              <div className="mb-0.5 font-semibold">
                Selected text{attached.page ? ` · p. ${attached.page}` : ""}
              </div>
              <div className="line-clamp-3 italic">“{attached.text}”</div>
            </div>
            <button
              type="button"
              onClick={() => setAttached(null)}
              title="Remove selection"
              className="shrink-0 rounded p-0.5 hover:bg-indigo-100 dark:hover:bg-indigo-900"
            >
              <CloseIcon width={14} height={14} />
            </button>
          </div>
        )}
        {speech.listening && (
          <div className="mb-2 flex items-center gap-2 text-xs font-medium text-red-600 dark:text-red-400">
            <span className="h-2 w-2 animate-pulse rounded-full bg-red-500" />
            Listening… release to send
          </div>
        )}
        {speech.error && <div className="mb-2 text-xs text-red-600 dark:text-red-400">{speech.error}</div>}
        <form
          className="flex items-end gap-1.5 rounded-xl border border-slate-300 bg-white p-1.5 focus-within:border-indigo-500 focus-within:ring-2 focus-within:ring-indigo-500/20 dark:border-slate-700 dark:bg-slate-950"
          onSubmit={(e) => {
            e.preventDefault();
            void send(input);
          }}
        >
          <textarea
            ref={inputRef}
            rows={1}
            value={shownInput}
            readOnly={speech.listening}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                void send(input);
              }
            }}
            placeholder={attached ? "Ask about the selected text…" : `Ask about page ${currentPage}, or anything in the paper…`}
            className="max-h-40 min-h-[2.5rem] flex-1 resize-none bg-transparent px-2 py-2 text-sm placeholder:text-slate-400 focus:outline-none"
          />
          <MicButton
            supported={speech.supported}
            listening={speech.listening}
            disabled={busy}
            onStart={speech.start}
            onStop={speech.stop}
          />
          {busy ? (
            <button
              type="button"
              onClick={() => abortRef.current?.abort()}
              title="Stop"
              className="grid h-10 w-10 shrink-0 place-items-center rounded-full bg-slate-800 text-white transition hover:bg-slate-700 dark:bg-slate-200 dark:text-slate-900"
            >
              <StopIcon width={16} height={16} />
            </button>
          ) : (
            <button
              type="submit"
              disabled={!input.trim() && !attached}
              title="Send (Enter)"
              className="grid h-10 w-10 shrink-0 place-items-center rounded-full bg-indigo-600 text-white transition hover:bg-indigo-500 disabled:bg-slate-200 disabled:text-slate-400 dark:disabled:bg-slate-800"
            >
              <SendIcon />
            </button>
          )}
        </form>
        <p className="mt-1.5 px-1 text-[11px] text-slate-400">
          Answers know you're on page {currentPage}. Enter to send, Shift+Enter for a new line.
        </p>
      </div>
    </div>
  );
}

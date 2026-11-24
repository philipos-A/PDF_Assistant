import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { MarkedPassage } from "../lib/passages";
import type { ChatMessage, Citation } from "../types";
import { ChatIcon, CloseIcon, SpeakerIcon } from "./icons";
import { Message } from "./Message";

interface Props {
  passage: MarkedPassage;
  /** Screen position of the clicked highlight. */
  anchor: DOMRect;
  pageCount: number;
  readingId: string | null;
  onClose: () => void;
  onPageClick: (page: number) => void;
  onCitationClick: (citation: Citation) => void;
  onSpeak?: (message: ChatMessage) => void;
  onShowInChat: (answerId: string) => void;
}

const MARGIN = 8;
const GAP = 10;

/** Card next to a clicked Marked Passage, listing the saved Answers about it (newest first). */
export function PassageCard({
  passage,
  anchor,
  pageCount,
  readingId,
  onClose,
  onPageClick,
  onCitationClick,
  onSpeak,
  onShowInChat,
}: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [pos, setPos] = useState<{ left: number; top: number; maxHeight: number; above: boolean } | null>(null);

  // Below the highlight if it fits, otherwise above it; always on screen.
  useLayoutEffect(() => {
    const width = Math.min(420, window.innerWidth - 2 * MARGIN);
    const left = Math.min(
      Math.max(MARGIN, anchor.left + anchor.width / 2 - width / 2),
      window.innerWidth - width - MARGIN,
    );
    const below = window.innerHeight - anchor.bottom - GAP - MARGIN;
    const above = anchor.top - GAP - MARGIN;
    const showAbove = below < 260 && above > below;
    const maxHeight = Math.min(520, showAbove ? above : below);
    setPos({ left, top: showAbove ? anchor.top - GAP : anchor.bottom + GAP, maxHeight, above: showAbove });
  }, [anchor]);

  useEffect(() => {
    ref.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      // Capture phase + preventDefault: runs before the viewer's Escape handler, which then keeps its highlight.
      e.preventDefault();
      onClose();
    };
    const onDown = (e: PointerEvent) => {
      const target = e.target as Element;
      // Clicking another highlight opens that one instead (handled by the viewer).
      if (!ref.current?.contains(target) && !target.closest?.("mark[data-passage]")) onClose();
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("pointerdown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("pointerdown", onDown);
    };
  }, [onClose]);

  const answers = [...passage.answers].reverse();
  const asked = passage.kind === "asked";
  const pages =
    passage.endPage > passage.startPage ? `pp. ${passage.startPage}–${passage.endPage}` : `p. ${passage.startPage}`;

  return (
    <div
      ref={ref}
      role="dialog"
      aria-label={`Saved answers about the passage on ${pages}`}
      tabIndex={-1}
      style={
        pos
          ? {
              left: pos.left,
              top: pos.top,
              maxHeight: pos.maxHeight,
              width: Math.min(420, window.innerWidth - 2 * MARGIN),
              transform: pos.above ? "translateY(-100%)" : undefined,
            }
          : { visibility: "hidden" }
      }
      className="fixed z-50 flex flex-col overflow-hidden rounded-2xl bg-white shadow-2xl shadow-slate-900/20 ring-1 ring-slate-200 focus:outline-none dark:bg-slate-900 dark:ring-slate-700"
    >
      <div className={`popover-enter flex min-h-0 flex-col ${pos?.above ? "origin-bottom" : "origin-top"}`}>
        <div className="flex items-start gap-3 border-b border-slate-100 px-4 pb-3 pt-3.5 dark:border-slate-800">
          <div className="min-w-0 flex-1">
            <div className="mb-1.5 flex items-center gap-2 text-[11px] font-semibold uppercase tracking-wide">
              <span
                className={`rounded-full px-2 py-0.5 ${
                  asked
                    ? "bg-indigo-100 text-indigo-700 dark:bg-indigo-950 dark:text-indigo-300"
                    : "bg-amber-100 text-amber-800 dark:bg-amber-950 dark:text-amber-300"
                }`}
              >
                {asked ? "You asked about this" : "Cited"}
              </span>
              <span className="text-slate-400">
                {pages} · {answers.length} {answers.length === 1 ? "answer" : "answers"}
              </span>
            </div>
            <blockquote
              className={`border-l-2 pl-2.5 text-[13px] italic leading-snug text-slate-600 line-clamp-3 dark:text-slate-300 ${
                asked ? "border-indigo-400" : "border-amber-400"
              }`}
            >
              “{passage.text}”
            </blockquote>
          </div>
          <button
            type="button"
            onClick={onClose}
            title="Close (Esc)"
            className="-mr-1.5 -mt-1 grid h-9 w-9 shrink-0 place-items-center rounded-lg text-slate-400 transition hover:bg-slate-100 hover:text-slate-700 active:scale-95 dark:hover:bg-slate-800 dark:hover:text-slate-200"
          >
            <CloseIcon width={16} height={16} />
          </button>
        </div>

        <ol className="min-h-0 flex-1 space-y-4 overflow-y-auto overscroll-contain bg-slate-50/70 px-4 py-3.5 dark:bg-slate-950/40">
          {answers.map(({ question, answer }) => (
            <li key={answer.id} className="space-y-2">
              <div className="flex items-start justify-between gap-2">
                <p className="text-[13px] font-medium leading-snug text-slate-800 dark:text-slate-100">
                  {question.content}
                </p>
                <div className="flex shrink-0 items-center">
                  {onSpeak && readingId !== answer.id && (
                    <button
                      type="button"
                      onClick={() => onSpeak(answer)}
                      className="flex shrink-0 items-center gap-1 rounded-md px-1.5 py-1 text-[11px] font-medium text-slate-500 transition hover:bg-slate-100 hover:text-indigo-600 active:scale-95 dark:text-slate-400 dark:hover:bg-slate-800"
                      title="Read this answer aloud"
                    >
                      <SpeakerIcon width={13} height={13} /> Listen
                    </button>
                  )}
                  <button
                    type="button"
                    onClick={() => onShowInChat(answer.id)}
                    className="flex shrink-0 items-center gap-1 rounded-md px-1.5 py-1 text-[11px] font-medium text-indigo-600 transition hover:bg-indigo-50 active:scale-95 dark:text-indigo-400 dark:hover:bg-indigo-950"
                    title="Show this answer in the chat"
                  >
                    <ChatIcon width={13} height={13} /> In chat
                  </button>
                </div>
              </div>
              <div className={`relative ${expanded.has(answer.id) ? "" : "max-h-64 overflow-hidden"}`}>
                <Message
                  message={answer}
                  pageCount={pageCount}
                  onPageClick={onPageClick}
                  onCitationClick={onCitationClick}
                  reading={readingId === answer.id}
                  compact
                />
                {!expanded.has(answer.id) && answer.content.length > 500 && (
                  <div className="absolute inset-x-0 bottom-0 flex h-20 items-end justify-center bg-gradient-to-t from-slate-50 via-slate-50/90 to-transparent pb-2 dark:from-slate-950 dark:via-slate-950/90">
                    <button
                      type="button"
                      onClick={() => setExpanded((s) => new Set(s).add(answer.id))}
                      className="rounded-full bg-white px-3 py-1 text-xs font-medium text-indigo-600 shadow-sm ring-1 ring-slate-200 transition hover:bg-indigo-50 active:scale-95 dark:bg-slate-900 dark:text-indigo-400 dark:ring-slate-700 dark:hover:bg-slate-800"
                    >
                      Show full answer
                    </button>
                  </div>
                )}
              </div>
            </li>
          ))}
        </ol>
      </div>
    </div>
  );
}

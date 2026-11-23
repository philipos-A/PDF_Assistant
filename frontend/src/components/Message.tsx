import { memo, useMemo } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import rehypeKatex from "rehype-katex";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import { citationPages, parseCiteHref } from "../lib/citations";
import { linkifyPageRefs, parsePageHref } from "../lib/pageRefs";
import type { ChatMessage, Citation } from "../types";
import { SpeakerIcon } from "./icons";

interface Props {
  message: ChatMessage;
  pageCount: number;
  onPageClick: (page: number) => void;
  /** Show a cited passage in the PDF. */
  onCitationClick: (citation: Citation) => void;
  onSpeak?: (text: string) => void;
}

function CitationChip({ citation, onClick }: { citation: Citation; onClick: (c: Citation) => void }) {
  return (
    <button
      type="button"
      onClick={() => onClick(citation)}
      title={`${citationPages(citation)}: “${citation.cited_text}”`}
      aria-label={`Source ${citation.n}, ${citationPages(citation)}`}
      className="mx-0.5 inline-grid h-4 min-w-4 -translate-y-1 place-items-center rounded bg-amber-100 px-1 align-baseline text-[10px] font-semibold leading-none text-amber-800 no-underline transition hover:bg-amber-200 dark:bg-amber-900/60 dark:text-amber-200 dark:hover:bg-amber-800"
    >
      {citation.n}
    </button>
  );
}

function CitationList({ citations, onClick }: { citations: Citation[]; onClick: (c: Citation) => void }) {
  return (
    <div className="mt-3 border-t border-slate-100 pt-2 dark:border-slate-800">
      <div className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-slate-400">Sources</div>
      <ol className="space-y-1">
        {citations.map((c) => (
          <li key={c.n}>
            <button
              type="button"
              onClick={() => onClick(c)}
              className="flex w-full gap-2 rounded-md px-1.5 py-1 text-left text-xs text-slate-600 transition hover:bg-amber-50 dark:text-slate-300 dark:hover:bg-amber-950/40"
            >
              <span className="shrink-0 font-semibold text-amber-700 dark:text-amber-300">
                {c.n} · {citationPages(c)}
              </span>
              <span className="line-clamp-2 italic">“{c.cited_text}”</span>
            </button>
          </li>
        ))}
      </ol>
    </div>
  );
}

function PageBadge({ page, onClick }: { page: number; onClick: (p: number) => void }) {
  return (
    <button
      type="button"
      onClick={() => onClick(page)}
      title={`Go to page ${page}`}
      className="rounded-full bg-slate-200/70 px-2 py-0.5 text-[11px] font-medium text-slate-600 transition hover:bg-indigo-100 hover:text-indigo-700 dark:bg-slate-800 dark:text-slate-400 dark:hover:bg-indigo-950 dark:hover:text-indigo-300"
    >
      Asked from p. {page}
    </button>
  );
}

export const Message = memo(function Message({ message, pageCount, onPageClick, onCitationClick, onSpeak }: Props) {
  const citations = message.citations;
  const components = useMemo<Components>(
    () => ({
      a: ({ href, children }) => {
        const n = parseCiteHref(href);
        if (n !== null) {
          const citation = citations?.find((c) => c.n === n);
          // The backend sends each citation before its first marker; drop a marker with none.
          return citation ? <CitationChip citation={citation} onClick={onCitationClick} /> : null;
        }
        const page = parsePageHref(href);
        if (page !== null) {
          return (
            <button
              type="button"
              onClick={() => onPageClick(page)}
              className="rounded px-0.5 font-medium text-indigo-600 underline decoration-indigo-300 underline-offset-2 hover:bg-indigo-50 dark:text-indigo-400 dark:decoration-indigo-700 dark:hover:bg-indigo-950"
            >
              {children}
            </button>
          );
        }
        return (
          <a href={href} target="_blank" rel="noreferrer noopener">
            {children}
          </a>
        );
      },
    }),
    [onPageClick, onCitationClick, citations],
  );

  if (message.role === "user") {
    return (
      <div className="flex flex-col items-end gap-1.5">
        {message.context?.selected_text && (
          <blockquote className="max-w-[90%] border-l-2 border-indigo-400 pl-2 text-xs italic text-slate-500 line-clamp-3 dark:text-slate-400">
            “{message.context.selected_text}”
          </blockquote>
        )}
        <div className="max-w-[90%] whitespace-pre-wrap rounded-2xl rounded-br-md bg-indigo-600 px-3.5 py-2 text-sm text-white">
          {message.content}
        </div>
        <PageBadge page={message.page} onClick={onPageClick} />
      </div>
    );
  }

  const waiting = message.status === "pending" || message.status === "thinking";
  const markdown = linkifyPageRefs(message.content, pageCount);

  return (
    <div className="group flex flex-col items-start gap-1.5">
      <div className="w-full rounded-2xl rounded-bl-md bg-white px-4 py-3 shadow-sm ring-1 ring-slate-200 dark:bg-slate-900 dark:ring-slate-800">
        {waiting && !message.content ? (
          <div className="flex items-center gap-2 text-sm text-slate-500">
            <span className="flex gap-1">
              {[0, 150, 300].map((d) => (
                <span
                  key={d}
                  className="h-1.5 w-1.5 animate-bounce rounded-full bg-indigo-400"
                  style={{ animationDelay: `${d}ms` }}
                />
              ))}
            </span>
            {message.status === "thinking" ? "Thinking…" : "Reading the paper…"}
          </div>
        ) : (
          <div className="prose prose-sm prose-slate max-w-none dark:prose-invert prose-pre:bg-slate-900 prose-pre:text-slate-100 prose-code:before:content-none prose-code:after:content-none">
            <ReactMarkdown remarkPlugins={[remarkGfm, remarkMath]} rehypePlugins={[rehypeKatex]} components={components}>
              {markdown}
            </ReactMarkdown>
            {message.status === "streaming" && (
              <span className="ml-0.5 inline-block h-4 w-1.5 translate-y-0.5 animate-pulse rounded-sm bg-indigo-400" />
            )}
          </div>
        )}
        {message.status === "done" && citations && citations.length > 0 && (
          <CitationList citations={citations} onClick={onCitationClick} />
        )}
        {message.status === "error" && (
          <p className="mt-2 rounded-md bg-red-50 px-3 py-2 text-sm text-red-700 dark:bg-red-950/50 dark:text-red-300">
            {message.error}
          </p>
        )}
      </div>
      <div className="flex items-center gap-2 pl-1 text-[11px] text-slate-500">
        <PageBadge page={message.page} onClick={onPageClick} />
        {message.usedFallback && <span title="Answered by a fallback model">fallback model</span>}
        {onSpeak && message.status === "done" && message.content && (
          <button
            type="button"
            onClick={() => onSpeak(message.content)}
            className="flex items-center gap-1 opacity-0 transition hover:text-indigo-600 group-hover:opacity-100 focus:opacity-100"
            title="Read this answer aloud"
          >
            <SpeakerIcon width={13} height={13} /> Read aloud
          </button>
        )}
      </div>
    </div>
  );
});

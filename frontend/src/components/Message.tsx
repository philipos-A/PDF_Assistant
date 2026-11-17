import { memo, useMemo } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import rehypeKatex from "rehype-katex";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import { linkifyPageRefs, parsePageHref } from "../lib/pageRefs";
import type { ChatMessage } from "../types";
import { SpeakerIcon } from "./icons";

interface Props {
  message: ChatMessage;
  pageCount: number;
  onPageClick: (page: number) => void;
  onSpeak?: (text: string) => void;
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

export const Message = memo(function Message({ message, pageCount, onPageClick, onSpeak }: Props) {
  const components = useMemo<Components>(
    () => ({
      a: ({ href, children }) => {
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
    [onPageClick],
  );

  if (message.role === "user") {
    return (
      <div className="flex flex-col items-end gap-1.5">
        {message.selectedText && (
          <blockquote className="max-w-[90%] border-l-2 border-indigo-400 pl-2 text-xs italic text-slate-500 line-clamp-3 dark:text-slate-400">
            “{message.selectedText}”
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

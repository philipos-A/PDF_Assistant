import { useEffect, useState } from "react";
import { deleteDocument, listDocuments } from "../api";
import type { DocInfo } from "../types";
import { FileIcon, TrashIcon } from "./icons";

interface Props {
  onOpen: (doc: DocInfo) => void;
}

function ago(iso: string, now = Date.now()): string {
  const s = Math.max(0, (now - Date.parse(iso)) / 1000);
  if (Number.isNaN(s)) return "";
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  if (s < 86400 * 30) return `${Math.floor(s / 86400)} d ago`;
  return new Date(iso).toLocaleDateString();
}

/** Documents uploaded earlier, most recently active first. */
export function Library({ onOpen }: Props) {
  const [docs, setDocs] = useState<DocInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<string | null>(null);

  useEffect(() => {
    listDocuments()
      .then(setDocs)
      .catch((e: Error) => setError(e.message));
  }, []);

  const remove = async (doc: DocInfo) => {
    setConfirming(null);
    try {
      await deleteDocument(doc.doc_id);
      setDocs((ds) => ds?.filter((d) => d.doc_id !== doc.doc_id) ?? null);
    } catch (e) {
      setError((e as Error).message);
    }
  };

  if (error) return <p className="mt-8 text-center text-sm text-red-600 dark:text-red-400">{error}</p>;
  if (!docs?.length) return null;

  return (
    <section className="mt-10" aria-labelledby="library-heading">
      <h2 id="library-heading" className="mb-2 px-1 text-sm font-semibold text-slate-500">
        Your papers
      </h2>
      <ul className="divide-y divide-slate-200 overflow-hidden rounded-xl border border-slate-200 bg-white dark:divide-slate-800 dark:border-slate-800 dark:bg-slate-900">
        {docs.map((d) => (
          <li key={d.doc_id} className="flex items-center gap-2 pr-2">
            <button
              type="button"
              onClick={() => onOpen(d)}
              className="flex min-w-0 flex-1 items-center gap-3 px-4 py-3 text-left transition hover:bg-slate-50 dark:hover:bg-slate-800/60"
            >
              <FileIcon width={18} height={18} className="shrink-0 text-indigo-500" />
              <span className="min-w-0 flex-1">
                <span className="block truncate text-sm font-medium">{d.filename}</span>
                <span className="block text-xs text-slate-500">
                  {d.page_count} pages · {d.question_count} {d.question_count === 1 ? "question" : "questions"} ·{" "}
                  {ago(d.last_active_at)}
                </span>
              </span>
            </button>
            {confirming === d.doc_id ? (
              <span className="flex shrink-0 items-center gap-1 text-xs">
                <button
                  type="button"
                  onClick={() => void remove(d)}
                  className="rounded-md bg-red-600 px-2 py-1.5 font-medium text-white hover:bg-red-500"
                >
                  Delete
                </button>
                <button
                  type="button"
                  onClick={() => setConfirming(null)}
                  className="rounded-md px-2 py-1.5 text-slate-500 hover:bg-slate-100 dark:hover:bg-slate-800"
                >
                  Cancel
                </button>
              </span>
            ) : (
              <button
                type="button"
                onClick={() => setConfirming(d.doc_id)}
                title={`Delete ${d.filename} and its questions`}
                aria-label={`Delete ${d.filename}`}
                className="grid h-8 w-8 shrink-0 place-items-center rounded-md text-slate-400 transition hover:bg-red-50 hover:text-red-600 dark:hover:bg-red-950/50"
              >
                <TrashIcon width={16} height={16} />
              </button>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}

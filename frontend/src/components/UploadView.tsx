import { useRef, useState } from "react";
import { uploadDocument } from "../api";
import type { DocInfo, Health } from "../types";
import { FileIcon, UploadIcon } from "./icons";

interface Props {
  health: Health | null;
  onUploaded: (doc: DocInfo) => void;
}

export function UploadView({ health, onUploaded }: Props) {
  const inputRef = useRef<HTMLInputElement>(null);
  const [dragging, setDragging] = useState(false);
  const [uploading, setUploading] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const maxMb = health?.max_upload_mb ?? 24;

  const handleFile = async (file: File | undefined) => {
    if (!file) return;
    setError(null);
    if (file.type !== "application/pdf" && !file.name.toLowerCase().endsWith(".pdf")) {
      setError("That file isn't a PDF.");
      return;
    }
    if (file.size > maxMb * 1024 * 1024) {
      setError(`That PDF is ${(file.size / 1024 / 1024).toFixed(1)} MB; the limit is ${maxMb} MB.`);
      return;
    }
    setUploading(file.name);
    try {
      onUploaded(await uploadDocument(file));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setUploading(null);
    }
  };

  return (
    <div className="flex min-h-full items-center justify-center p-6">
      <div className="w-full max-w-xl">
        <div className="mb-8 text-center">
          <div className="mx-auto mb-4 grid h-12 w-12 place-items-center rounded-xl bg-indigo-600 text-white shadow-lg shadow-indigo-600/20">
            <FileIcon width={24} height={24} />
          </div>
          <h1 className="text-2xl font-semibold tracking-tight">PDF Assistant</h1>
          <p className="mt-2 text-slate-500 dark:text-slate-400">
            Open a paper and ask about terms, equations and figures as you read, by typing or speaking.
          </p>
        </div>

        <button
          type="button"
          disabled={!!uploading}
          onClick={() => inputRef.current?.click()}
          onDragOver={(e) => {
            e.preventDefault();
            setDragging(true);
          }}
          onDragLeave={() => setDragging(false)}
          onDrop={(e) => {
            e.preventDefault();
            setDragging(false);
            void handleFile(e.dataTransfer.files[0]);
          }}
          className={`flex w-full flex-col items-center gap-3 rounded-2xl border-2 border-dashed px-6 py-14 transition ${
            dragging
              ? "border-indigo-500 bg-indigo-50 dark:bg-indigo-950/40"
              : "border-slate-300 bg-white hover:border-indigo-400 dark:border-slate-700 dark:bg-slate-900"
          } disabled:cursor-wait`}
        >
          {uploading ? (
            <>
              <span className="h-8 w-8 animate-spin rounded-full border-2 border-indigo-200 border-t-indigo-600" />
              <span className="text-sm text-slate-600 dark:text-slate-300">Uploading {uploading}…</span>
            </>
          ) : (
            <>
              <UploadIcon width={28} height={28} className="text-indigo-500" />
              <span className="font-medium">Drop a PDF here or click to choose one</span>
              <span className="text-xs text-slate-500">
                Up to {maxMb} MB and {health?.max_pages ?? 600} pages
              </span>
            </>
          )}
        </button>
        <input
          ref={inputRef}
          type="file"
          accept="application/pdf,.pdf"
          className="hidden"
          onChange={(e) => {
            void handleFile(e.target.files?.[0]);
            e.target.value = "";
          }}
        />

        {error && (
          <p className="mt-4 rounded-lg bg-red-50 px-4 py-3 text-sm text-red-700 dark:bg-red-950/50 dark:text-red-300">
            {error}
          </p>
        )}
      </div>
    </div>
  );
}

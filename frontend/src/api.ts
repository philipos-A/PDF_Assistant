import type { ChatRequest, DocInfo, Health } from "./types";

async function errorMessage(res: Response): Promise<string> {
  try {
    const body = await res.json();
    if (typeof body.detail === "string") return body.detail;
    if (Array.isArray(body.detail)) return body.detail.map((d: { msg: string }) => d.msg).join("; ");
  } catch {
    /* not JSON */
  }
  return `Request failed (${res.status} ${res.statusText})`;
}

export async function getHealth(): Promise<Health> {
  const res = await fetch("/api/health");
  if (!res.ok) throw new Error(await errorMessage(res));
  return res.json();
}

export async function uploadDocument(file: File): Promise<DocInfo> {
  const form = new FormData();
  form.append("file", file);
  const res = await fetch("/api/documents", { method: "POST", body: form });
  if (!res.ok) throw new Error(await errorMessage(res));
  return res.json();
}

export async function getDocument(docId: string): Promise<DocInfo | null> {
  const res = await fetch(`/api/documents/${encodeURIComponent(docId)}`);
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(await errorMessage(res));
  return res.json();
}

export const documentFileUrl = (docId: string) => `/api/documents/${encodeURIComponent(docId)}/file`;

export interface StreamHandlers {
  onStatus?: (state: "thinking" | "writing" | "fallback") => void;
  onDelta: (text: string) => void;
  onDone?: (info: { stop_reason: string; model: string }) => void;
  /** `refused` is true when Claude declined; any partial answer should be discarded. */
  onError: (message: string, info: { refused: boolean }) => void;
}

/**
 * POST a question and consume the Server-Sent Events response.
 * (EventSource only supports GET, so the stream is parsed by hand.)
 */
export async function streamChat(req: ChatRequest, handlers: StreamHandlers, signal?: AbortSignal) {
  let res: Response;
  try {
    res = await fetch("/api/chat", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(req),
      signal,
    });
  } catch (e) {
    if ((e as Error).name === "AbortError") return;
    handlers.onError("Could not reach the backend. Is it running on port 8000?", { refused: false });
    return;
  }
  if (!res.ok || !res.body) {
    handlers.onError(await errorMessage(res), { refused: false });
    return;
  }

  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let finished = false;

  const dispatch = (raw: string) => {
    if (finished) return;
    let event = "message";
    const data: string[] = [];
    for (const line of raw.split("\n")) {
      if (line.startsWith("event:")) event = line.slice(6).trim();
      else if (line.startsWith("data:")) data.push(line.slice(5).trimStart());
    }
    if (!data.length) return;
    let payload;
    try {
      payload = JSON.parse(data.join("\n"));
    } catch {
      finished = true;
      handlers.onError("The server sent a response the app couldn't read.", { refused: false });
      return;
    }
    switch (event) {
      case "status":
        handlers.onStatus?.(payload.state);
        break;
      case "delta":
        handlers.onDelta(payload.text);
        break;
      case "done":
        finished = true;
        handlers.onDone?.(payload);
        break;
      case "error":
        finished = true;
        handlers.onError(payload.message, { refused: payload.refused === true });
        break;
    }
  };

  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true }).replace(/\r\n/g, "\n");
      let idx;
      while ((idx = buffer.indexOf("\n\n")) !== -1) {
        dispatch(buffer.slice(0, idx));
        buffer = buffer.slice(idx + 2);
      }
    }
    if (buffer.trim()) dispatch(buffer);
  } catch (e) {
    if ((e as Error).name === "AbortError") return;
    handlers.onError("The connection was interrupted.", { refused: false });
    return;
  }
  if (!finished) handlers.onError("The response ended unexpectedly.", { refused: false });
}

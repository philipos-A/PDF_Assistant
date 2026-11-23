/** A Document as the library lists it. */
export interface DocInfo {
  doc_id: string;
  filename: string;
  page_count: number;
  size_bytes: number;
  uploaded_at: string;
  /** Time of the latest Turn, or the upload time. */
  last_active_at: string;
  question_count: number;
}

/** A passage an Answer cites. `n` matches the `[n](#cite-n)` marker in the Answer. */
export interface Citation {
  n: number;
  cited_text: string;
  start_page: number;
  /** Last page of the passage (inclusive). */
  end_page: number;
}

/** A saved Turn, as `GET /api/documents/{id}/conversation` returns it. */
export interface StoredTurn {
  role: "user" | "assistant";
  content: string;
  context: ReaderContext | null;
  model: string | null;
  used_fallback: boolean;
  citations: Citation[];
  created_at: string;
}

export interface Health {
  status: string;
  model: string;
  api_key_configured: boolean;
  max_upload_mb: number;
  max_pages: number;
}

export type MessageStatus = "pending" | "thinking" | "streaming" | "done" | "error";

/** Where the Reader is when a Question is asked. Mirrors the backend's `ReaderContext`. */
export interface ReaderContext {
  current_page: number;
  visible_pages: number[];
  selected_text?: string;
  /** Page the Selection was made on; may differ from current_page after scrolling. */
  selection_page?: number;
}

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  /** Page the reader was on when the question was asked. */
  page: number;
  /** User messages only: the full reader context sent with the question. */
  context?: ReaderContext;
  status: MessageStatus;
  error?: string;
  /** Set when a fallback model answered instead of the configured one. */
  usedFallback?: boolean;
  /** Answers: the passages its `[n](#cite-n)` markers point to. */
  citations?: Citation[];
}

/** A Question. The backend keeps the Conversation, so no history is sent. */
export interface ChatRequest extends ReaderContext {
  doc_id: string;
  question: string;
}

export interface DocInfo {
  doc_id: string;
  filename: string;
  page_count: number;
  size_bytes: number;
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
}

export type HistoryTurn =
  | ({ role: "user"; content: string } & ReaderContext)
  | { role: "assistant"; content: string };

export interface ChatRequest extends ReaderContext {
  doc_id: string;
  question: string;
  history: HistoryTurn[];
}

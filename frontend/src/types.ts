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

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  /** Page the reader was on when the question was asked. */
  page: number;
  visiblePages?: number[];
  selectedText?: string;
  status: MessageStatus;
  error?: string;
  /** Set when a fallback model answered instead of the configured one. */
  usedFallback?: boolean;
}

export interface HistoryTurn {
  role: "user" | "assistant";
  content: string;
  current_page?: number;
  visible_pages?: number[];
  selected_text?: string;
}

export interface ChatRequest {
  doc_id: string;
  question: string;
  current_page: number;
  visible_pages: number[];
  selected_text?: string;
  history: HistoryTurn[];
}

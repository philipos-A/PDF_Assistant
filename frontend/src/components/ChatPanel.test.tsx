import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "../api";
import type { DocInfo, StoredTurn } from "../types";
import { ChatPanel } from "./ChatPanel";

vi.mock("../api", () => ({
  getConversation: vi.fn(),
  clearConversation: vi.fn(),
  streamChat: vi.fn(),
}));

const doc: DocInfo = {
  doc_id: "d".repeat(32),
  filename: "paper.pdf",
  page_count: 10,
  size_bytes: 1,
  uploaded_at: "",
  last_active_at: "",
  question_count: 1,
};

const saved: StoredTurn[] = [
  {
    role: "user",
    content: "What is the main idea?",
    context: { current_page: 2, visible_pages: [2] },
    model: null,
    used_fallback: false,
    citations: [],
    created_at: "",
  },
  {
    role: "assistant",
    content: "Attention only.",
    context: null,
    model: "claude-sonnet-5-5",
    used_fallback: false,
    citations: [],
    created_at: "",
  },
];

function renderPanel() {
  const onCitationClick = vi.fn();
  render(
    <ChatPanel
      doc={doc}
      currentPage={3}
      visiblePages={[3, 4]}
      selectionRequest={null}
      onPageClick={vi.fn()}
      onCitationClick={onCitationClick}
      onCollapse={vi.fn()}
    />,
  );
  return { onCitationClick };
}

describe("ChatPanel", () => {
  beforeEach(() => {
    vi.mocked(api.getConversation).mockResolvedValue(saved);
    vi.mocked(api.clearConversation).mockResolvedValue();
    vi.mocked(api.streamChat).mockReset();
  });

  it("reopens the saved Conversation", async () => {
    renderPanel();
    expect(await screen.findByText("What is the main idea?")).toBeInTheDocument();
    expect(screen.getByText("Attention only.")).toBeInTheDocument();
    expect(api.getConversation).toHaveBeenCalledWith(doc.doc_id);
  });

  it("sends a Question with its Reader Context and streams the cited Answer", async () => {
    vi.mocked(api.streamChat).mockImplementation(async (_req, h) => {
      h.onStatus?.("writing");
      h.onDelta("Softmax");
      h.onCitation?.({ n: 1, cited_text: "We apply a softmax.", start_page: 4, end_page: 4 });
      h.onDelta("[1](#cite-1).");
      h.onDone?.({ stop_reason: "end_turn", model: "m" });
    });
    const { onCitationClick } = renderPanel();
    await screen.findByText("Attention only.");

    await userEvent.type(screen.getByRole("textbox"), "Which function?{Enter}");

    expect(api.streamChat).toHaveBeenCalledWith(
      { doc_id: doc.doc_id, question: "Which function?", current_page: 3, visible_pages: [3, 4] },
      expect.anything(),
      expect.anything(),
    );
    await userEvent.click(await screen.findByRole("button", { name: "Source 1, p. 4" }));
    expect(onCitationClick).toHaveBeenCalledWith(expect.objectContaining({ n: 1, start_page: 4 }));
  });

  it("drops a partial answer when Claude declines", async () => {
    vi.mocked(api.streamChat).mockImplementation(async (_req, h) => {
      h.onDelta("Partial text");
      h.onError("Claude declined to answer this question.", { refused: true });
    });
    renderPanel();
    await screen.findByText("Attention only.");
    await userEvent.type(screen.getByRole("textbox"), "Hmm{Enter}");
    expect(await screen.findByText("Claude declined to answer this question.")).toBeInTheDocument();
    expect(screen.queryByText("Partial text")).not.toBeInTheDocument();
  });

  it("clears the Conversation after confirmation", async () => {
    renderPanel();
    await screen.findByText("Attention only.");
    await userEvent.click(screen.getByRole("button", { name: "Clear" }));
    expect(api.clearConversation).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Clear all" }));
    expect(api.clearConversation).toHaveBeenCalledWith(doc.doc_id);
    await waitFor(() => expect(screen.queryByText("Attention only.")).not.toBeInTheDocument());
  });
});

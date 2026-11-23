import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ChatMessage } from "../types";
import { Message } from "./Message";

const citation = { n: 1, cited_text: "We apply a softmax.", start_page: 3, end_page: 3 };

function answer(patch: Partial<ChatMessage> = {}): ChatMessage {
  return {
    id: "a",
    role: "assistant",
    content: "It uses softmax[1](#cite-1), see p. 4.",
    page: 2,
    status: "done",
    citations: [citation],
    ...patch,
  };
}

function renderMessage(message: ChatMessage) {
  const onPageClick = vi.fn();
  const onCitationClick = vi.fn();
  render(<Message message={message} pageCount={10} onPageClick={onPageClick} onCitationClick={onCitationClick} />);
  return { onPageClick, onCitationClick };
}

describe("Message", () => {
  it("turns citation markers into chips that show the passage", async () => {
    const { onCitationClick } = renderMessage(answer());
    await userEvent.click(screen.getByRole("button", { name: "Source 1, p. 3" }));
    expect(onCitationClick).toHaveBeenCalledWith(citation);
  });

  it("lists the sources under a finished answer", async () => {
    const { onCitationClick } = renderMessage(answer());
    expect(screen.getByText("Sources")).toBeInTheDocument();
    await userEvent.click(screen.getByText("“We apply a softmax.”"));
    expect(onCitationClick).toHaveBeenCalledWith(citation);
  });

  it("hides sources while the answer is streaming", () => {
    renderMessage(answer({ status: "streaming" }));
    expect(screen.queryByText("Sources")).not.toBeInTheDocument();
  });

  it("links page references and the page the question was asked from", async () => {
    const { onPageClick } = renderMessage(answer());
    await userEvent.click(screen.getByRole("button", { name: "4" }));
    expect(onPageClick).toHaveBeenCalledWith(4);
    await userEvent.click(screen.getByRole("button", { name: "Asked from p. 2" }));
    expect(onPageClick).toHaveBeenCalledWith(2);
  });

  it("shows an error", () => {
    renderMessage(answer({ status: "error", content: "", error: "Claude declined." }));
    expect(screen.getByText("Claude declined.")).toBeInTheDocument();
  });

  it("shows the selection a question was asked about", () => {
    renderMessage({
      id: "q",
      role: "user",
      content: "Explain",
      page: 5,
      status: "done",
      context: { current_page: 5, visible_pages: [5], selected_text: "attention" },
    });
    expect(screen.getByText("“attention”")).toBeInTheDocument();
  });
});

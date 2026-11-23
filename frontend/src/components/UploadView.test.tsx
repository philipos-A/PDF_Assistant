import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "../api";
import type { DocInfo } from "../types";
import { UploadView } from "./UploadView";

vi.mock("../api", () => ({
  listDocuments: vi.fn(),
  deleteDocument: vi.fn(),
  uploadDocument: vi.fn(),
}));

const paper: DocInfo = {
  doc_id: "a".repeat(32),
  filename: "attention.pdf",
  page_count: 15,
  size_bytes: 1,
  uploaded_at: new Date().toISOString(),
  last_active_at: new Date().toISOString(),
  question_count: 3,
};

function renderView() {
  const onOpen = vi.fn();
  const { container } = render(<UploadView health={null} onOpen={onOpen} />);
  return { onOpen, input: container.querySelector("input[type=file]") as HTMLInputElement };
}

describe("UploadView", () => {
  beforeEach(() => {
    vi.mocked(api.listDocuments).mockResolvedValue([paper]);
    vi.mocked(api.deleteDocument).mockResolvedValue();
    vi.mocked(api.uploadDocument).mockReset();
  });

  it("lists earlier papers and opens one", async () => {
    const { onOpen } = renderView();
    await userEvent.click(await screen.findByText("attention.pdf"));
    expect(onOpen).toHaveBeenCalledWith(paper);
    expect(screen.getByText(/15 pages · 3 questions · just now/)).toBeInTheDocument();
  });

  it("deletes a paper only after confirmation", async () => {
    renderView();
    await userEvent.click(await screen.findByRole("button", { name: "Delete attention.pdf" }));
    expect(api.deleteDocument).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(api.deleteDocument).toHaveBeenCalledWith(paper.doc_id);
    await waitFor(() => expect(screen.queryByText("attention.pdf")).not.toBeInTheDocument());
  });

  it("rejects files that aren't PDFs without uploading", async () => {
    const { input } = renderView();
    await userEvent.upload(input, new File(["hi"], "notes.txt", { type: "text/plain" }), { applyAccept: false });
    expect(screen.getByText("That file isn't a PDF.")).toBeInTheDocument();
    expect(api.uploadDocument).not.toHaveBeenCalled();
  });

  it("uploads a PDF and opens it", async () => {
    vi.mocked(api.uploadDocument).mockResolvedValue(paper);
    const { input, onOpen } = renderView();
    await userEvent.upload(input, new File(["%PDF-"], "paper.pdf", { type: "application/pdf" }));
    await waitFor(() => expect(onOpen).toHaveBeenCalledWith(paper));
  });
});

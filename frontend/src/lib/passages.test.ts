import { describe, expect, it } from "vitest";
import type { ChatMessage } from "../types";
import { buildPassages, passageKey } from "./passages";

const q = (id: string, extra: Partial<ChatMessage> = {}): ChatMessage => ({
  id,
  role: "user",
  content: "Why?",
  page: 2,
  status: "done",
  ...extra,
});
const a = (id: string, extra: Partial<ChatMessage> = {}): ChatMessage => ({
  id,
  role: "assistant",
  content: "Because.",
  page: 2,
  status: "done",
  ...extra,
});
const cite = (n: number, text: string, page = 3) => ({ n, cited_text: text, start_page: page, end_page: page });

describe("buildPassages", () => {
  it("marks the Selection a Question asked about, on the Selection's page", () => {
    const passages = buildPassages([
      q("q1", { context: { current_page: 2, visible_pages: [2], selected_text: "self-attention", selection_page: 1 } }),
      a("a1"),
    ]);
    expect(passages).toHaveLength(1);
    expect(passages[0]).toMatchObject({ kind: "asked", text: "self-attention", startPage: 1 });
    expect(passages[0].answers[0].question.id).toBe("q1");
  });

  it("groups Answers that cite the same passage", () => {
    const passages = buildPassages([
      q("q1"),
      a("a1", { citations: [cite(1, "Attention is all you need.")] }),
      q("q2"),
      a("a2", { citations: [cite(1, "attention  is all you need.")] }),
    ]);
    expect(passages).toHaveLength(1);
    expect(passages[0].kind).toBe("cited");
    expect(passages[0].answers.map((x) => x.answer.id)).toEqual(["a1", "a2"]);
  });

  it("skips Answers that are still streaming or failed", () => {
    expect(
      buildPassages([q("q1"), a("a1", { status: "streaming", citations: [cite(1, "Some passage text")] })]),
    ).toEqual([]);
  });

  it("lists an Answer once even if it cites a passage twice", () => {
    const passages = buildPassages([q("q1"), a("a1", { citations: [cite(1, "Same"), cite(2, "same")] })]);
    expect(passages[0].answers).toHaveLength(1);
  });
});

describe("passageKey", () => {
  it("ignores case and whitespace", () => {
    expect(passageKey(4, "Self Attention")).toBe(passageKey(4, "self-attention"));
  });
});

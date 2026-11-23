import { describe, expect, it } from "vitest";
import type { StoredTurn } from "../types";
import { citationPages, parseCiteHref, stripCitationMarkers, turnsToMessages } from "./citations";

describe("citation links", () => {
  it("reads a citation link", () => {
    expect(parseCiteHref("#cite-2")).toBe(2);
    expect(parseCiteHref("#page-2")).toBeNull();
    expect(parseCiteHref("#cite-0")).toBeNull();
  });

  it("strips markers", () => {
    expect(stripCitationMarkers("a[1](#cite-1) b[12](#cite-12)")).toBe("a b");
  });

  it("formats one page or a range", () => {
    expect(citationPages({ n: 1, cited_text: "", start_page: 3, end_page: 3 })).toBe("p. 3");
    expect(citationPages({ n: 1, cited_text: "", start_page: 3, end_page: 4 })).toBe("pp. 3–4");
  });
});

describe("turnsToMessages", () => {
  const turn = (t: Partial<StoredTurn>): StoredTurn => ({
    role: "user",
    content: "",
    context: null,
    model: null,
    used_fallback: false,
    citations: [],
    created_at: "",
    ...t,
  });

  it("gives each Answer the page its Question was asked from", () => {
    const citation = { n: 1, cited_text: "x", start_page: 2, end_page: 2 };
    const messages = turnsToMessages([
      turn({ content: "Q1", context: { current_page: 4, visible_pages: [4] } }),
      turn({ role: "assistant", content: "A1", used_fallback: true, citations: [citation] }),
    ]);
    expect(messages.map((m) => [m.role, m.content, m.page, m.status])).toEqual([
      ["user", "Q1", 4, "done"],
      ["assistant", "A1", 4, "done"],
    ]);
    expect(messages[1].usedFallback).toBe(true);
    expect(messages[1].citations).toEqual([citation]);
    expect(messages[0].context?.current_page).toBe(4);
  });
});

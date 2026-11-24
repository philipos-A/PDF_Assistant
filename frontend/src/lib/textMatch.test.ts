import { describe, expect, it } from "vitest";
import { findMatches, findPassage, markItemHtml, normalizeText } from "./textMatch";

describe("normalizeText", () => {
  it("ignores case, whitespace, hyphens, ligatures and curly quotes", () => {
    expect(normalizeText("The  “Trans-\nformer” ﬁts")).toBe('the"transformer"fits');
  });
});

describe("findMatches", () => {
  it("finds every match, case-insensitively", () => {
    expect(findMatches(["Attention and attention"], "ATTENTION")).toEqual([
      [{ item: 0, start: 0, end: 9 }],
      [{ item: 0, start: 14, end: 23 }],
    ]);
  });

  it("matches across text items and line-break hyphens", () => {
    expect(findMatches(["the multi-", "head atten", "tion layer"], "multihead attention")).toEqual([
      [
        { item: 0, start: 4, end: 9 },
        { item: 1, start: 0, end: 10 },
        { item: 2, start: 0, end: 4 },
      ],
    ]);
  });

  it("returns nothing for an empty or whitespace query", () => {
    expect(findMatches(["text"], "  ")).toEqual([]);
  });
});

describe("findPassage", () => {
  const items = ["We apply a softmax over the scores ", "to obtain the weights."];

  it("finds a whole cited passage", () => {
    expect(findPassage(items, "softmax over the scores to obtain")).toEqual([
      { item: 0, start: 11, end: 34 },
      { item: 1, start: 0, end: 9 },
    ]);
  });

  it("falls back to the passage's opening words", () => {
    const passage = "We apply a softmax over the scores to obtain the weights. Then we continue on the next page.";
    expect(findPassage(items, passage, 20)?.[0]).toEqual({ item: 0, start: 0, end: 25 });
  });

  it("gives up when even the opening is missing", () => {
    expect(findPassage(items, "Something else entirely, not on this page.")).toBeNull();
  });
});

describe("findPassage with extraction artefacts", () => {
  it("ignores control characters left where a word was hyphenated", () => {
    expect(findPassage(["the lack of in-", "tegrability in LN"], "the lack of in\u0002tegrability")).not.toBeNull();
  });
});

describe("markItemHtml", () => {
  it("wraps marks and escapes HTML", () => {
    expect(markItemHtml("a <b> c", [{ item: 0, start: 2, end: 5, kind: "hit" }])).toBe(
      'a <mark class="pdf-mark pdf-mark-hit">&lt;b&gt;</mark> c',
    );
  });

  it("keeps the first of overlapping marks", () => {
    expect(
      markItemHtml("abcdef", [
        { item: 0, start: 3, end: 6, kind: "hit" },
        { item: 0, start: 0, end: 4, kind: "cite" },
      ]),
    ).toBe('<mark class="pdf-mark pdf-mark-cite">abcd</mark><mark class="pdf-mark pdf-mark-hit">ef</mark>');
  });

  it("links a mark to its Marked Passage and sets its sweep timing", () => {
    expect(
      markItemHtml("ab", [
        { item: 0, start: 0, end: 2, kind: "cite", passage: '3:a"b', sweep: { delay: -120.4, duration: 300 } },
      ]),
    ).toBe(
      '<mark class="pdf-mark pdf-mark-cite" data-passage="3:a&quot;b" data-sweep style="--sweep-delay:-120ms;--sweep-duration:300ms">ab</mark>',
    );
  });
});

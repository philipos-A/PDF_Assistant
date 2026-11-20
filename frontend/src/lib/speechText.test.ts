import { describe, expect, it } from "vitest";
import { chunkForSpeech, markdownToSpeech } from "./speechText";

describe("markdownToSpeech", () => {
  it("strips emphasis and expands page abbreviations", () => {
    expect(markdownToSpeech("**Bold** claim, see p. 4 and pp. 5–6")).toBe(
      "Bold claim, see page 4 and pages 5–6",
    );
  });

  it("replaces display math and code blocks with a spoken note", () => {
    expect(markdownToSpeech("$$x^2$$")).toBe("(equation shown on screen)");
    expect(markdownToSpeech("```\nlet x = 1;\n```")).toBe("(code omitted)");
  });

  it("reads inline LaTeX commands as words", () => {
    expect(markdownToSpeech("$\\alpha_i$")).toBe("alpha i");
  });

  it("keeps link text and turns paragraphs into sentences", () => {
    expect(markdownToSpeech("# Title\n\nSee [the paper](https://example.com)")).toBe(
      "Title. See the paper",
    );
  });
});

describe("chunkForSpeech", () => {
  it("packs whole sentences into chunks of at most `max` characters", () => {
    expect(chunkForSpeech("One. Two. Three.", 10)).toEqual(["One. Two.", "Three."]);
  });

  it("returns nothing for empty text", () => {
    expect(chunkForSpeech("")).toEqual([]);
  });
});

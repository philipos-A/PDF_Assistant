import { describe, expect, it } from "vitest";
import { linkifyPageRefs, parsePageHref } from "./pageRefs";

describe("linkifyPageRefs", () => {
  it("links a single page reference", () => {
    expect(linkifyPageRefs("See p. 4.", 10)).toBe("See p. [4](#page-4).");
  });

  it("links both ends of a page range", () => {
    expect(linkifyPageRefs("pp. 5–6", 10)).toBe("pp. [5](#page-5)–[6](#page-6)");
  });

  it("links 'Pages N and M'", () => {
    expect(linkifyPageRefs("Pages 2 and 3", 10)).toBe("Pages [2](#page-2) and [3](#page-3)");
  });

  it("leaves pages outside the Document unlinked", () => {
    expect(linkifyPageRefs("page 12", 10)).toBe("page 12");
  });

  it("never touches code or LaTeX", () => {
    const md = "`p. 4` and $p. 4$ and $$p. 4$$";
    expect(linkifyPageRefs(md, 10)).toBe(md);
  });
});

describe("parsePageHref", () => {
  it("reads a page link", () => {
    expect(parsePageHref("#page-3")).toBe(3);
  });

  it("rejects other links and page 0", () => {
    expect(parsePageHref("https://example.com")).toBeNull();
    expect(parsePageHref("#page-0")).toBeNull();
    expect(parsePageHref("#page-x")).toBeNull();
    expect(parsePageHref(undefined)).toBeNull();
  });
});

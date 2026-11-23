import type { Citation, ChatMessage, StoredTurn } from "../types";

/** The backend writes `[n](#cite-n)` into an Answer where citation n applies. */
export const CITE_HREF_PREFIX = "#cite-";

export function parseCiteHref(href: string | undefined): number | null {
  if (!href?.startsWith(CITE_HREF_PREFIX)) return null;
  const n = Number(href.slice(CITE_HREF_PREFIX.length));
  return Number.isInteger(n) && n > 0 ? n : null;
}

export function stripCitationMarkers(text: string): string {
  return text.replace(/\[\d+\]\(#cite-\d+\)/g, "");
}

/** "p. 4" or "pp. 4–5" for a citation's pages. */
export function citationPages(c: Citation): string {
  return c.end_page > c.start_page ? `pp. ${c.start_page}–${c.end_page}` : `p. ${c.start_page}`;
}

/** Rebuild the chat from a saved Conversation. Answers show the page their Question was asked from. */
export function turnsToMessages(turns: StoredTurn[]): ChatMessage[] {
  let page = 1;
  return turns.map((t, i): ChatMessage => {
    if (t.role === "user") {
      page = t.context?.current_page ?? page;
      return {
        id: `saved-${i}`,
        role: "user",
        content: t.content,
        page,
        context: t.context ?? undefined,
        status: "done",
      };
    }
    return {
      id: `saved-${i}`,
      role: "assistant",
      content: t.content,
      page,
      status: "done",
      usedFallback: t.used_fallback || undefined,
      citations: t.citations,
    };
  });
}

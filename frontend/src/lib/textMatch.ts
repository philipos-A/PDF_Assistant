/**
 * Find text in a Page's PDF.js text items, for search and for highlighting cited
 * passages. A PDF splits a line into many items and a sentence across lines, so
 * matching ignores whitespace, hyphens, case and typographic quotes and ligatures.
 */

/** Part of one text item: characters [start, end) of item `item`. */
export interface ItemRange {
  item: number;
  start: number;
  end: number;
}

/** A highlight to draw: search hits, the current hit, or a cited passage. */
export type MarkKind = "hit" | "current" | "cite";

export interface Mark extends ItemRange {
  kind: MarkKind;
}

const SKIP = /[\s­\-‐-―]/;
const QUOTES: Record<string, string> = { "‘": "'", "’": "'", "“": '"', "”": '"' };

function normalizeChar(ch: string): string {
  let out = "";
  for (const c of ch.normalize("NFKC").toLowerCase()) {
    if (!SKIP.test(c)) out += QUOTES[c] ?? c;
  }
  return out;
}

/** Normalize a query or cited passage the same way Page text is normalized. */
export function normalizeText(text: string): string {
  let out = "";
  for (const ch of text) out += normalizeChar(ch);
  return out;
}

interface Normalized {
  text: string;
  /** For each character of `text`: the item and offset it came from. */
  origin: { item: number; offset: number }[];
}

function normalizeItems(items: string[]): Normalized {
  let text = "";
  const origin: Normalized["origin"] = [];
  items.forEach((str, item) => {
    let offset = 0;
    for (const ch of str) {
      for (const c of normalizeChar(ch)) {
        text += c;
        origin.push({ item, offset });
      }
      offset += ch.length;
    }
  });
  return { text, origin };
}

function toRanges(n: Normalized, from: number, to: number, items: string[]): ItemRange[] {
  const ranges: ItemRange[] = [];
  for (let k = from; k < to; k++) {
    const { item, offset } = n.origin[k];
    const end = offset + (items[item].codePointAt(offset)! > 0xffff ? 2 : 1);
    const last = ranges[ranges.length - 1];
    if (last && last.item === item) last.end = Math.max(last.end, end);
    else ranges.push({ item, start: offset, end });
  }
  return ranges;
}

/** Every non-overlapping match of `query` in a Page's items, in reading order. */
export function findMatches(items: string[], query: string): ItemRange[][] {
  const needle = normalizeText(query);
  if (!needle) return [];
  const n = normalizeItems(items);
  const matches: ItemRange[][] = [];
  for (let at = n.text.indexOf(needle); at !== -1; at = n.text.indexOf(needle, at + needle.length)) {
    matches.push(toRanges(n, at, at + needle.length, items));
  }
  return matches;
}

/**
 * Where a cited passage is on a Page. Claude's `cited_text` can differ slightly
 * from the text layer (or run onto the next Page), so if the whole passage is not
 * found, fall back to its opening words.
 */
export function findPassage(items: string[], passage: string, prefixChars = 60): ItemRange[] | null {
  const whole = findMatches(items, passage)[0];
  if (whole) return whole;
  const opening = normalizeText(passage).slice(0, prefixChars);
  if (opening.length < 12) return null;
  const n = normalizeItems(items);
  const at = n.text.indexOf(opening);
  return at === -1 ? null : toRanges(n, at, at + opening.length, items);
}

const escapeHtml = (s: string) =>
  s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

/** HTML for one text item with its marks wrapped in `<mark>`. Overlaps keep the first mark. */
export function markItemHtml(str: string, marks: Mark[]): string {
  if (!marks.length) return escapeHtml(str);
  const sorted = [...marks].sort((a, b) => a.start - b.start);
  let html = "";
  let pos = 0;
  for (const m of sorted) {
    const start = Math.max(m.start, pos);
    const end = Math.min(m.end, str.length);
    if (start >= end) continue;
    html += escapeHtml(str.slice(pos, start));
    html += `<mark class="pdf-mark pdf-mark-${m.kind}">${escapeHtml(str.slice(start, end))}</mark>`;
    pos = end;
  }
  return html + escapeHtml(str.slice(pos));
}

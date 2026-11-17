/**
 * Turn page references in Claude's markdown ("p. 4", "pp. 4–5", "page 12")
 * into `#page-N` links, skipping code and LaTeX so math is never altered.
 */

const PROTECTED = /(```[\s\S]*?```|`[^`\n]*`|\$\$[\s\S]*?\$\$|\$[^$\n]+?\$)/g;
const PAGE_REF =
  /\b(pp?\.|[Pp]ages?)(\s*)(\d{1,4})(?:(\s*[–—-]\s*|\s+and\s+|,\s*)(\d{1,4}))?(?![\d\]])/g;

export const PAGE_HREF_PREFIX = "#page-";

export function linkifyPageRefs(markdown: string, pageCount: number): string {
  const inRange = (n: number) => n >= 1 && n <= pageCount;
  const link = (n: string) => (inRange(Number(n)) ? `[${n}](${PAGE_HREF_PREFIX}${n})` : n);

  return markdown
    .split(PROTECTED)
    .map((segment, i) =>
      i % 2 === 1
        ? segment
        : segment.replace(PAGE_REF, (_m, label, sp, a, sep, b) =>
            `${label}${sp}${link(a)}` + (b ? `${sep}${link(b)}` : ""),
          ),
    )
    .join("");
}

export function parsePageHref(href: string | undefined): number | null {
  if (!href?.startsWith(PAGE_HREF_PREFIX)) return null;
  const n = Number(href.slice(PAGE_HREF_PREFIX.length));
  return Number.isInteger(n) && n > 0 ? n : null;
}

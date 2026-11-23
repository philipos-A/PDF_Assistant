import { useCallback, useEffect, useState } from "react";
import { findMatches, type ItemRange } from "../lib/textMatch";

export interface SearchMatch {
  page: number;
  ranges: ItemRange[];
}

/**
 * Search the whole Document. Page text is read page by page (and cached by the
 * caller), so results fill in as a long Document is searched. `reveal` scrolls to a
 * page and brings its current match into view.
 */
export function usePdfSearch(
  numPages: number,
  getPageText: (page: number) => Promise<string[]>,
  reveal: (page: number) => void,
) {
  const [open, setOpenState] = useState(false);
  const [query, setQuery] = useState("");
  const [matches, setMatches] = useState<SearchMatch[]>([]);
  const [current, setCurrent] = useState(0);
  const [searching, setSearching] = useState(false);

  useEffect(() => {
    const q = query.trim();
    setMatches([]);
    setCurrent(0);
    if (!open || !q || !numPages) {
      setSearching(false);
      return;
    }
    let cancelled = false;
    setSearching(true);
    const timer = setTimeout(async () => {
      const found: SearchMatch[] = [];
      let revealed = false;
      for (let page = 1; page <= numPages && !cancelled; page++) {
        for (const ranges of findMatches(await getPageText(page), q)) found.push({ page, ranges });
        if (cancelled) return;
        // Show progress, and jump to the first match as soon as there is one.
        if (page % 20 === 0 || page === numPages) setMatches([...found]);
        if (!revealed && found.length) {
          revealed = true;
          setMatches([...found]);
          reveal(found[0].page);
        }
      }
      if (!cancelled) setSearching(false);
    }, 250);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [query, open, numPages, getPageText, reveal]);

  const step = useCallback(
    (delta: number) => {
      if (!matches.length) return;
      const next = (current + delta + matches.length) % matches.length;
      setCurrent(next);
      reveal(matches[next].page);
    },
    [matches, current, reveal],
  );

  const setOpen = useCallback((value: boolean) => {
    setOpenState(value);
    if (!value) setQuery("");
  }, []);

  const status = !query.trim()
    ? ""
    : matches.length
      ? `${current + 1} of ${matches.length}${searching ? "+" : ""}`
      : searching
        ? "Searching…"
        : "No matches";

  return { open, setOpen, query, setQuery, matches, current, step, status };
}

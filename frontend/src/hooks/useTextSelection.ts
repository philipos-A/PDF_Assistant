import { type RefObject, useCallback, useEffect, useRef, useState } from "react";

export interface TextSelectionInfo {
  text: string;
  page: number | null;
  rect: DOMRect;
}

/** Track a non-empty text selection inside `containerRef` and its screen position. */
export function useTextSelection(containerRef: RefObject<HTMLElement | null>) {
  const [selection, setSelection] = useState<TextSelectionInfo | null>(null);
  const rangeRef = useRef<Range | null>(null);

  const clear = useCallback(() => {
    rangeRef.current = null;
    setSelection(null);
  }, []);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const read = () => {
      const sel = window.getSelection();
      if (!sel || sel.isCollapsed || sel.rangeCount === 0) return clear();
      const range = sel.getRangeAt(0);
      if (!container.contains(range.commonAncestorContainer)) return clear();
      const text = sel.toString().replace(/\s+/g, " ").trim();
      if (text.length < 2) return clear();
      const node = range.startContainer;
      const el = node instanceof Element ? node : node.parentElement;
      const pageEl = el?.closest<HTMLElement>("[data-page-number]");
      rangeRef.current = range;
      setSelection({
        text: text.slice(0, 5000),
        page: pageEl ? Number(pageEl.dataset.pageNumber) : null,
        rect: range.getBoundingClientRect(),
      });
    };

    const onUp = () => setTimeout(read, 0);
    const onSelectionChange = () => {
      const sel = window.getSelection();
      if (!sel || sel.isCollapsed) clear();
    };
    const onScroll = () => {
      const range = rangeRef.current;
      if (range) setSelection((s) => (s ? { ...s, rect: range.getBoundingClientRect() } : s));
    };

    container.addEventListener("mouseup", onUp);
    container.addEventListener("keyup", onUp);
    container.addEventListener("scroll", onScroll, { passive: true });
    document.addEventListener("selectionchange", onSelectionChange);
    return () => {
      container.removeEventListener("mouseup", onUp);
      container.removeEventListener("keyup", onUp);
      container.removeEventListener("scroll", onScroll);
      document.removeEventListener("selectionchange", onSelectionChange);
    };
  }, [containerRef, clear]);

  return { selection, clear };
}

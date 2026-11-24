import {
  memo,
  type Ref,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Document, Page, pdfjs } from "react-pdf";
import type { PDFDocumentProxy } from "pdfjs-dist";
import { usePdfSearch } from "../hooks/usePdfSearch";
import { useTextSelection } from "../hooks/useTextSelection";
import { citationKey, type MarkedPassage } from "../lib/passages";
import { findPassage, type ItemRange, type Mark, markItemHtml } from "../lib/textMatch";
import type { Citation } from "../types";
import {
  ChevronDownIcon,
  ChevronUpIcon,
  CloseIcon,
  FitIcon,
  HighlighterIcon,
  MinusIcon,
  PlusIcon,
  SearchIcon,
} from "./icons";
import { SelectionPopover } from "./SelectionPopover";

pdfjs.GlobalWorkerOptions.workerSrc = new URL("pdfjs-dist/build/pdf.worker.min.mjs", import.meta.url).toString();

const ZOOM_STEPS = [0.5, 0.67, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3];
const PAGE_GAP = 16;
const SIDE_PADDING = 48;
/** Pages rendered above/below the current one; the rest are sized placeholders. */
const RENDER_WINDOW = 3;
/** Highlighter sweep speed, and the shortest and longest a whole passage takes. */
const SWEEP_MS_PER_CHAR = 14;
const SWEEP_MIN_MS = 350;
const SWEEP_MAX_MS = 1600;
const PASSAGES_KEY = "pdf-assistant:show-passages";

/** Where a passage was found: its Page and text-layer ranges. */
interface Located {
  page: number;
  ranges: ItemRange[];
}

/** Ranges in reading order → per-range sweep start and length (ms), so the highlighter crosses lines in turn. */
function planSweep(ranges: ItemRange[]): { start: number; duration: number }[] {
  const total = ranges.reduce((n, r) => n + (r.end - r.start), 0) || 1;
  const scale = Math.min(SWEEP_MAX_MS, Math.max(SWEEP_MIN_MS, total * SWEEP_MS_PER_CHAR)) / total;
  let at = 0;
  return ranges.map((r) => {
    const duration = (r.end - r.start) * scale;
    const plan = { start: at, duration };
    at += duration;
    return plan;
  });
}

function loadFlag(key: string, fallback: boolean): boolean {
  try {
    const v = localStorage.getItem(key);
    return v === null ? fallback : v === "1";
  } catch {
    return fallback;
  }
}

export interface PdfViewerHandle {
  scrollToPage: (page: number) => void;
  /** Scroll to a cited passage and sweep a highlight across it. */
  showCitation: (citation: Citation) => void;
}

interface Props {
  fileUrl: string;
  ref?: Ref<PdfViewerHandle>;
  onPageChange: (current: number, visible: number[]) => void;
  onAskAboutSelection: (text: string, page: number | null) => void;
  /** Passages to keep marked; clicking one calls onPassageClick. */
  passages?: MarkedPassage[];
  onPassageClick?: (key: string, rect: DOMRect) => void;
}

interface PageSize {
  w: number;
  h: number;
}

const NO_PASSAGES: MarkedPassage[] = [];

/**
 * Memoized: the App re-renders on every streamed word of an Answer, which this
 * viewer (one element per Page) doesn't need to follow.
 */
export const PdfViewer = memo(function PdfViewer({
  fileUrl,
  ref,
  onPageChange,
  onAskAboutSelection,
  passages = NO_PASSAGES,
  onPassageClick,
}: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const pageEls = useRef<(HTMLDivElement | null)[]>([]);
  const visibleHeights = useRef(new Map<number, number>());
  const zoomAnchor = useRef<{ page: number; frac: number } | null>(null);
  const pdfRef = useRef<PDFDocumentProxy | null>(null);
  const pageText = useRef(new Map<number, Promise<string[]>>());
  /** Page whose current highlight should be scrolled into view once its text layer renders. */
  const pendingReveal = useRef<number | null>(null);
  const searchInput = useRef<HTMLInputElement>(null);

  const [numPages, setNumPages] = useState(0);
  const [sizes, setSizes] = useState<PageSize[]>([]);
  const [containerWidth, setContainerWidth] = useState(800);
  const [zoom, setZoom] = useState(1);
  const [currentPage, setCurrentPage] = useState(1);
  const [visiblePages, setVisiblePages] = useState<number[]>([1]);
  const [pageInput, setPageInput] = useState("1");
  const [loadError, setLoadError] = useState<string | null>(null);
  /** The cited passage being shown, and when its highlighter sweep started. */
  const [cited, setCited] = useState<(Located & { key: string; startedAt: number }) | null>(null);
  const [located, setLocated] = useState<Map<string, Located>>(new Map());
  const [showPassages, setShowPassages] = useState(() => loadFlag(PASSAGES_KEY, true));
  const renderSetRef = useRef<Set<number>>(new Set());
  /** Latest showCitation call; an older one that finishes later must not replace its highlight. */
  const citationRequest = useRef(0);

  const { selection, clear: clearSelection } = useTextSelection(scrollRef);

  // Fit-width base: zoom 1 fills the viewer (capped so huge monitors stay readable).
  const pageWidth = Math.round(Math.max(240, Math.min(containerWidth - SIDE_PADDING, 1100)) * zoom);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => setContainerWidth(entry.contentRect.width));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  /** A Page's text items, as the text layer numbers them (loaded once per Page). */
  const getPageText = useCallback((page: number): Promise<string[]> => {
    const pdf = pdfRef.current;
    if (!pdf) return Promise.resolve([]);
    let text = pageText.current.get(page);
    if (!text) {
      text = pdf
        .getPage(page)
        .then((p) => p.getTextContent())
        .then((c) => c.items.map((item) => ("str" in item ? item.str : "")));
      pageText.current.set(page, text);
    }
    return text;
  }, []);

  const onLoadSuccess = useCallback(async (pdf: PDFDocumentProxy) => {
    pdfRef.current = pdf;
    pageText.current.clear();
    setNumPages(pdf.numPages);
    setLoadError(null);
    const all = await Promise.all(
      Array.from({ length: pdf.numPages }, async (_, i) => {
        const page = await pdf.getPage(i + 1);
        const vp = page.getViewport({ scale: 1 });
        return { w: vp.width, h: vp.height };
      }),
    );
    setSizes(all);
  }, []);

  const heightOf = useCallback(
    (i: number) => {
      const s = sizes[i] ?? sizes[0] ?? { w: 612, h: 792 };
      return Math.round((pageWidth * s.h) / s.w);
    },
    [sizes, pageWidth],
  );

  const scrollToPage = useCallback((page: number, smooth = true) => {
    const el = pageEls.current[page - 1];
    const container = scrollRef.current;
    if (!el || !container) return;
    container.scrollTo({ top: el.offsetTop - PAGE_GAP, behavior: smooth ? "smooth" : "auto" });
  }, []);

  const reveal = useCallback(
    (page: number) => {
      pendingReveal.current = page;
      // A rendered page redraws its text layer for the new highlight, which then scrolls it into
      // view; jumping to the page top first would make the view lurch.
      if (!renderSetRef.current.has(page)) scrollToPage(page);
    },
    [scrollToPage],
  );

  const search = usePdfSearch(numPages, getPageText, reveal);

  const showCitation = useCallback(
    async (c: Citation) => {
      const request = ++citationRequest.current;
      const last = Math.min(Math.max(c.end_page, c.start_page), numPages || c.end_page);
      for (let page = c.start_page; page <= last; page++) {
        const ranges = findPassage(await getPageText(page), c.cited_text);
        if (request !== citationRequest.current) return;
        if (ranges) {
          setCited({ page, ranges, key: citationKey(c), startedAt: performance.now() });
          reveal(page);
          return;
        }
      }
      // Not found in the text layer (e.g. a scanned page): just go to the page.
      setCited(null);
      scrollToPage(c.start_page);
    },
    [getPageText, numPages, reveal, scrollToPage],
  );

  useImperativeHandle(
    ref,
    () => ({
      scrollToPage: (p) => scrollToPage(Math.min(Math.max(1, p), numPages || p)),
      showCitation: (c) => void showCitation(c),
    }),
    [scrollToPage, showCitation, numPages],
  );

  // Find each Marked Passage in the text layer (only when the set of passages changes).
  const passageSig = passages.map((p) => p.key).join("\n");
  useEffect(() => {
    if (!numPages) return;
    let cancelled = false;
    void (async () => {
      const found = new Map<string, Located>();
      for (const p of passages) {
        for (let page = p.startPage; page <= Math.min(p.endPage, numPages); page++) {
          const ranges = findPassage(await getPageText(page), p.text);
          if (ranges) {
            found.set(p.key, { page, ranges });
            break;
          }
        }
      }
      if (!cancelled) setLocated(found);
    })();
    return () => {
      cancelled = true;
    };
    // passageSig stands for `passages`, so a new array with the same passages does not re-run this.
  }, [passageSig, numPages, getPageText]);

  // Every highlight, by page and then by text item, for the text layer to draw.
  // Earlier ones win where they overlap: search, then the cited passage, then Marked Passages.
  const marks = useMemo(() => {
    const byPage = new Map<number, Map<number, Mark[]>>();
    const add = (page: number, ranges: ItemRange[], mark: (r: ItemRange, i: number) => Mark) => {
      let items = byPage.get(page);
      if (!items) byPage.set(page, (items = new Map()));
      ranges.forEach((r, i) => items.set(r.item, [...(items.get(r.item) ?? []), mark(r, i)]));
    };
    search.matches.forEach((m, i) =>
      add(m.page, m.ranges, (r) => ({ ...r, kind: i === search.current ? "current" : "hit" })),
    );
    if (cited) {
      const plan = planSweep(cited.ranges);
      const passage = located.has(cited.key) ? cited.key : undefined;
      // `sweep.delay` is relative to the sweep's start here; renderText makes it relative to now.
      add(cited.page, cited.ranges, (r, i) => ({
        ...r,
        kind: "cite",
        passage,
        sweep: { delay: plan[i].start, duration: plan[i].duration },
      }));
    }
    if (showPassages) {
      const kindOf = new Map(passages.map((p) => [p.key, p.kind]));
      for (const [key, loc] of located) {
        if (key === cited?.key) continue;
        add(loc.page, loc.ranges, (r) => ({
          ...r,
          kind: kindOf.get(key) === "asked" ? "asked" : "noted",
          passage: key,
        }));
      }
    }
    return byPage;
  }, [search.matches, search.current, cited, located, showPassages, passages]);

  const renderText = useCallback(
    ({ pageNumber, itemIndex, str }: { pageNumber: number; itemIndex: number; str: string }) => {
      const itemMarks = marks.get(pageNumber)?.get(itemIndex) ?? [];
      // The text layer is redrawn whenever any highlight changes. Offsetting the sweep by the time
      // since it started lets a redrawn sweep carry on where it was instead of starting over.
      const elapsed = cited ? performance.now() - cited.startedAt : 0;
      return markItemHtml(
        str,
        itemMarks.map((m) => {
          if (!m.sweep) return m;
          const delay = m.sweep.delay - elapsed;
          return delay + m.sweep.duration <= 0 ? { ...m, sweep: undefined } : { ...m, sweep: { ...m.sweep, delay } };
        }),
      );
    },
    [marks, cited],
  );

  const toggleShowPassages = () => {
    setShowPassages((v) => {
      try {
        localStorage.setItem(PASSAGES_KEY, v ? "0" : "1");
      } catch {
        /* storage unavailable */
      }
      return !v;
    });
  };

  // Clicking a Marked Passage (without selecting text) shows its saved Answers.
  const onViewerClick = (e: React.MouseEvent) => {
    const mark = (e.target as Element).closest?.<HTMLElement>("mark[data-passage]");
    if (!mark || !onPassageClick) return;
    const sel = window.getSelection();
    if (sel && !sel.isCollapsed) return;
    onPassageClick(mark.dataset.passage!, mark.getBoundingClientRect());
  };

  // Once a page's text layer is drawn, bring its current highlight into view.
  const onTextLayerRendered = (page: number) => {
    if (pendingReveal.current !== page) return;
    pendingReveal.current = null;
    const el = pageEls.current[page - 1]?.querySelector(".pdf-mark-current, .pdf-mark-cite");
    el?.scrollIntoView({ block: "center", behavior: "smooth" });
  };

  // Cmd/Ctrl+F opens search; Escape clears a citation highlight.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "f") {
        e.preventDefault();
        search.setOpen(true);
        requestAnimationFrame(() => searchInput.current?.select());
      } else if (
        e.key === "Escape" &&
        !e.defaultPrevented &&
        !(e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement)
      ) {
        setCited(null);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [search.setOpen]);

  // Track which pages are on screen and which one holds the most screen area.
  useEffect(() => {
    const root = scrollRef.current;
    if (!root || !numPages) return;
    visibleHeights.current.clear();
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          const page = Number((e.target as HTMLElement).dataset.pageNumber);
          if (e.isIntersecting) visibleHeights.current.set(page, e.intersectionRect.height);
          else visibleHeights.current.delete(page);
        }
        const visible = [...visibleHeights.current.keys()].sort((a, b) => a - b);
        if (!visible.length) return;
        let best = visible[0];
        for (const p of visible) if (visibleHeights.current.get(p)! > visibleHeights.current.get(best)!) best = p;
        setCurrentPage(best);
        setVisiblePages((prev) => (prev.join() === visible.join() ? prev : visible));
      },
      { root, threshold: Array.from({ length: 21 }, (_, i) => i / 20) },
    );
    pageEls.current.slice(0, numPages).forEach((el) => el && io.observe(el));
    return () => io.disconnect();
  }, [numPages, pageWidth, sizes]);

  useEffect(() => {
    onPageChange(currentPage, visiblePages);
    setPageInput(String(currentPage));
  }, [currentPage, visiblePages, onPageChange]);

  // Keep the same spot in view when zooming.
  const changeZoom = (next: number) => {
    const container = scrollRef.current;
    const el = pageEls.current[currentPage - 1];
    if (container && el) {
      zoomAnchor.current = { page: currentPage, frac: (container.scrollTop - el.offsetTop) / el.offsetHeight };
    }
    clearSelection();
    setZoom(next);
  };
  useLayoutEffect(() => {
    const anchor = zoomAnchor.current;
    const container = scrollRef.current;
    const el = anchor && pageEls.current[anchor.page - 1];
    if (anchor && container && el) container.scrollTop = el.offsetTop + anchor.frac * el.offsetHeight;
    zoomAnchor.current = null;
  }, [pageWidth]);

  const zoomIndex = ZOOM_STEPS.findIndex((z) => z >= zoom - 1e-6);
  const zoomIn = () => changeZoom(ZOOM_STEPS[Math.min(ZOOM_STEPS.length - 1, zoomIndex + 1)]);
  const zoomOut = () => changeZoom(ZOOM_STEPS[Math.max(0, zoomIndex - 1)]);

  const renderSet = useMemo(() => {
    const s = new Set<number>(visiblePages);
    for (let p = currentPage - RENDER_WINDOW; p <= currentPage + RENDER_WINDOW; p++) s.add(p);
    renderSetRef.current = s;
    return s;
  }, [currentPage, visiblePages]);

  const goToInput = () => {
    const n = Number(pageInput);
    if (Number.isInteger(n) && n >= 1 && n <= numPages) scrollToPage(n);
    else setPageInput(String(currentPage));
  };

  const toolbarButton =
    "grid h-8 w-8 place-items-center rounded-md text-slate-600 transition hover:bg-slate-200 disabled:opacity-40 dark:text-slate-300 dark:hover:bg-slate-700";

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex h-11 shrink-0 items-center justify-between gap-2 border-b border-slate-200 bg-white px-3 dark:border-slate-800 dark:bg-slate-900">
        <form
          className="flex items-center gap-1.5 text-sm text-slate-600 dark:text-slate-300"
          onSubmit={(e) => {
            e.preventDefault();
            goToInput();
          }}
        >
          <span>Page</span>
          <input
            aria-label="Current page"
            value={pageInput}
            inputMode="numeric"
            onChange={(e) => setPageInput(e.target.value.replace(/\D/g, ""))}
            onBlur={goToInput}
            className="w-12 rounded-md border border-slate-300 bg-transparent px-1.5 py-0.5 text-center tabular-nums focus:border-indigo-500 focus:outline-none dark:border-slate-600"
          />
          <span className="tabular-nums">of {numPages || "–"}</span>
        </form>
        <div className="flex items-center gap-1">
          {!search.open && (
            <button
              type="button"
              className={toolbarButton}
              onClick={() => {
                search.setOpen(true);
                requestAnimationFrame(() => searchInput.current?.focus());
              }}
              title="Search (Ctrl+F)"
            >
              <SearchIcon width={16} height={16} />
            </button>
          )}
          {passages.length > 0 && (
            <button
              type="button"
              className={`${toolbarButton} ${showPassages ? "bg-amber-100 text-amber-800 hover:bg-amber-200 dark:bg-amber-900/50 dark:text-amber-200 dark:hover:bg-amber-900" : ""}`}
              onClick={toggleShowPassages}
              aria-pressed={showPassages}
              title={showPassages ? "Hide highlights of answered passages" : "Show highlights of answered passages"}
            >
              <HighlighterIcon width={16} height={16} />
            </button>
          )}
          <button type="button" className={toolbarButton} onClick={zoomOut} disabled={zoomIndex <= 0} title="Zoom out">
            <MinusIcon />
          </button>
          <span className="w-12 text-center text-sm tabular-nums text-slate-600 dark:text-slate-300">
            {Math.round(zoom * 100)}%
          </span>
          <button
            type="button"
            className={toolbarButton}
            onClick={zoomIn}
            disabled={zoomIndex >= ZOOM_STEPS.length - 1}
            title="Zoom in"
          >
            <PlusIcon />
          </button>
          <button type="button" className={toolbarButton} onClick={() => changeZoom(1)} title="Fit to width">
            <FitIcon />
          </button>
        </div>
      </div>

      {search.open && (
        <div className="flex h-10 shrink-0 items-center gap-0.5 border-b border-slate-200 bg-white pl-3 pr-2 dark:border-slate-800 dark:bg-slate-900">
          <SearchIcon width={14} height={14} className="shrink-0 text-slate-400" />
          <input
            ref={searchInput}
            aria-label="Search in the PDF"
            placeholder="Search"
            value={search.query}
            onChange={(e) => search.setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                search.step(e.shiftKey ? -1 : 1);
              } else if (e.key === "Escape") {
                search.setOpen(false);
              }
            }}
            className="min-w-0 flex-1 bg-transparent px-2 py-1 text-sm focus:outline-none"
          />
          <span className="shrink-0 px-1 text-right text-xs tabular-nums text-slate-500" aria-live="polite">
            {search.status}
          </span>
          <button
            type="button"
            className={toolbarButton}
            onClick={() => search.step(-1)}
            disabled={!search.matches.length}
            title="Previous match (Shift+Enter)"
          >
            <ChevronUpIcon width={16} height={16} />
          </button>
          <button
            type="button"
            className={toolbarButton}
            onClick={() => search.step(1)}
            disabled={!search.matches.length}
            title="Next match (Enter)"
          >
            <ChevronDownIcon width={16} height={16} />
          </button>
          <button type="button" className={toolbarButton} onClick={() => search.setOpen(false)} title="Close search">
            <CloseIcon width={14} height={14} />
          </button>
        </div>
      )}

      <div
        ref={scrollRef}
        onClick={onViewerClick}
        className="relative min-h-0 flex-1 overflow-auto bg-slate-200/70 dark:bg-slate-950"
      >
        {loadError ? (
          <div className="p-8 text-center text-sm text-red-600">{loadError}</div>
        ) : (
          <Document
            file={fileUrl}
            onLoadSuccess={onLoadSuccess}
            onLoadError={(e) => setLoadError(`Could not load the PDF: ${e.message}`)}
            onItemClick={({ pageNumber }) => scrollToPage(pageNumber)}
            loading={<div className="p-8 text-center text-sm text-slate-500">Loading PDF…</div>}
            className="flex flex-col items-center"
            externalLinkTarget="_blank"
          >
            {Array.from({ length: numPages }, (_, i) => {
              const pageNumber = i + 1;
              return (
                <div
                  key={pageNumber}
                  ref={(el) => {
                    pageEls.current[i] = el;
                  }}
                  data-page-number={pageNumber}
                  className="relative shrink-0 overflow-hidden bg-white shadow-md shadow-slate-900/10"
                  style={{ width: pageWidth, height: heightOf(i), marginTop: PAGE_GAP }}
                >
                  {renderSet.has(pageNumber) && sizes.length > 0 && (
                    <Page
                      pageNumber={pageNumber}
                      width={pageWidth}
                      loading={null}
                      customTextRenderer={marks.has(pageNumber) ? renderText : undefined}
                      onRenderTextLayerSuccess={() => onTextLayerRendered(pageNumber)}
                    />
                  )}
                </div>
              );
            })}
            <div style={{ height: PAGE_GAP }} />
          </Document>
        )}
      </div>

      {selection && (
        <SelectionPopover
          selection={selection}
          onAsk={() => {
            onAskAboutSelection(selection.text, selection.page);
            window.getSelection()?.removeAllRanges();
            clearSelection();
          }}
        />
      )}
    </div>
  );
});

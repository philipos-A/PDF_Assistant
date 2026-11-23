import {
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
import { findPassage, type ItemRange, type Mark, markItemHtml } from "../lib/textMatch";
import type { Citation } from "../types";
import { ChevronDownIcon, ChevronUpIcon, CloseIcon, FitIcon, MinusIcon, PlusIcon, SearchIcon } from "./icons";
import { SelectionPopover } from "./SelectionPopover";

pdfjs.GlobalWorkerOptions.workerSrc = new URL("pdfjs-dist/build/pdf.worker.min.mjs", import.meta.url).toString();

const ZOOM_STEPS = [0.5, 0.67, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3];
const PAGE_GAP = 16;
const SIDE_PADDING = 48;
/** Pages rendered above/below the current one; the rest are sized placeholders. */
const RENDER_WINDOW = 3;

export interface PdfViewerHandle {
  scrollToPage: (page: number) => void;
  /** Scroll to a cited passage and highlight it. */
  showCitation: (citation: Citation) => void;
}

interface Props {
  fileUrl: string;
  ref?: Ref<PdfViewerHandle>;
  onPageChange: (current: number, visible: number[]) => void;
  onAskAboutSelection: (text: string, page: number | null) => void;
}

interface PageSize {
  w: number;
  h: number;
}

export function PdfViewer({ fileUrl, ref, onPageChange, onAskAboutSelection }: Props) {
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
  const [cited, setCited] = useState<{ page: number; ranges: ItemRange[] } | null>(null);

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

  const scrollToPage = useCallback(
    (page: number, smooth = true) => {
      const el = pageEls.current[page - 1];
      const container = scrollRef.current;
      if (!el || !container) return;
      container.scrollTo({ top: el.offsetTop - PAGE_GAP, behavior: smooth ? "smooth" : "auto" });
    },
    [],
  );

  const reveal = useCallback(
    (page: number) => {
      pendingReveal.current = page;
      scrollToPage(page);
    },
    [scrollToPage],
  );

  const search = usePdfSearch(numPages, getPageText, reveal);

  const showCitation = useCallback(
    async (c: Citation) => {
      const last = Math.min(Math.max(c.end_page, c.start_page), numPages || c.end_page);
      for (let page = c.start_page; page <= last; page++) {
        const ranges = findPassage(await getPageText(page), c.cited_text);
        if (ranges) {
          setCited({ page, ranges });
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

  // Every highlight, by page and then by text item, for the text layer to draw.
  const marks = useMemo(() => {
    const byPage = new Map<number, Map<number, Mark[]>>();
    const add = (page: number, ranges: ItemRange[], kind: Mark["kind"]) => {
      let items = byPage.get(page);
      if (!items) byPage.set(page, (items = new Map()));
      for (const r of ranges) items.set(r.item, [...(items.get(r.item) ?? []), { ...r, kind }]);
    };
    search.matches.forEach((m, i) => add(m.page, m.ranges, i === search.current ? "current" : "hit"));
    if (cited) add(cited.page, cited.ranges, "cite");
    return byPage;
  }, [search.matches, search.current, cited]);

  const renderText = useCallback(
    ({ pageNumber, itemIndex, str }: { pageNumber: number; itemIndex: number; str: string }) =>
      markItemHtml(str, marks.get(pageNumber)?.get(itemIndex) ?? []),
    [marks],
  );

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
      } else if (e.key === "Escape" && !(e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement)) {
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

      <div ref={scrollRef} className="relative min-h-0 flex-1 overflow-auto bg-slate-200/70 dark:bg-slate-950">
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
}

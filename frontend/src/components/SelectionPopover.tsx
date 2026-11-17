import type { TextSelectionInfo } from "../hooks/useTextSelection";
import { SparkIcon } from "./icons";

interface Props {
  selection: TextSelectionInfo;
  onAsk: () => void;
}

/** Floating "Ask about this" button above (or below) the highlighted text. */
export function SelectionPopover({ selection, onAsk }: Props) {
  const { rect } = selection;
  if (rect.width === 0 && rect.height === 0) return null;
  const width = 150;
  const left = Math.min(Math.max(8, rect.left + rect.width / 2 - width / 2), window.innerWidth - width - 8);
  const above = rect.top > 56;
  const top = above ? rect.top - 44 : rect.bottom + 8;

  return (
    <button
      type="button"
      style={{ left, top, width }}
      // Keep the text selection alive when the button is pressed.
      onMouseDown={(e) => e.preventDefault()}
      onClick={onAsk}
      className="fixed z-50 flex items-center justify-center gap-1.5 rounded-full bg-indigo-600 px-3 py-2 text-sm font-medium text-white shadow-lg shadow-indigo-900/20 transition hover:bg-indigo-500 focus:outline-none focus-visible:ring-2 focus-visible:ring-indigo-300"
    >
      <SparkIcon width={15} height={15} />
      Ask about this
    </button>
  );
}

/** Make markdown + LaTeX answers sound reasonable when read aloud. */
export function markdownToSpeech(md: string): string {
  return md
    .replace(/```[\s\S]*?```/g, " (code omitted) ")
    .replace(/\$\$[\s\S]*?\$\$/g, " (equation shown on screen) ")
    .replace(/\$([^$\n]+?)\$/g, (_m, tex: string) =>
      tex
        .replace(/\\([a-zA-Z]+)/g, " $1 ")
        .replace(/[{}^_\\]/g, " ")
        .replace(/\s+/g, " "),
    )
    .replace(/`([^`]*)`/g, "$1")
    .replace(/!\[[^\]]*\]\([^)]*\)/g, "")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/^\s{0,3}#{1,6}\s+/gm, "")
    .replace(/^\s*[-*+]\s+/gm, "")
    .replace(/^\s*>\s?/gm, "")
    .replace(/[*_~|]+/g, "")
    .replace(/\bpp\.\s*/g, "pages ")
    .replace(/\bp\.\s*(?=\d)/g, "page ")
    .replace(/\n{2,}/g, ". ")
    .replace(/\s+/g, " ")
    .trim();
}

/** Split into sentence-sized chunks; long utterances get cut off in Chrome. */
export function chunkForSpeech(text: string, max = 220): string[] {
  const sentences = text.match(/[^.!?]+[.!?]+["')\]]*\s*|[^.!?]+$/g) ?? [text];
  const chunks: string[] = [];
  let current = "";
  for (const s of sentences) {
    if ((current + s).length > max && current) {
      chunks.push(current.trim());
      current = "";
    }
    current += s;
  }
  if (current.trim()) chunks.push(current.trim());
  return chunks;
}

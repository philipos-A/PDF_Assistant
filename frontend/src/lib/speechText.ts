/** Make markdown + LaTeX answers sound reasonable when read aloud. */
export function markdownToSpeech(md: string): string {
  return md
    .replace(/\[\d+\]\(#cite-\d+\)/g, "") // citation markers
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
    .replace(/([.!?:;])\s*\n{2,}/g, "$1 ") // a paragraph that already ends a sentence
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

/** One chunk to speak, with the Citations its sentences rely on. */
export interface SpeechSegment {
  text: string;
  /** Citation numbers (`n`) whose markers fall in this chunk, in order. */
  cites: number[];
}

// Private-use characters carry citation markers through the cleanup above.
const OPEN = "\uE000";
const CLOSE = "\uE001";
const TOKEN = /\uE000(\d+)\uE001/g;

/**
 * An Answer as plain speakable text, with each citation marker kept as a token.
 * The backend writes a marker just before the text it supports, so a token
 * belongs to the sentence that follows it.
 */
export function speechSource(markdown: string): string {
  return markdownToSpeech(markdown.replace(/\[(\d+)\]\(#cite-\d+\)/g, (_m, n: string) => ` ${OPEN}${n}${CLOSE}`));
}

/** Length of `text` up to and including its last finished sentence (0 if none is finished yet). */
export function finishedSentencesLength(text: string): number {
  let end = 0;
  for (const m of text.matchAll(/[.!?]["')\]]*\s+/g)) end = m.index + m[0].length;
  return end;
}

/** Split speech source text into chunks tagged with their Citations. */
export function segmentSpeech(source: string, max = 220): SpeechSegment[] {
  const segments: SpeechSegment[] = [];
  for (const chunk of chunkForSpeech(source.trim(), max)) {
    const cites = [...chunk.matchAll(TOKEN)].map((m) => Number(m[1]));
    const clean = chunk
      .replace(TOKEN, "")
      .replace(/\s+([.,!?;:])/g, "$1")
      .replace(/\s+/g, " ")
      .trim();
    if (/[\p{L}\p{N}]/u.test(clean)) segments.push({ text: clean, cites });
    else if (segments.length) segments[segments.length - 1].cites.push(...cites);
  }
  return segments;
}

/**
 * Speakable chunks of an Answer, each tagged with the Citations it mentions, so
 * the cited passage can be highlighted while that chunk is spoken.
 */
export function speechSegments(markdown: string, max = 220): SpeechSegment[] {
  return segmentSpeech(speechSource(markdown), max);
}

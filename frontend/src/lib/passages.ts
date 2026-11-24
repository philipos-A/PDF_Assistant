import type { ChatMessage, Citation } from "../types";
import { normalizeText } from "./textMatch";

/** An Answer about a Marked Passage, with the Question that asked for it. */
export interface PassageAnswer {
  question: ChatMessage;
  answer: ChatMessage;
}

/**
 * A passage of the Document that a Selection or Citation in the Conversation
 * points to. It stays marked in the viewer; clicking it shows its Answers.
 */
export interface MarkedPassage {
  key: string;
  /** "asked": the Reader selected it and asked; "cited": an Answer cited it. */
  kind: "asked" | "cited";
  text: string;
  startPage: number;
  endPage: number;
  answers: PassageAnswer[];
}

/** Same passage on the same Page → same key, however its whitespace or case differ. */
export function passageKey(page: number, text: string): string {
  return `${page}:${normalizeText(text).slice(0, 160)}`;
}

export function citationKey(c: Citation): string {
  return passageKey(c.start_page, c.cited_text);
}

/** Every Marked Passage in a Conversation, in the order first mentioned. */
export function buildPassages(messages: ChatMessage[]): MarkedPassage[] {
  const byKey = new Map<string, MarkedPassage>();
  const add = (p: Omit<MarkedPassage, "key" | "answers">, key: string, entry: PassageAnswer) => {
    let passage = byKey.get(key);
    if (!passage) byKey.set(key, (passage = { ...p, key, answers: [] }));
    if (p.kind === "asked") passage.kind = "asked";
    if (!passage.answers.some((a) => a.answer.id === entry.answer.id)) passage.answers.push(entry);
  };

  let question: ChatMessage | null = null;
  for (const m of messages) {
    if (m.role === "user") {
      question = m;
      continue;
    }
    if (!question || m.status !== "done" || !m.content) continue;
    const entry = { question, answer: m };
    const selected = question.context?.selected_text;
    if (selected) {
      const page = question.context?.selection_page ?? question.context?.current_page ?? question.page;
      add({ kind: "asked", text: selected, startPage: page, endPage: page }, passageKey(page, selected), entry);
    }
    for (const c of m.citations ?? []) {
      add(
        { kind: "cited", text: c.cited_text, startPage: c.start_page, endPage: Math.max(c.start_page, c.end_page) },
        citationKey(c),
        entry,
      );
    }
  }
  return [...byKey.values()];
}

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// jsdom has no Worker or AudioContext, so the player uses the browser voice; stub that.
const spoken: string[] = [];

class FakeUtterance {
  onend: (() => void) | null = null;
  onerror: (() => void) | null = null;
  lang = "";
  constructor(public text: string) {}
}

async function loadVoice() {
  vi.resetModules();
  return (await import("./voice")).voice;
}

const sentence = (word: string) => `${word} ${"is a fairly long sentence about the paper ".repeat(2).trim()}.`;
/** Let the player's promise chain run. */
const settle = () => new Promise((r) => setTimeout(r, 0));

describe("voice (browser fallback)", () => {
  beforeEach(() => {
    spoken.length = 0;
    vi.stubGlobal("SpeechSynthesisUtterance", FakeUtterance);
    vi.stubGlobal("speechSynthesis", {
      speak: (u: FakeUtterance) => {
        spoken.push(u.text);
        setTimeout(() => u.onend?.(), 0);
      },
      cancel: vi.fn(),
    });
  });
  afterEach(() => vi.unstubAllGlobals());

  it("speaks finished chunks while streaming and the rest on finish", async () => {
    const voice = await loadVoice();
    const reading = voice.begin("a1");
    reading.update(`${sentence("One")} ${sentence("Two")} Three is still being wri`);
    for (let i = 0; i < 6; i++) await settle();
    expect(spoken).toEqual([sentence("One"), sentence("Two")]);

    reading.finish(`${sentence("One")} ${sentence("Two")} Three is done.`);
    for (let i = 0; i < 6; i++) await settle();
    expect(spoken.at(-1)).toBe("Three is done.");
    expect(voice.getSnapshot()).toMatchObject({ status: "idle", readingId: null });
  });

  it("reports each chunk's citations as it starts", async () => {
    const voice = await loadVoice();
    const cites: number[][] = [];
    voice.speak("a1", `[1](#cite-1)${sentence("One")} [2](#cite-2)${sentence("Two")}`, (s) => cites.push(s.cites));
    for (let i = 0; i < 6; i++) await settle();
    expect(cites).toEqual([[1], [2]]);
  });

  it("stops speaking after cancel", async () => {
    const voice = await loadVoice();
    voice.speak("a1", `${sentence("One")} ${sentence("Two")}`);
    await settle();
    voice.cancel();
    for (let i = 0; i < 6; i++) await settle();
    expect(spoken).toEqual([sentence("One")]);
    expect(voice.getSnapshot().status).toBe("idle");
  });
});

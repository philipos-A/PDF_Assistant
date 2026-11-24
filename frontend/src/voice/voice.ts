/**
 * Reads Answers aloud with the Kokoro voice (see kokoro.worker.ts), falling back
 * to the browser's built-in voice if Kokoro can't run. Speech starts on the first
 * finished sentence while the Answer is still streaming, and each spoken chunk
 * reports the Citations it relies on so their passages can be highlighted in time.
 */
import { finishedSentencesLength, type SpeechSegment, segmentSpeech, speechSource } from "../lib/speechText";
import type { WorkerRequest, WorkerResponse } from "./kokoro.worker";

const KOKORO_VOICE = "af_heart";
/** Chunks generated ahead of the one playing, so there's no gap between sentences. */
const LOOKAHEAD = 2;
/** Characters per spoken chunk: short enough that speech starts quickly and highlights track the sentence. */
const CHUNK_CHARS = 160;

export type VoiceStatus = "idle" | "loading" | "speaking";

export interface VoiceSnapshot {
  status: VoiceStatus;
  /** Share of the voice model downloaded (0–1) while it loads for the first time. */
  progress: number | null;
  /** Id of the Answer being read. */
  readingId: string | null;
}

export interface Reading {
  /** Pass the Answer so far; finished sentences are queued for speaking. */
  update(markdown: string): void;
  /** The Answer is complete: speak whatever is left. */
  finish(markdown?: string): void;
}

type SegmentListener = (segment: SpeechSegment) => void;

const hasKokoro = () => typeof Worker !== "undefined" && typeof AudioContext !== "undefined";
const hasBrowserVoice = () => typeof window !== "undefined" && "speechSynthesis" in window;

class Voice {
  readonly supported = hasKokoro() || hasBrowserVoice();
  private snapshot: VoiceSnapshot = { status: "idle", progress: null, readingId: null };
  private listeners = new Set<() => void>();

  private worker: Worker | null = null;
  private kokoroFailed = !hasKokoro();
  private requests = new Map<number, { resolve: (b: AudioBuffer) => void; reject: (e: Error) => void }>();
  private nextRequest = 1;
  private ctx: AudioContext | null = null;
  private source: AudioBufferSourceNode | null = null;
  /** Bumped on every begin/cancel; a stale reading loop stops when it sees a newer one. */
  private generation = 0;
  private wake: (() => void) | null = null;

  subscribe = (fn: () => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };
  getSnapshot = () => this.snapshot;

  private set(patch: Partial<VoiceSnapshot>) {
    this.snapshot = { ...this.snapshot, ...patch };
    this.listeners.forEach((fn) => fn());
  }

  /** Start downloading the model and unlock audio. Call from a click so the browser allows sound. */
  preload() {
    if (this.kokoroFailed) return;
    this.audioContext();
    this.ensureWorker()?.postMessage({ type: "load" } satisfies WorkerRequest);
  }

  private audioContext(): AudioContext {
    this.ctx ??= new AudioContext();
    if (this.ctx.state === "suspended") void this.ctx.resume();
    return this.ctx;
  }

  private ensureWorker(): Worker | null {
    if (this.kokoroFailed) return null;
    if (this.worker) return this.worker;
    try {
      this.worker = new Worker(new URL("./kokoro.worker.ts", import.meta.url), { type: "module" });
    } catch {
      this.kokoroFailed = true;
      return null;
    }
    this.worker.onmessage = (e: MessageEvent<WorkerResponse>) => {
      const msg = e.data;
      if (msg.type === "progress") {
        if (msg.total > 0) this.set({ progress: msg.loaded / msg.total });
      } else if (msg.type === "ready") {
        this.set({ progress: null });
      } else if (msg.type === "audio") {
        const req = this.requests.get(msg.id);
        this.requests.delete(msg.id);
        if (!req) return;
        const buffer = this.audioContext().createBuffer(1, msg.samples.length, msg.sampleRate);
        buffer.copyToChannel(msg.samples as Float32Array<ArrayBuffer>, 0);
        req.resolve(buffer);
      } else if (msg.type === "error") {
        if (msg.id === undefined) this.failKokoro(msg.message);
        else {
          this.requests.get(msg.id)?.reject(new Error(msg.message));
          this.requests.delete(msg.id);
        }
      }
    };
    this.worker.onerror = (e) => this.failKokoro(e.message || "voice worker crashed");
    return this.worker;
  }

  /** Kokoro can't run here: use the browser's voice from now on. */
  private failKokoro(message: string) {
    console.warn(`Kokoro voice unavailable, using the browser voice: ${message}`);
    this.kokoroFailed = true;
    this.worker?.terminate();
    this.worker = null;
    for (const req of this.requests.values()) req.reject(new Error(message));
    this.requests.clear();
    this.set({ progress: null });
  }

  private generate(text: string): Promise<AudioBuffer> {
    const worker = this.ensureWorker();
    if (!worker) return Promise.reject(new Error("Kokoro unavailable"));
    const id = this.nextRequest++;
    return new Promise((resolve, reject) => {
      this.requests.set(id, { resolve, reject });
      worker.postMessage({ type: "generate", id, text, voice: KOKORO_VOICE } satisfies WorkerRequest);
    });
  }

  private play(buffer: AudioBuffer): Promise<void> {
    const ctx = this.audioContext();
    return new Promise((resolve) => {
      const source = ctx.createBufferSource();
      source.buffer = buffer;
      source.connect(ctx.destination);
      source.onended = () => {
        if (this.source === source) this.source = null;
        resolve();
      };
      this.source = source;
      source.start();
    });
  }

  private speakWithBrowser(text: string, isCurrent: () => boolean): Promise<void> {
    if (!hasBrowserVoice()) return Promise.resolve();
    return new Promise((resolve) => {
      const u = new SpeechSynthesisUtterance(text);
      u.lang = navigator.language || "en-US";
      u.onend = () => resolve();
      u.onerror = () => resolve();
      if (isCurrent()) window.speechSynthesis.speak(u);
      else resolve();
    });
  }

  /** Start reading Answer `id`; feed it with update()/finish(). Stops any other reading. */
  begin(id: string, onSegment?: SegmentListener): Reading {
    this.cancel();
    if (!this.supported) return { update: () => undefined, finish: () => undefined };
    const gen = this.generation;
    const isCurrent = () => this.generation === gen;
    const segments: SpeechSegment[] = [];
    const audio: (Promise<AudioBuffer> | undefined)[] = [];
    /** How much of the Answer's speech text has been queued, in characters. */
    let consumed = 0;
    let finished = false;
    let latest = "";

    // Queue every finished sentence not queued yet; on finish, the rest too. Earlier text
    // doesn't change as the Answer streams on, so a character offset marks the place.
    const accept = (markdown: string, all: boolean) => {
      if (!isCurrent() || finished) return;
      latest = markdown;
      const rest = speechSource(markdown).slice(consumed);
      const end = all ? rest.length : finishedSentencesLength(rest);
      if (end > 0) {
        segments.push(...segmentSpeech(rest.slice(0, end), CHUNK_CHARS));
        consumed += end;
      }
      if (all) finished = true;
      this.wake?.();
    };

    const requestAudio = (i: number) => {
      if (this.kokoroFailed || i >= segments.length || audio[i]) return;
      const pending = this.generate(segments[i].text);
      pending.catch(() => undefined); // handled where awaited
      audio[i] = pending;
    };

    this.set({ status: this.kokoroFailed ? "speaking" : "loading", readingId: id });

    const run = async () => {
      for (let i = 0; isCurrent();) {
        if (i >= segments.length) {
          if (finished) break;
          await new Promise<void>((r) => (this.wake = r));
          continue;
        }
        for (let k = i; k <= i + LOOKAHEAD; k++) requestAudio(k);
        let buffer: AudioBuffer | null = null;
        if (!this.kokoroFailed) {
          try {
            buffer = (await audio[i]) ?? null;
          } catch {
            /* fall through to the browser voice for this chunk */
          }
        }
        if (!isCurrent()) return;
        this.set({ status: "speaking" });
        onSegment?.(segments[i]);
        if (buffer) await this.play(buffer);
        else await this.speakWithBrowser(segments[i].text, isCurrent);
        i++;
      }
      if (isCurrent()) this.set({ status: "idle", readingId: null });
    };
    void run();

    return { update: (md) => accept(md, false), finish: (md) => accept(md ?? latest, true) };
  }

  /** Read a finished Answer. */
  speak(id: string, markdown: string, onSegment?: SegmentListener) {
    this.begin(id, onSegment).finish(markdown);
  }

  cancel() {
    this.generation++;
    this.wake?.();
    this.wake = null;
    if (this.source) {
      try {
        this.source.stop();
      } catch {
        /* already stopped */
      }
      this.source = null;
    }
    // Drop queued generations so the next reading isn't stuck behind them.
    if (this.requests.size) {
      for (const req of this.requests.values()) req.reject(new Error("cancelled"));
      this.requests.clear();
      this.worker?.postMessage({ type: "cancel" } satisfies WorkerRequest);
    }
    if (hasBrowserVoice()) window.speechSynthesis.cancel();
    if (this.snapshot.status !== "idle") this.set({ status: "idle", readingId: null });
  }
}

export const voice = new Voice();

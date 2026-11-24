/// <reference lib="webworker" />
/**
 * Runs the Kokoro text-to-speech model off the main thread, so the PDF and the
 * highlighter animations stay smooth while speech is generated. The model is
 * downloaded from Hugging Face on first use and then cached by the browser.
 */
import { KokoroTTS } from "kokoro-js";

const MODEL = "onnx-community/Kokoro-82M-v1.0-ONNX";

export type WorkerRequest =
  | { type: "load" }
  | { type: "generate"; id: number; text: string; voice: string }
  /** Drop every queued generation (the reading was stopped). */
  | { type: "cancel" };
export type WorkerResponse =
  | { type: "progress"; loaded: number; total: number }
  | { type: "ready"; device: string }
  | { type: "audio"; id: number; samples: Float32Array; sampleRate: number }
  | { type: "error"; id?: number; message: string };

const post = (msg: WorkerResponse, transfer: Transferable[] = []) => self.postMessage(msg, { transfer });

let model: Promise<KokoroTTS> | null = null;

async function load(): Promise<KokoroTTS> {
  // WebGPU is several times faster than WebAssembly; it needs the full-precision weights.
  const gpu = (navigator as Navigator & { gpu?: { requestAdapter(): Promise<unknown> } }).gpu;
  const adapter = gpu ? await gpu.requestAdapter().catch(() => null) : null;
  const device = adapter ? "webgpu" : "wasm";
  const files = new Map<string, { loaded: number; total: number }>();
  const tts = await KokoroTTS.from_pretrained(MODEL, {
    dtype: adapter ? "fp32" : "q8",
    device,
    progress_callback: (p) => {
      if (p.status !== "progress") return;
      files.set(p.file, { loaded: p.loaded, total: p.total });
      let loaded = 0;
      let total = 0;
      for (const f of files.values()) {
        loaded += f.loaded;
        total += f.total;
      }
      post({ type: "progress", loaded, total });
    },
  });
  post({ type: "ready", device });
  return tts;
}

function ensureModel(): Promise<KokoroTTS> {
  model ??= load().catch((e) => {
    model = null;
    throw e;
  });
  return model;
}

// One generation at a time: the model is not safe to run concurrently.
let queue: Promise<void> = Promise.resolve();
/** Bumped by "cancel"; queued generations from an older epoch are skipped. */
let epoch = 0;

self.onmessage = (e: MessageEvent<WorkerRequest>) => {
  const msg = e.data;
  if (msg.type === "load") {
    ensureModel().catch((err: Error) => post({ type: "error", message: err.message }));
    return;
  }
  if (msg.type === "cancel") {
    epoch++;
    return;
  }
  const queuedIn = epoch;
  queue = queue.then(async () => {
    if (queuedIn !== epoch) {
      post({ type: "error", id: msg.id, message: "cancelled" });
      return;
    }
    try {
      const tts = await ensureModel();
      const audio = await tts.generate(msg.text, { voice: msg.voice as never });
      const samples = audio.audio as Float32Array;
      post({ type: "audio", id: msg.id, samples, sampleRate: audio.sampling_rate }, [samples.buffer]);
    } catch (err) {
      post({ type: "error", id: msg.id, message: (err as Error).message });
    }
  });
};

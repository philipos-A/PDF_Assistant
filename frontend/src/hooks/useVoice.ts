import { useSyncExternalStore } from "react";
import { voice } from "../voice/voice";

/** The shared read-aloud voice and its state (idle, loading the model, or speaking). */
export function useVoice() {
  const state = useSyncExternalStore(voice.subscribe, voice.getSnapshot);
  return { ...state, voice };
}

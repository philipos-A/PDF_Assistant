import { useCallback, useEffect, useRef, useState } from "react";
import { chunkForSpeech, markdownToSpeech } from "../lib/speechText";

export function useSpeechSynthesis() {
  const [supported] = useState(() => typeof window !== "undefined" && "speechSynthesis" in window);
  const [speaking, setSpeaking] = useState(false);
  // Bumped on every speak/cancel. A cancelled utterance's onend/onerror can fire
  // after the next speak() has started; the check stops it clearing `speaking`.
  const generation = useRef(0);

  const cancel = useCallback(() => {
    if (!supported) return;
    generation.current++;
    window.speechSynthesis.cancel();
    setSpeaking(false);
  }, [supported]);

  const speak = useCallback(
    (markdown: string) => {
      if (!supported) return;
      const gen = ++generation.current;
      window.speechSynthesis.cancel();
      const chunks = chunkForSpeech(markdownToSpeech(markdown));
      if (!chunks.length) {
        setSpeaking(false);
        return;
      }
      const finish = () => {
        if (generation.current === gen) setSpeaking(false);
      };
      chunks.forEach((chunk, i) => {
        const u = new SpeechSynthesisUtterance(chunk);
        u.lang = navigator.language || "en-US";
        if (i === chunks.length - 1) {
          u.onend = finish;
          u.onerror = finish;
        }
        window.speechSynthesis.speak(u);
      });
      setSpeaking(true);
    },
    [supported],
  );

  useEffect(() => () => {
    if (supported) window.speechSynthesis.cancel();
  }, [supported]);

  return { supported, speaking, speak, cancel };
}

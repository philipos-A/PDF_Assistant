import { useCallback, useEffect, useState } from "react";
import { chunkForSpeech, markdownToSpeech } from "../lib/speechText";

export function useSpeechSynthesis() {
  const [supported] = useState(() => typeof window !== "undefined" && "speechSynthesis" in window);
  const [speaking, setSpeaking] = useState(false);

  const cancel = useCallback(() => {
    if (!supported) return;
    window.speechSynthesis.cancel();
    setSpeaking(false);
  }, [supported]);

  const speak = useCallback(
    (markdown: string) => {
      if (!supported) return;
      window.speechSynthesis.cancel();
      const chunks = chunkForSpeech(markdownToSpeech(markdown));
      if (!chunks.length) return;
      chunks.forEach((chunk, i) => {
        const u = new SpeechSynthesisUtterance(chunk);
        u.lang = navigator.language || "en-US";
        if (i === chunks.length - 1) {
          u.onend = () => setSpeaking(false);
          u.onerror = () => setSpeaking(false);
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

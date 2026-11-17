import { useRef } from "react";
import { MicIcon, MicOffIcon } from "./icons";

interface Props {
  supported: boolean;
  listening: boolean;
  disabled?: boolean;
  onStart: () => void;
  onStop: () => void;
}

const HOLD_MS = 350;

/**
 * Push-to-talk: hold to speak, release to send. A quick tap toggles
 * hands-free listening instead (tap again to stop).
 */
export function MicButton({ supported, listening, disabled, onStart, onStop }: Props) {
  const downAt = useRef(0);
  const startedHere = useRef(false);

  if (!supported) {
    return (
      <span
        className="grid h-10 w-10 shrink-0 cursor-help place-items-center rounded-full text-slate-300 dark:text-slate-600"
        title="Voice input isn't supported in this browser. Try Chrome, Edge or Safari."
        aria-label="Voice input not supported"
      >
        <MicOffIcon />
      </span>
    );
  }

  return (
    <button
      type="button"
      disabled={disabled}
      aria-pressed={listening}
      aria-label={listening ? "Stop listening" : "Hold to talk"}
      title={listening ? "Release (or tap) to send" : "Hold to talk, or tap for hands-free"}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.currentTarget.setPointerCapture(e.pointerId);
        if (listening) {
          startedHere.current = false;
          onStop();
          return;
        }
        downAt.current = Date.now();
        startedHere.current = true;
        onStart();
      }}
      onPointerUp={() => {
        if (startedHere.current && Date.now() - downAt.current > HOLD_MS) onStop();
        startedHere.current = false;
      }}
      onKeyDown={(e) => {
        if ((e.key === " " || e.key === "Enter") && !e.repeat) {
          e.preventDefault();
          if (listening) onStop();
          else onStart();
        }
      }}
      className={`relative grid h-10 w-10 shrink-0 touch-none select-none place-items-center rounded-full transition disabled:opacity-40 ${
        listening
          ? "bg-red-500 text-white shadow-lg shadow-red-500/30"
          : "text-slate-500 hover:bg-slate-100 hover:text-slate-700 dark:hover:bg-slate-800 dark:hover:text-slate-200"
      }`}
    >
      {listening && <span className="absolute inset-0 animate-ping rounded-full bg-red-400/40" />}
      <MicIcon className="relative" />
    </button>
  );
}

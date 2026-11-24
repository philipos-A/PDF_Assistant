/** Small animated equalizer shown while an Answer is being read aloud. */
export function SpeakingBars({ className = "" }: { className?: string }) {
  return (
    <span className={`inline-flex h-3 items-end gap-[2px] ${className}`} aria-hidden="true">
      {[0, 180, 90, 270].map((delay) => (
        <span
          key={delay}
          className="speaking-bar w-[3px] rounded-full bg-current"
          style={{ animationDelay: `${delay}ms` }}
        />
      ))}
    </span>
  );
}

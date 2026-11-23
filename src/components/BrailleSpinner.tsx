import { useEffect, useState } from 'react';

/** The frames, in order. Each is one braille cell, so the spinner keeps its width. */
export const BRAILLE_FRAMES = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
/** The one frame shown when the user asks for reduced motion. */
export const BRAILLE_STILL = '⠶';
export const BRAILLE_INTERVAL_MS = 80;

/** The frame for a tick. Reduced motion always shows the still frame. */
export function brailleFrame(tick: number, reducedMotion: boolean): string {
  if (reducedMotion) {
    return BRAILLE_STILL;
  }
  const n = BRAILLE_FRAMES.length;
  return BRAILLE_FRAMES[((tick % n) + n) % n];
}

function prefersReducedMotion(): boolean {
  return typeof window !== 'undefined' && typeof window.matchMedia === 'function' && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

/** A braille spinner. It turns every BRAILLE_INTERVAL_MS, and stops (no timer) under reduced motion. Decorative: aria-hidden. */
export function BrailleSpinner() {
  const [reduced] = useState(prefersReducedMotion);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    if (reduced) {
      return;
    }
    const id = setInterval(() => setTick((t) => t + 1), BRAILLE_INTERVAL_MS);
    return () => clearInterval(id);
  }, [reduced]);
  return (
    <span className="braille-spinner" aria-hidden="true">
      {brailleFrame(tick, reduced)}
    </span>
  );
}

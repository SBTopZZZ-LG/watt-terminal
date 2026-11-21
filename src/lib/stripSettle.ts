// What the status strip shows of the input state. Every agent command passes submitted -> running -> prompt, so showing
// each step at once makes the strip flicker on every tool call. Leaving the prompt is therefore shown only after the
// strip has held the prompt for STRIP_SETTLE_MS; a command that ends sooner never shows. Every other change shows at once.
// The input logic never reads this: App.tsx feeds the settled view to the strip only.

import { useEffect, useRef, useState } from 'react';
import { sameForeground } from './inputState';
import type { Foreground, InputState } from './types';

/** How long the strip keeps showing the prompt after a command starts, before it shows the command. */
export const STRIP_SETTLE_MS = 400;

export interface StripView {
  state: InputState;
  foreground: Foreground | null;
}

/** The states a command passes through between the prompt and the prompt. */
const TRANSIENT: ReadonlySet<InputState> = new Set<InputState>(['submitted', 'running']);

export function sameStripView(a: StripView, b: StripView): boolean {
  return a.state === b.state && sameForeground(a.foreground, b.foreground);
}

/** True when the strip shows the prompt and the incoming view is a command starting, so the strip waits before it changes. */
export function holdsStrip(shown: StripView, incoming: StripView): boolean {
  return shown.state === 'prompt' && TRANSIENT.has(incoming.state);
}

/**
 * What the strip shows now. `heldMs` is the time since the strip started holding the prompt for the incoming command
 * (0 when it is not holding). The strip returns to the prompt at once; it shows the command only once `heldMs` reaches
 * STRIP_SETTLE_MS. A change between two non-prompt states shows at once.
 */
export function settleStrip(shown: StripView, incoming: StripView, heldMs: number): StripView {
  if (holdsStrip(shown, incoming) && heldMs < STRIP_SETTLE_MS) {
    return shown;
  }
  return incoming;
}

export interface StripSettle {
  /** What the strip shows. */
  shown: StripView;
  /** When the strip began holding the prompt for the incoming command, or null when it is not holding. */
  heldSince: number | null;
}

/**
 * One step of the settle: the incoming view at time `now` (ms). `recheckMs` is the time until the held view should be
 * re-checked (the caller sets a timer for it), or null when there is nothing to wait for.
 */
export function stepStrip(
  prev: StripSettle,
  incoming: StripView,
  now: number,
): { next: StripSettle; recheckMs: number | null } {
  const holding = holdsStrip(prev.shown, incoming);
  const heldSince = holding ? (prev.heldSince ?? now) : null;
  const heldMs = heldSince === null ? 0 : now - heldSince;
  const shown = settleStrip(prev.shown, incoming, heldMs);
  const recheckMs = holding && shown === prev.shown ? STRIP_SETTLE_MS - heldMs : null;
  return { next: { shown, heldSince }, recheckMs };
}

/** The strip's settled view of the input state and foreground hint. Re-renders when a held change comes due. */
export function useSettledStrip(state: InputState, foreground: Foreground | null): StripView {
  const [shown, setShown] = useState<StripView>({ state, foreground });
  const heldSince = useRef<number | null>(null);

  useEffect(() => {
    const incoming: StripView = { state, foreground };
    const { next, recheckMs } = stepStrip({ shown, heldSince: heldSince.current }, incoming, Date.now());
    heldSince.current = next.heldSince;
    if (!sameStripView(next.shown, shown)) {
      setShown(next.shown);
      return;
    }
    if (recheckMs === null) {
      return;
    }
    // The held time is up: the incoming view is what the strip shows. The effect re-runs when it changes, so the
    // timer always carries the latest incoming view.
    const timer = setTimeout(() => setShown(incoming), recheckMs);
    return () => clearTimeout(timer);
  }, [state, foreground, shown]);

  return shown;
}

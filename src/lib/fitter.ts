// Fits the terminal to its container, one fit at a time. Pure: the terminal, the fit addon and the PTY are injected,
// so the serialization is tested without xterm. terminal.ts wires the real ones.

export interface Size {
  rows: number;
  cols: number;
}

export interface FitDeps {
  /** False before the session has started. There is no PTY to resize yet. */
  connected(): boolean;
  /** The size the container would fit, or null when it cannot be measured. */
  proposeSize(): Size | null;
  /** The size xterm has now. */
  currentSize(): Size;
  /** Resizes the PTY (Rust resizes the PTY and the observer together). Rejects if Rust refused. */
  resizePty(size: Size): Promise<void>;
  /** Resizes xterm. Called only after resizePty succeeded, so xterm never gets ahead of the PTY. */
  resizeTerminal(size: Size): void;
  onError(e: unknown): void;
}

/** Re-proposals per call. A container that keeps changing stops here; the next ResizeObserver tick carries on. */
export const MAX_FIT_PASSES = 5;

function sameSize(a: Size, b: Size): boolean {
  return a.rows === b.rows && a.cols === b.cols;
}

/**
 * Returns the fit function. Calls run one after another: a second call waits for the first to finish its IPC, so it
 * never compares against a size that is about to change. After each completed resize the size is proposed again,
 * and the loop runs until xterm matches the container or MAX_FIT_PASSES is reached. If the PTY resize fails, xterm is
 * left alone, the error is reported, and the call ends. A later call (the next tick) tries again.
 */
export function createFitter(deps: FitDeps): () => Promise<void> {
  let queue: Promise<void> = Promise.resolve();

  const fitLoop = async (): Promise<void> => {
    if (!deps.connected()) {
      return;
    }
    for (let pass = 0; pass < MAX_FIT_PASSES; pass++) {
      const size = deps.proposeSize();
      if (!size || sameSize(size, deps.currentSize())) {
        return;
      }
      try {
        await deps.resizePty(size);
      } catch (e) {
        deps.onError(e);
        return;
      }
      deps.resizeTerminal(size);
    }
  };

  return () => {
    const run = queue.then(fitLoop);
    // A failed run must not block the calls queued behind it.
    queue = run.catch(() => undefined);
    return run;
  };
}

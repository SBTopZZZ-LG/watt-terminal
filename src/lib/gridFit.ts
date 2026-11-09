// Safety net for a grid that is wider than its container. Pure: the measurement and the refit are injected, so the
// rule is tested without a DOM. TerminalView wires the real ones.

export interface GridMeasure {
  /** Right edge of .xterm-screen, in CSS px from the viewport's left. */
  screenRight: number;
  /** Right edge of the host box (.term-host), same origin. */
  hostRight: number;
  /** Right padding of .xterm, which the grid may not enter. */
  padRight: number;
}

/** True when the grid reaches past the host's right padding. The 14px scrollbar allowance counts as spare space. */
export function overflowsHost(screenRight: number, hostRight: number, padRight: number): boolean {
  return screenRight > hostRight - padRight;
}

/** Refits in a row, without a clean check in between, before the guard gives up. */
export const MAX_SELF_CHECK_REFITS = 3;

export interface OverflowGuardDeps {
  /** Null when the terminal is not on screen yet. Nothing is checked then. */
  measure(): GridMeasure | null;
  /** Runs a fit and resolves when it is done. */
  refit(): Promise<void>;
  /** Called once per streak when the cap is reached and the grid still overflows. */
  onGiveUp(m: GridMeasure): void;
}

/**
 * Returns the check. A clean check resets the streak. An overflowing check refits and checks again, until the grid
 * fits or MAX_SELF_CHECK_REFITS refits have not fixed it. Then it gives up until a later check comes back clean.
 * The loop is bounded by the cap, and a refit that changes nothing cannot re-arm it.
 */
export function createOverflowGuard(deps: OverflowGuardDeps): () => Promise<void> {
  let refits = 0;
  let gaveUp = false;

  return async () => {
    for (;;) {
      const m = deps.measure();
      if (m === null || !overflowsHost(m.screenRight, m.hostRight, m.padRight)) {
        refits = 0;
        gaveUp = false;
        return;
      }
      if (refits >= MAX_SELF_CHECK_REFITS) {
        if (!gaveUp) {
          gaveUp = true;
          deps.onGiveUp(m);
        }
        return;
      }
      refits += 1;
      await deps.refit();
    }
  };
}

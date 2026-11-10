// Pure geometry for the command stripes: which part of each command lies in the viewport. No DOM, no xterm, so it
// is tested directly. Rows are absolute buffer rows (0 is the top of the scrollback), the same numbering xterm's
// markers use.

/** How a command ended, as the stripe shows it. `running` is drawn while the command has not ended. */
export type StripeExit = 'running' | 'ok' | 'fail' | 'unknown';

export interface StripeMark {
  /** First row of the command. -1 when the start has scrolled out of the scrollback. */
  start: number;
  /** Last row of the command, inclusive. */
  end: number;
  exit: StripeExit;
  agent: boolean;
}

export interface StripeSegment {
  /** First row of the stripe, relative to the top of the viewport. */
  firstRow: number;
  rowCount: number;
  exit: StripeExit;
  agent: boolean;
  /** True when the command's first row is in the viewport. The agent badge is shown only then. */
  startVisible: boolean;
}

/**
 * The part of each mark that lies in the viewport, which covers rows viewportY to viewportY + rows - 1. A mark
 * with no row in the viewport gives no segment. The stripe is cut at the viewport edges, so it stays visible while
 * any of its rows is on screen.
 */
export function stripeSegments(marks: readonly StripeMark[], viewportY: number, rows: number): StripeSegment[] {
  const bottom = viewportY + rows - 1;
  const segments: StripeSegment[] = [];
  for (const mark of marks) {
    const first = Math.max(mark.start, viewportY);
    const last = Math.min(mark.end, bottom);
    if (last < first) {
      continue;
    }
    segments.push({
      firstRow: first - viewportY,
      rowCount: last - first + 1,
      exit: mark.exit,
      agent: mark.agent,
      startVisible: mark.start >= viewportY && mark.start <= bottom,
    });
  }
  return segments;
}

/**
 * The viewport-relative row of each absolute row that lies in the viewport, in the order given. Rows outside
 * the viewport give nothing. Used for the separator lines, which are one row each.
 */
export function separatorSegments(absoluteRows: readonly number[], viewportY: number, viewportRows: number): number[] {
  const bottom = viewportY + viewportRows - 1;
  const out: number[] = [];
  for (const row of absoluteRows) {
    if (row >= viewportY && row <= bottom) {
      out.push(row - viewportY);
    }
  }
  return out;
}

/**
 * The top edge in pixels of a 1px separator line on a grid row. When the row is the blank row the shell leaves
 * before a prompt, the line is centred in it. Otherwise the line sits on the top edge of the row.
 */
export function separatorTop(row: number, cellHeight: number, centered: boolean): number {
  const top = row * cellHeight;
  return centered ? top + cellHeight / 2 - 0.5 : top;
}

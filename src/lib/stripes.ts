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
  /** Absolute row of the separator line above the command's prompt, or null when it has none (or it is gone). */
  separatorRow: number | null;
}

export interface StripeSegment {
  /** First row of the stripe, relative to the top of the viewport. */
  firstRow: number;
  rowCount: number;
  exit: StripeExit;
  agent: boolean;
  /** True when the command's first row is in the viewport. The agent badge is shown only then. */
  startVisible: boolean;
  /** Absolute row of the command's separator, as on StripeMark. */
  separatorRow: number | null;
}

/** A separator line. `row` is absolute, or viewport-relative once separatorSegments has run. */
export interface SeparatorLine {
  row: number;
  /** The command whose prompt sits below the line is an agent command. */
  agent: boolean;
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
      separatorRow: mark.separatorRow,
    });
  }
  return segments;
}

/**
 * Whether a command's agent badge sits on its separator line rather than on its stripe. It does when the command
 * has a separator and that row is blank, the usable blank row the shell leaves before the prompt. `isBlank` takes an
 * absolute row, and the check does not depend on the viewport: a separator scrolled out of view still keeps the badge
 * off the stripe, so the badge does not jump between places while scrolling.
 */
export function badgeOnSeparator(separatorRow: number | null, isBlank: (absoluteRow: number) => boolean): boolean {
  return separatorRow !== null && isBlank(separatorRow);
}

/**
 * The separator lines inside the viewport, with `row` made viewport-relative, in the order given. Lines outside
 * the viewport give nothing. Each separator is one row.
 */
export function separatorSegments(
  lines: readonly SeparatorLine[],
  viewportY: number,
  viewportRows: number,
): SeparatorLine[] {
  const bottom = viewportY + viewportRows - 1;
  const out: SeparatorLine[] = [];
  for (const line of lines) {
    if (line.row >= viewportY && line.row <= bottom) {
      out.push({ row: line.row - viewportY, agent: line.agent });
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

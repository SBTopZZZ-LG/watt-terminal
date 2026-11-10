// Gutter marks for each command: a thin stripe (green for exit 0, red otherwise, grey when unknown, muted while it
// runs) and an "agent" badge for commands the agent ran. Positional UI only. Rust records stay the source of truth.
//
// Each C from the shell opens a mark, and each D closes the oldest open mark. The Rust side reports
// RecordStarted in the same order, so the oldest unpaired mark is that record. The badge needs that pairing,
// so it is attached only while the observer has never desynced. After a desync the pairing can drift, and the
// badges already shown stay as they are.
//
// A mark is two xterm markers, one on its first row and one on its last row. The stripes are drawn from them by
// StripeLayer, outside xterm's decoration system, which hides a decoration as soon as its first row scrolls out
// of view. While a command runs, its last row is the cursor row, so the stripe grows with the output.
//
// A separator line marks a prompt that follows a command which ran, so output and the next prompt are visibly apart.
// It is one marker on the row where that prompt's A is emitted. The shell leaves a blank row before such a prompt
// (integration.zsh), so the marker is on that blank row and StripeLayer centres the line in it. Drawn by StripeLayer too.
// The command that starts below a separator keeps it (Mark.separator), so the agent badge can sit on the line.

import type { IMarker, Terminal } from '@xterm/xterm';
import type { SeparatorLine, StripeExit, StripeMark } from './stripes';

/** The part of xterm this module uses. Kept small so the logic can be tested without a DOM. */
export type MarksTerminal = Pick<Terminal, 'buffer' | 'parser' | 'registerMarker'>;

type Exit = number | 'unknown';

interface Mark {
  /** On the command's first row. Undefined when xterm gave no marker (alt screen). */
  start: IMarker | undefined;
  /**
   * On the command's last row. Set when the command finishes. A running command has none, unless a clear cut its
   * rows short (see eraseViewport), and then its end is this marker.
   */
  end: IMarker | undefined;
  /** null while the command runs. */
  exit: Exit | null;
  agent: boolean;
  /** A clear erased the first row. The command has no stripe any more. */
  dropped: boolean;
  /**
   * The separator of the prompt this command ran after, when there was one. Its agent badge is drawn on that line.
   * Undefined for the first command, after an empty Enter, or after a clear.
   */
  separator: IMarker | undefined;
}

export class CommandMarks {
  /** Opened by C, closed by D (or by the next C or A). Oldest first. */
  private open: Mark[] = [];
  /**
   * Marks whose record has not been reported yet, and records reported whose C has not been parsed yet. The
   * two sides can arrive in either order, so whichever is ahead waits here for the other. Oldest first.
   */
  private unpairedMarks: Mark[] = [];
  private unpairedStarts: boolean[] = [];
  private trusted = true;
  /** Every mark that may still have a stripe. Finished marks with no rows left are dropped when stripes are read. */
  private marks: Mark[] = [];
  /**
   * Separator lines: one marker on the row of each prompt that follows a command which ran. Dropped by xterm with
   * their rows, and pruned when read.
   */
  private separatorMarkers: IMarker[] = [];
  /** A command started since the last prompt. An empty Enter, or a Ctrl-L redraw, leaves it false. */
  private commandSincePrompt = false;
  /** The most recently finished mark, which the next prompt's separator belongs to. */
  private lastClosed: Mark | undefined;
  /**
   * The separator of the latest prompt, handed to the next command that starts. Cleared when a prompt follows no
   * separator (an empty Enter, a clear), and kept when a prompt is redrawn in place.
   */
  private promptSeparator: IMarker | undefined;
  /** Absolute row of the latest prompt's A. Tells an in-place redraw (same row) from a new prompt. */
  private lastPromptRow = -1;

  /** `changed` is called when a stripe may have changed, so the layer can repaint. */
  constructor(
    private readonly term: MarksTerminal,
    private readonly changed: () => void = () => undefined,
  ) {
    // Runs before xterm's own handler (it returns false), so the rows and markers are still in place when it runs.
    term.parser.registerCsiHandler({ final: 'J' }, (params) => {
      if (params[0] === 2) {
        this.eraseViewport();
      }
      return false;
    });
  }

  /** Shell event C: a command is starting. */
  commandStart(): void {
    // A C while a command is still open: the earlier command ended without a D we can trust.
    this.closeOpen('unknown');
    this.commandSincePrompt = true;
    const start: IMarker | undefined = this.term.registerMarker(0);
    const mark: Mark = {
      start,
      end: undefined,
      exit: null,
      agent: false,
      dropped: false,
      separator: this.promptSeparator,
    };
    this.marks.push(mark);
    this.open.push(mark);
    const agent = this.unpairedStarts.shift();
    if (agent === undefined) {
      this.unpairedMarks.push(mark);
    } else {
      mark.agent = agent;
    }
    this.changed();
  }

  /** Shell event D: the oldest open command has ended. A D with no open command is ignored. */
  commandEnd(exit: number): void {
    const mark = this.open.shift();
    if (mark) {
      this.finish(mark, exit);
    }
    this.changed();
  }

  /** Shell event A: a new prompt. Any command still open ended without a D. */
  promptStart(): void {
    this.closeOpen('unknown');
    const buf = this.term.buffer.active;
    const row = buf.baseY + buf.cursorY;
    // The row A is emitted on (the blank row, when the shell adds one) gets a separator when a command ran before
    // it and that command still has a stripe. A clear that erased the command's first row leaves no stripe, so no
    // line either.
    let sep: IMarker | undefined;
    if (this.commandSincePrompt && this.lastClosed && this.range(this.lastClosed) !== null) {
      sep = this.term.registerMarker(0);
      if (sep) {
        this.separatorMarkers.push(sep);
      }
    }
    // The next command takes this prompt's separator. With no separator: after a command, the prompt has none (a
    // clear). With no command, a prompt on a new row is an empty Enter, which has none; on the same row it is a
    // redraw, which keeps the separator it had.
    if (sep) {
      this.promptSeparator = sep;
    } else if (this.commandSincePrompt) {
      this.promptSeparator = undefined;
    } else if (row !== this.lastPromptRow) {
      this.promptSeparator = undefined;
    }
    this.lastPromptRow = row;
    this.commandSincePrompt = false;
    this.changed();
  }

  /**
   * The separator lines, pruned of disposed markers, in the order they were made. `agent` is true when the command
   * whose prompt sits below the line is an agent command. Rows are read by StripeLayer on each repaint, and the
   * separators are not drawn in the alternate screen.
   */
  separators(): SeparatorLine[] {
    this.separatorMarkers = this.separatorMarkers.filter((sep) => !sep.isDisposed && sep.line >= 0);
    return this.separatorMarkers.map((sep) => ({
      row: sep.line,
      agent: this.marks.some((mark) => mark.separator === sep && mark.agent),
    }));
  }

  /** Rust reported a record started. `agent` is true when the agent ran it. */
  recordStarted(agent: boolean): void {
    if (!this.trusted) {
      return;
    }
    const mark = this.unpairedMarks.shift();
    if (mark) {
      mark.agent = agent;
      this.changed();
    } else {
      this.unpairedStarts.push(agent);
    }
  }

  /** The observer desynced. Records dropped during the overflow can never be matched, so stop pairing for good. */
  observerDesynced(): void {
    this.trusted = false;
    this.unpairedMarks = [];
    this.unpairedStarts = [];
  }

  /**
   * A new shell session: forget every mark and all pairing, and trust pairing again. The markers are disposed here.
   * xterm's reset() has already cleared the rows, so the stripes go with them.
   */
  reset(): void {
    for (const mark of this.marks) {
      mark.start?.dispose();
      mark.end?.dispose();
    }
    for (const sep of this.separatorMarkers) {
      sep.dispose();
    }
    this.marks = [];
    this.separatorMarkers = [];
    this.commandSincePrompt = false;
    this.lastClosed = undefined;
    this.promptSeparator = undefined;
    this.lastPromptRow = -1;
    this.open = [];
    this.unpairedMarks = [];
    this.unpairedStarts = [];
    this.trusted = true;
    this.changed();
  }

  /**
   * The stripes to draw, as absolute buffer rows. Read by StripeLayer on each repaint. Finished marks that have no
   * rows left are dropped from the list here. `separatorRow` is the row of the mark's separator while that marker
   * is alive, else null.
   */
  stripes(): StripeMark[] {
    this.marks = this.marks.filter((mark) => mark.exit === null || this.range(mark) !== null);
    const stripes: StripeMark[] = [];
    for (const mark of this.marks) {
      const range = this.range(mark);
      if (range) {
        const sep = mark.separator;
        const separatorRow = sep && !sep.isDisposed && sep.line >= 0 ? sep.line : null;
        stripes.push({
          start: range.start,
          end: range.end,
          exit: exitClass(mark.exit),
          agent: mark.agent,
          separatorRow,
        });
      }
    }
    return stripes;
  }

  private closeOpen(exit: Exit): void {
    for (let mark = this.open.shift(); mark; mark = this.open.shift()) {
      this.finish(mark, exit);
    }
  }

  private finish(mark: Mark, exit: Exit): void {
    mark.exit = exit;
    this.lastClosed = mark;
    mark.end?.dispose();
    mark.end = undefined;
    const startRow = this.startRow(mark);
    if (startRow === null) {
      return;
    }
    const buf = this.term.buffer.active;
    // D is emitted before the next prompt is drawn. Output that ends in a newline leaves the cursor at column 0
    // of the next prompt's row, which is not part of the command, so that row is left out. Output with no
    // trailing newline leaves the cursor mid-line on its own row, which is part of the command.
    const cursorRow = buf.baseY + buf.cursorY;
    const endRow = Math.max(this.lastRow(), startRow);
    if (endRow < 0) {
      return;
    }
    mark.end = this.term.registerMarker(endRow - cursorRow);
  }

  /**
   * A clear (ESC[2J) erases the viewport rows in place, and xterm disposes the markers on them. A mark that starts
   * in the viewport goes with them. A mark that starts in the scrollback and reaches into the viewport is cut at
   * the last scrollback row, so no stripe covers the blank rows. Runs before the erase, while the rows and markers
   * are still there.
   */
  private eraseViewport(): void {
    const buf = this.term.buffer.active;
    if (buf.type === 'alternate') {
      return;
    }
    const top = buf.baseY;
    for (const mark of this.marks) {
      const startRow = this.startRow(mark);
      if (startRow === null) {
        continue;
      }
      if (startRow >= top) {
        mark.dropped = true;
        continue;
      }
      const endRow = this.endRow(mark, startRow);
      if (endRow === null || endRow < top) {
        continue;
      }
      if (top === 0) {
        // Nothing is left above the viewport.
        mark.dropped = true;
        continue;
      }
      const cursorRow = top + buf.cursorY;
      const old = mark.end;
      mark.end = this.term.registerMarker(top - 1 - cursorRow);
      old?.dispose();
    }
    this.changed();
  }

  /**
   * The rows a mark covers, inclusive, or null when it has no stripe. A finished mark with no end marker has none.
   */
  private range(mark: Mark): { start: number; end: number } | null {
    const startRow = this.startRow(mark);
    if (startRow === null) {
      return null;
    }
    const endRow = this.endRow(mark, startRow);
    if (endRow === null) {
      return null;
    }
    return { start: startRow, end: Math.max(endRow, startRow) };
  }

  /**
   * The first row, -1 when the marker scrolled off the top of the scrollback, or null when the mark has no first
   * row (no marker, or a clear removed it). A marker that is disposed while on screen was removed by a clear,
   * which sets `dropped` first, so anything else disposed is trimmed.
   */
  private startRow(mark: Mark): number | null {
    if (!mark.start || mark.dropped) {
      return null;
    }
    if (mark.start.isDisposed || mark.start.line < 0) {
      return -1;
    }
    return mark.start.line;
  }

  /** The last row: the end marker, else the live cursor row while running, else null. */
  private endRow(mark: Mark, startRow: number): number | null {
    if (mark.end) {
      return mark.end.isDisposed || mark.end.line < 0 ? null : mark.end.line;
    }
    if (mark.exit !== null) {
      return null;
    }
    return Math.max(this.lastRow(), startRow);
  }

  /**
   * The cursor's row, minus the row after a trailing newline. Output that ends in a newline leaves the cursor on
   * the next row, which is not part of the command.
   */
  private lastRow(): number {
    const buf = this.term.buffer.active;
    const cursorRow = buf.baseY + buf.cursorY;
    return buf.cursorX > 0 ? cursorRow : cursorRow - 1;
  }
}

export function exitClass(exit: Exit | null): StripeExit {
  if (exit === null) {
    return 'running';
  }
  if (exit === 'unknown') {
    return 'unknown';
  }
  return exit === 0 ? 'ok' : 'fail';
}

// Gutter marks for each command: a thin stripe (green for exit 0, red otherwise, grey when unknown) and an
// "agent" badge for commands the agent ran. Positional UI only. Rust records stay the source of truth.
//
// Each C from the shell opens a mark, and each D closes the oldest open mark. The Rust side reports
// RecordStarted in the same order, so the oldest unpaired mark is that record. The badge needs that pairing,
// so it is attached only while the observer has never desynced. After a desync the pairing can drift, and the
// badges already shown stay as they are.

import type { IDecoration, IMarker, Terminal } from '@xterm/xterm';

/** The part of xterm this module uses. Kept small so the logic can be tested without a DOM. */
export type MarksTerminal = Pick<Terminal, 'cols' | 'buffer' | 'registerMarker' | 'registerDecoration'>;

type Exit = number | 'unknown';

interface Mark {
  marker: IMarker | undefined;
  /** null while the command runs. */
  exit: Exit | null;
  agent: boolean;
  /** The decoration's element, once xterm has rendered it. */
  el: HTMLElement | null;
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

  constructor(private readonly term: MarksTerminal) {}

  /** Shell event C: a command is starting. */
  commandStart(): void {
    // A C while a command is still open: the earlier command ended without a D we can trust.
    this.closeOpen('unknown');
    const mark: Mark = { marker: this.term.registerMarker(0), exit: null, agent: false, el: null };
    this.open.push(mark);
    const agent = this.unpairedStarts.shift();
    if (agent === undefined) {
      this.unpairedMarks.push(mark);
    } else {
      this.pair(mark, agent);
    }
  }

  /** Shell event D: the oldest open command has ended. A D with no open command is ignored. */
  commandEnd(exit: number): void {
    const mark = this.open.shift();
    if (mark) {
      this.finish(mark, exit);
    }
  }

  /** Shell event A: a new prompt. Any command still open ended without a D. */
  promptStart(): void {
    this.closeOpen('unknown');
  }

  /** Rust reported a record started. `agent` is true when the agent ran it. */
  recordStarted(agent: boolean): void {
    if (!this.trusted) {
      return;
    }
    const mark = this.unpairedMarks.shift();
    if (mark) {
      this.pair(mark, agent);
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
   * A new shell session: forget every open mark and all pairing, and trust pairing again. The decorations already
   * painted go with xterm's reset(); this only disposes the markers still held here.
   */
  reset(): void {
    for (const mark of [...this.open, ...this.unpairedMarks]) {
      mark.marker?.dispose();
    }
    this.open = [];
    this.unpairedMarks = [];
    this.unpairedStarts = [];
    this.trusted = true;
  }

  private pair(mark: Mark, agent: boolean): void {
    mark.agent = agent;
    this.paint(mark);
  }

  private closeOpen(exit: Exit): void {
    for (let mark = this.open.shift(); mark; mark = this.open.shift()) {
      this.finish(mark, exit);
    }
  }

  private finish(mark: Mark, exit: Exit): void {
    mark.exit = exit;
    const marker = mark.marker;
    if (marker && marker.line >= 0) {
      const buf = this.term.buffer.active;
      // D is emitted before the next prompt is drawn. Output that ends in a newline leaves the cursor at column 0
      // of the next prompt's row, which is not part of the command, so that row is left out. Output with no
      // trailing newline leaves the cursor mid-line on its own row, which is part of the command.
      const cursorRow = buf.baseY + buf.cursorY;
      const lastRow = buf.cursorX > 0 ? cursorRow : cursorRow - 1;
      const height = Math.max(1, lastRow - marker.line + 1);
      const decoration: IDecoration | undefined = this.term.registerDecoration({
        marker,
        x: 0,
        width: this.term.cols,
        height,
      });
      decoration?.onRender((el) => {
        mark.el = el;
        this.paint(mark);
      });
      decoration?.onDispose(() => {
        mark.el = null;
      });
    }
    this.paint(mark);
  }

  private paint(mark: Mark): void {
    const el = mark.el;
    if (!el) {
      return;
    }
    el.dataset.exit = exitClass(mark.exit);
    el.dataset.agent = mark.agent ? 'true' : 'false';
  }
}

export function exitClass(exit: Exit | null): string {
  if (exit === null) {
    return 'running';
  }
  if (exit === 'unknown') {
    return 'unknown';
  }
  return exit === 0 ? 'ok' : 'fail';
}

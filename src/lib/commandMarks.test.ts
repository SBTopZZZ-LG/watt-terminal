import { describe, expect, it } from 'vitest';
import type { IMarker } from '@xterm/xterm';
import { CommandMarks, exitClass } from './commandMarks';
import type { MarksTerminal } from './commandMarks';

/** A marker that is disposed the way xterm disposes one: line -1 and isDisposed set. */
class FakeMarker {
  line: number;
  isDisposed = false;
  constructor(line: number) {
    this.line = line;
  }
  dispose(): void {
    this.line = -1;
    this.isDisposed = true;
  }
}

interface FakeBuffer {
  type: 'normal' | 'alternate';
  baseY: number;
  cursorY: number;
  cursorX: number;
  viewportY: number;
}

/**
 * Enough of xterm for CommandMarks: a buffer whose cursor the test moves, markers placed relative to the cursor, and
 * the ED2 handler, which runs first and then erases the viewport rows (disposing the markers on them), as xterm does.
 */
function fakeTerminal(opts: { alt?: boolean } = {}) {
  const markers: FakeMarker[] = [];
  const buffer: FakeBuffer = {
    type: opts.alt ? 'alternate' : 'normal',
    baseY: 0,
    cursorY: 0,
    cursorX: 0,
    viewportY: 0,
  };
  let csiHandler: ((params: (number | number[])[]) => boolean) | undefined;
  const term = {
    buffer: { active: buffer },
    parser: {
      registerCsiHandler: (id: { final: string }, cb: (params: (number | number[])[]) => boolean) => {
        if (id.final === 'J') {
          csiHandler = cb;
        }
        return { dispose: () => undefined };
      },
    },
    registerMarker: (offset = 0) => {
      if (buffer.type === 'alternate') {
        return undefined;
      }
      const m = new FakeMarker(buffer.baseY + buffer.cursorY + offset);
      markers.push(m);
      return m as unknown as IMarker;
    },
  };
  return {
    term: term as unknown as MarksTerminal,
    markers,
    buffer,
    /** Moves the cursor down n rows, as output does. Ends at column 0, as output that ends in a newline does. */
    advance: (n: number) => {
      buffer.cursorY += n;
      buffer.cursorX = 0;
    },
    /** Places the cursor mid-line, as output with no trailing newline does. */
    setCursorX: (x: number) => {
      buffer.cursorX = x;
    },
    /** ESC[2J: the handler sees the rows first, then the viewport rows are erased and their markers disposed. */
    clear: () => {
      csiHandler?.([2]);
      for (const m of markers) {
        if (m.line >= buffer.baseY) {
          m.dispose();
        }
      }
    },
  };
}

describe('CommandMarks', () => {
  it('paints the exit status of a finished command', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandStart();
    f.advance(3);
    marks.commandEnd(0);
    expect(marks.stripes()).toEqual([{ start: 0, end: 2, exit: 'ok', agent: false, separatorRow: null }]);
  });

  it('a non-zero exit paints as a failure, and the stripe covers only the output rows', () => {
    // Four lines of output, each ending in a newline. The cursor is at column 0 of row 4, which is the next
    // prompt's row (D fires before the prompt is drawn). Rows 0 to 3 are the output, so the stripe is 4 rows.
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandStart();
    f.advance(4);
    marks.commandEnd(1);
    expect(marks.stripes()).toEqual([{ start: 0, end: 3, exit: 'fail', agent: false, separatorRow: null }]);
  });

  it('output with no trailing newline includes the cursor row', () => {
    // Two full lines, then text on row 2 with the cursor mid-line at column 5. Row 2 is output, so rows 0 to 2
    // are covered: 3 rows.
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandStart();
    f.advance(2);
    f.setCursorX(5);
    marks.commandEnd(0);
    expect(marks.stripes()).toEqual([{ start: 0, end: 2, exit: 'ok', agent: false, separatorRow: null }]);
  });

  it('a command with no output still gets a one-row stripe', () => {
    // The cursor is at column 0 of the command's own row, so the row-excluding rule would give none. The minimum is 1.
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandStart();
    marks.commandEnd(0);
    expect(marks.stripes()).toEqual([{ start: 0, end: 0, exit: 'ok', agent: false, separatorRow: null }]);
  });

  it('a single line of output ending in a newline gets a one-row stripe', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandStart();
    f.advance(1);
    marks.commandEnd(0);
    expect(marks.stripes()).toEqual([{ start: 0, end: 0, exit: 'ok', agent: false, separatorRow: null }]);
  });

  it('a running command has a muted stripe that grows with its output', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandStart();
    expect(marks.stripes()).toEqual([{ start: 0, end: 0, exit: 'running', agent: false, separatorRow: null }]);
    f.advance(1);
    expect(marks.stripes()).toEqual([{ start: 0, end: 0, exit: 'running', agent: false, separatorRow: null }]);
    f.advance(4);
    expect(marks.stripes()).toEqual([{ start: 0, end: 4, exit: 'running', agent: false, separatorRow: null }]);
    marks.commandEnd(0);
    expect(marks.stripes()).toEqual([{ start: 0, end: 4, exit: 'ok', agent: false, separatorRow: null }]);
  });

  it('a D with no open command draws nothing', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandEnd(0);
    expect(marks.stripes()).toEqual([]);
  });

  it('a second C closes the open command as unknown, and an A does the same', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandStart();
    marks.commandStart();
    expect(marks.stripes().map((s) => s.exit)).toEqual(['unknown', 'running']);
    marks.promptStart();
    expect(marks.stripes().map((s) => s.exit)).toEqual(['unknown', 'unknown']);
  });

  it('marks the agent badge by pairing with Rust records in order', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandStart();
    marks.commandEnd(0);
    marks.commandStart();
    marks.commandEnd(1);

    marks.recordStarted(true);
    marks.recordStarted(false);

    const [first, second] = marks.stripes();
    expect(first).toMatchObject({ exit: 'ok', agent: true });
    expect(second).toMatchObject({ exit: 'fail', agent: false });
  });

  it('pairs when the record is reported after its C', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandStart();
    marks.recordStarted(true);
    marks.commandEnd(0);
    expect(marks.stripes()).toEqual([{ start: 0, end: 0, exit: 'ok', agent: true, separatorRow: null }]);
  });

  it('pairs when the record is reported before its C is parsed, and keeps later commands aligned', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    // Rust is ahead of the renderer: both records arrive before either C is parsed.
    marks.recordStarted(true);
    marks.recordStarted(false);
    marks.commandStart();
    marks.commandEnd(0);
    marks.commandStart();
    marks.commandEnd(0);
    const [first, second] = marks.stripes();
    expect(first.agent).toBe(true);
    expect(second.agent).toBe(false);
  });

  it('stops pairing once the observer desyncs, so no badge is put on the wrong command', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandStart();
    marks.commandEnd(0);
    marks.observerDesynced();
    marks.commandStart();
    marks.commandEnd(0);
    marks.recordStarted(true);
    expect(marks.stripes()[1].agent).toBe(false);
  });

  it('reset forgets the old session and its pairing, so the new session starts clean', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    // An old command is open and its desync stopped pairing.
    marks.commandStart();
    marks.observerDesynced();
    marks.reset();

    marks.commandStart();
    marks.commandEnd(0);
    marks.recordStarted(true);

    expect(marks.stripes()).toEqual([{ start: 0, end: 0, exit: 'ok', agent: true, separatorRow: null }]);
  });

  it('reset disposes the markers it holds', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandStart();
    marks.commandEnd(0);
    marks.commandStart();
    marks.reset();
    expect(f.markers.every((m) => m.isDisposed)).toBe(true);
  });

  it('a clear cuts a finished command that started in the scrollback at the last scrollback row', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    // The command starts on row 4. Its output scrolls the viewport down to start at row 10, and the cursor ends
    // at row 21, column 0. So the last output row is 20.
    f.buffer.cursorY = 4;
    marks.commandStart();
    f.buffer.baseY = 10;
    f.buffer.cursorY = 11;
    marks.commandEnd(0);
    expect(marks.stripes()).toEqual([{ start: 4, end: 20, exit: 'ok', agent: false, separatorRow: null }]);

    // A clear erases rows 10 onwards. The stripe stops at row 9, the last row above the viewport.
    f.clear();
    expect(marks.stripes()).toEqual([{ start: 4, end: 9, exit: 'ok', agent: false, separatorRow: null }]);
  });

  it('a clear leaves a command that ended in the scrollback alone', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandStart();
    f.advance(4);
    f.buffer.baseY = 10;
    f.buffer.cursorY = 0;
    marks.commandEnd(0);
    const before = marks.stripes();
    f.clear();
    expect(marks.stripes()).toEqual(before);
  });

  it('the alternate screen gets no marker, so no stripe, and no exception', () => {
    const f = fakeTerminal({ alt: true });
    const marks = new CommandMarks(f.term);
    expect(() => {
      marks.commandStart();
      f.advance(2);
      marks.commandEnd(0);
      marks.promptStart();
    }).not.toThrow();
    expect(marks.stripes()).toEqual([]);
  });

  it('an erased marker gets no stripe, but its command still counts for pairing', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandStart();
    f.clear();
    marks.commandEnd(0);
    marks.commandStart();
    marks.commandEnd(1);
    expect(marks.stripes()).toHaveLength(1);

    // Records still arrive one per command, so the first record belongs to the erased command.
    marks.recordStarted(true);
    marks.recordStarted(false);
    expect(marks.stripes()[0]).toEqual({ start: 0, end: 0, exit: 'fail', agent: false, separatorRow: null });
  });
});

describe('exitClass', () => {
  it('maps the states to the classes the CSS uses', () => {
    expect(exitClass(null)).toBe('running');
    expect(exitClass('unknown')).toBe('unknown');
    expect(exitClass(0)).toBe('ok');
    expect(exitClass(127)).toBe('fail');
  });
});

// CommandMarks against a real xterm buffer (headless, no DOM). The fake in commandMarks.test.ts covers pairing; this
// file checks what xterm itself does: the ESC[2J handler runs before the erase, markers on erased rows are disposed,
// scrollback trims dispose markers, and the alternate screen gives no marker.

import { describe, expect, it } from 'vitest';
import pkg from '@xterm/headless';
import { CommandMarks } from './commandMarks';
import type { MarksTerminal } from './commandMarks';
import { stripeSegments } from './stripes';

const { Terminal } = pkg;

function makeTerminal(scrollback: number) {
  return new Terminal({ cols: 80, rows: 10, scrollback, allowProposedApi: true });
}

/** Resolves once xterm has parsed the bytes. */
function write(term: InstanceType<typeof Terminal>, data: string): Promise<void> {
  return new Promise((resolve) => term.write(data, resolve));
}

function lines(prefix: string, n: number): string {
  let out = '';
  for (let i = 0; i < n; i++) {
    out += `${prefix} ${i}\r\n`;
  }
  return out;
}

describe('CommandMarks on a real buffer', () => {
  it('a clear cuts a command that started in the scrollback at the last row above the viewport', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    marks.commandStart();
    await write(term, lines('out', 30));
    marks.commandEnd(0);
    const buf = term.buffer.active;
    expect(buf.baseY).toBe(21);
    expect(marks.stripes()).toEqual([{ start: 0, end: 29, exit: 'ok', agent: false }]);

    // ESC[H ESC[2J, what `clear` sends: the viewport rows are erased in place.
    await write(term, '\x1b[H\x1b[2J');
    expect(marks.stripes()).toEqual([{ start: 0, end: 20, exit: 'ok', agent: false }]);
    for (let y = 0; y < 10; y++) {
      expect(buf.getLine(21 + y)?.translateToString(true) ?? '').toBe('');
    }
    // No stripe covers the erased rows.
    expect(stripeSegments(marks.stripes(), buf.viewportY, 10)).toEqual([]);
  });

  it('a clear removes a command whose first row was in the viewport', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    await write(term, lines('before', 3));
    marks.commandStart();
    await write(term, 'running\r\n');
    marks.commandEnd(0);
    expect(marks.stripes()).toEqual([{ start: 3, end: 3, exit: 'ok', agent: false }]);

    await write(term, '\x1b[H\x1b[2J');
    expect(marks.stripes()).toEqual([]);
  });

  it('a clear in the alternate screen leaves the normal screen stripes alone', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    marks.commandStart();
    await write(term, lines('out', 30));
    marks.commandEnd(0);
    const before = marks.stripes();
    await write(term, '\x1b[?1049h\x1b[2J\x1b[?1049l');
    expect(term.buffer.active.type).toBe('normal');
    expect(marks.stripes()).toEqual(before);
  });

  it('a command whose first row is trimmed from the scrollback keeps its stripe from the top, until its end goes too', async () => {
    const term = makeTerminal(20);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    marks.commandStart();
    await write(term, lines('out', 60));
    marks.commandEnd(0);
    const buf = term.buffer.active;
    const [stripe] = marks.stripes();
    expect(stripe).toEqual({ start: -1, end: buf.baseY + buf.cursorY - 1, exit: 'ok', agent: false });
    const [seg] = stripeSegments([stripe], buf.viewportY, 10);
    expect(seg).toMatchObject({ firstRow: 0, startVisible: false });
  });

  it('a running command whose first row was trimmed still gets a stripe that grows', async () => {
    const term = makeTerminal(20);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    marks.commandStart();
    await write(term, lines('out', 60));
    const buf = term.buffer.active;
    expect(marks.stripes()).toEqual([{ start: -1, end: buf.baseY + buf.cursorY - 1, exit: 'running', agent: false }]);
  });

  it('the alternate screen gets no markers, so no stripes and no exception', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    await write(term, '\x1b[?1049h');
    expect(() => marks.commandStart()).not.toThrow();
    await write(term, 'vim line\r\n');
    expect(() => marks.commandEnd(0)).not.toThrow();
    expect(marks.stripes()).toEqual([]);
  });
});

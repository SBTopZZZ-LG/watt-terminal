// CommandMarks against a real xterm buffer (headless, no DOM). The fake in commandMarks.test.ts covers pairing; this
// file checks what xterm itself does: the ESC[2J handler runs before the erase, markers on erased rows are disposed,
// scrollback trims dispose markers, and the alternate screen gives no marker.

import { describe, expect, it } from 'vitest';
import pkg from '@xterm/headless';
import { CommandMarks } from './commandMarks';
import type { MarksTerminal } from './commandMarks';
import { badgeOnSeparator, separatorSegments, stripeSegments } from './stripes';

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

/** The absolute rows of the separator lines. */
function separatorRows(marks: CommandMarks): number[] {
  return marks.separators().map((sep) => sep.row);
}

function isBlank(term: InstanceType<typeof Terminal>, row: number): boolean {
  return term.buffer.active.getLine(row)?.translateToString(true) === '';
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
    expect(marks.stripes()).toEqual([{ start: 0, end: 29, exit: 'ok', agent: false, separatorRow: null }]);

    // ESC[H ESC[2J, what `clear` sends: the viewport rows are erased in place.
    await write(term, '\x1b[H\x1b[2J');
    expect(marks.stripes()).toEqual([{ start: 0, end: 20, exit: 'ok', agent: false, separatorRow: null }]);
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
    expect(marks.stripes()).toEqual([{ start: 3, end: 3, exit: 'ok', agent: false, separatorRow: null }]);

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
    expect(stripe).toEqual({
      start: -1,
      end: buf.baseY + buf.cursorY - 1,
      exit: "ok",
      agent: false,
      separatorRow: null,
    });
    const [seg] = stripeSegments([stripe], buf.viewportY, 10);
    expect(seg).toMatchObject({ firstRow: 0, startVisible: false });
  });

  it('a running command whose first row was trimmed still gets a stripe that grows', async () => {
    const term = makeTerminal(20);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    marks.commandStart();
    await write(term, lines('out', 60));
    const buf = term.buffer.active;
    expect(marks.stripes()).toEqual([
      { start: -1, end: buf.baseY + buf.cursorY - 1, exit: "running", agent: false, separatorRow: null },
    ]);
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

describe('separator lines on a real buffer', () => {
  it('one command gives one separator, on the prompt row after its output', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    marks.commandStart();
    await write(term, 'out 0\r\nout 1\r\n');
    marks.commandEnd(0);
    marks.promptStart();
    await write(term, '$ ');
    expect(separatorRows(marks)).toEqual([2]);
  });

  it('the first prompt alone gives no separator', () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    marks.promptStart();
    expect(separatorRows(marks)).toEqual([]);
  });

  it('a second prompt with no command in between gives none (an empty Enter)', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    marks.commandStart();
    await write(term, 'out 0\r\n');
    marks.commandEnd(0);
    marks.promptStart();
    await write(term, '$ \r\n');
    marks.promptStart();
    await write(term, '$ ');
    expect(separatorRows(marks)).toEqual([1]);
  });

  it('two commands give two separators, each on its own prompt row', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    marks.commandStart();
    await write(term, 'out 0\r\nout 1\r\n');
    marks.commandEnd(0);
    marks.promptStart();
    await write(term, '$ cmd\r\n');
    marks.commandStart();
    await write(term, 'a\r\nb\r\nc\r\n');
    marks.commandEnd(0);
    marks.promptStart();
    await write(term, '$ ');
    expect(separatorRows(marks)).toEqual([2, 6]);
  });

  it('a separator in the scrollback keeps its absolute row, which maps into the viewport when visible', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    marks.commandStart();
    await write(term, lines('out', 30));
    marks.commandEnd(0);
    marks.promptStart();
    const buf = term.buffer.active;
    expect(separatorRows(marks)).toEqual([30]);
    expect(separatorSegments(marks.separators(), buf.viewportY, 10)).toEqual([{ row: 9, agent: false }]);
  });

  it('a clear of the command itself draws no separator after it', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    marks.commandStart();
    await write(term, '\x1b[H\x1b[2J\x1b[3J');
    marks.commandEnd(0);
    marks.promptStart();
    await write(term, '$ ');
    expect(separatorRows(marks)).toEqual([]);
  });

  it('a clear after an earlier command erases that command\'s separator, and draws none for the clear', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    marks.commandStart();
    await write(term, 'out 0\r\n');
    marks.commandEnd(0);
    marks.promptStart();
    await write(term, '$ clear\r\n');
    expect(separatorRows(marks)).toEqual([1]);

    marks.commandStart();
    await write(term, '\x1b[H\x1b[2J\x1b[3J');
    marks.commandEnd(0);
    marks.promptStart();
    await write(term, '$ ');
    expect(separatorRows(marks)).toEqual([]);
  });

  it('ESC[H ESC[2J after a separator in the viewport removes it', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    marks.commandStart();
    await write(term, 'out\r\n');
    marks.commandEnd(0);
    marks.promptStart();
    await write(term, '$ ');
    expect(separatorRows(marks)).toEqual([1]);

    await write(term, '\x1b[H\x1b[2J');
    expect(separatorRows(marks)).toEqual([]);
  });

  it('a separator trimmed from the scrollback is dropped', async () => {
    const term = makeTerminal(20);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    marks.commandStart();
    await write(term, 'out\r\n');
    marks.commandEnd(0);
    marks.promptStart();
    expect(separatorRows(marks)).toEqual([1]);

    await write(term, lines('more', 60));
    expect(separatorRows(marks)).toEqual([]);
  });

  it('reset removes every separator', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    marks.commandStart();
    await write(term, 'out 0\r\n');
    marks.commandEnd(0);
    marks.promptStart();
    await write(term, '$ cmd\r\n');
    marks.commandStart();
    await write(term, 'out 1\r\n');
    marks.commandEnd(0);
    marks.promptStart();
    expect(separatorRows(marks)).toHaveLength(2);

    marks.reset();
    expect(separatorRows(marks)).toEqual([]);
  });

  it('the alternate screen gets no separator and no exception', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    await write(term, '\x1b[?1049h');
    marks.commandStart();
    await write(term, 'vim line\r\n');
    marks.commandEnd(0);
    expect(() => marks.promptStart()).not.toThrow();
    expect(separatorRows(marks)).toEqual([]);
  });
});

/** A command that prints one line and ends, then a prompt drawn as the shell does: a blank row, then the prompt. */
async function commandThenPrompt(term: InstanceType<typeof Terminal>, marks: CommandMarks): Promise<void> {
  marks.commandStart();
  marks.recordStarted(false);
  await write(term, 'out 0\r\n');
  marks.commandEnd(0);
  marks.promptStart();
  await write(term, '\r\n$ ');
}

/** The next command, which an agent ran or the user did, printing one line and ending. */
function agentCommand(term: InstanceType<typeof Terminal>, marks: CommandMarks, agent: boolean): Promise<void> {
  marks.commandStart();
  marks.recordStarted(agent);
  return write(term, 'a\r\n').then(() => marks.commandEnd(0));
}

describe('the agent badge on the separator, on a real buffer', () => {
  it('the command under a prompt takes its separator, on a blank row, and shows the agent badge', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    await commandThenPrompt(term, marks);
    await agentCommand(term, marks, true);
    expect(marks.separators()).toEqual([{ row: 1, agent: true }]);
    expect(isBlank(term, 1)).toBe(true);
    expect(marks.stripes()).toEqual([
      { start: 0, end: 0, exit: 'ok', agent: false, separatorRow: null },
      { start: 2, end: 2, exit: 'ok', agent: true, separatorRow: 1 },
    ]);
  });

  it('a user command after a separator gives the separator agent false', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    await commandThenPrompt(term, marks);
    await agentCommand(term, marks, false);
    expect(marks.separators()).toEqual([{ row: 1, agent: false }]);
    expect(marks.stripes()[1]).toMatchObject({ agent: false, separatorRow: 1 });
  });

  it('the first command ever has no separator, even after a prompt', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    marks.promptStart();
    await write(term, '$ ');
    await agentCommand(term, marks, true);
    expect(marks.separators()).toEqual([]);
    expect(marks.stripes()).toEqual([{ start: 0, end: 0, exit: 'ok', agent: true, separatorRow: null }]);
  });

  it('a command after an empty Enter has no separator, and the earlier separator stays after its command', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    await commandThenPrompt(term, marks);
    // Empty Enter: the prompt moves to a new row before the next prompt.
    await write(term, '\r\n');
    marks.promptStart();
    await write(term, '$ ');
    await agentCommand(term, marks, true);
    expect(marks.separators()).toEqual([{ row: 1, agent: false }]);
    expect(marks.stripes()[1]).toMatchObject({ agent: true, separatorRow: null });
  });

  it('a redraw of the prompt on the same row keeps the separator for the next command', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    marks.commandStart();
    marks.recordStarted(false);
    await write(term, 'out 0\r\n');
    marks.commandEnd(0);
    marks.promptStart();
    marks.promptStart();
    await write(term, '\r\n$ ');
    await agentCommand(term, marks, true);
    expect(marks.separators()).toEqual([{ row: 1, agent: true }]);
    expect(marks.stripes()[1]).toMatchObject({ agent: true, separatorRow: 1 });
  });

  it('a command after a clear has no separator, since the clear erased the one above it', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    await commandThenPrompt(term, marks);
    marks.commandStart();
    marks.recordStarted(false);
    await write(term, '\x1b[H\x1b[2J\x1b[3J');
    marks.commandEnd(0);
    marks.promptStart();
    await write(term, '$ ');
    expect(separatorRows(marks)).toEqual([]);
    await agentCommand(term, marks, true);
    expect(marks.stripes()).toEqual([{ start: 0, end: 0, exit: 'ok', agent: true, separatorRow: null }]);
  });

  it('reset forgets the separators, and a command after it has none', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    await commandThenPrompt(term, marks);
    await agentCommand(term, marks, true);
    expect(marks.separators()).toHaveLength(1);

    marks.reset();
    expect(marks.separators()).toEqual([]);
    await agentCommand(term, marks, true);
    expect(marks.stripes().map((s) => s.separatorRow)).toEqual([null]);
  });

  it('a separator trimmed from the scrollback leaves its command with no separator row', async () => {
    const term = makeTerminal(20);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    await commandThenPrompt(term, marks);
    marks.commandStart();
    marks.recordStarted(true);
    await write(term, lines('more', 60));
    expect(separatorRows(marks)).toEqual([]);
    expect(marks.stripes()).toEqual([
      expect.objectContaining({ exit: 'running', agent: true, separatorRow: null }),
    ]);
  });

  it('a blank separator scrolled out of view still keeps the badge off the stripe', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    await commandThenPrompt(term, marks);
    marks.commandStart();
    marks.recordStarted(true);
    await write(term, lines('out', 30));
    marks.commandEnd(0);
    const buf = term.buffer.active;
    expect(separatorRows(marks)).toEqual([1]);
    expect(separatorSegments(marks.separators(), buf.viewportY, 10)).toEqual([]);
    const [, cmd2] = marks.stripes();
    expect(cmd2.separatorRow).toBe(1);
    // The stripe's own rows are on screen, but its badge stays off the stripe while the blank separator is up there.
    expect(stripeSegments([cmd2], buf.viewportY, 10)).toHaveLength(1);
    expect(badgeOnSeparator(cmd2.separatorRow, (row) => isBlank(term, row))).toBe(true);
  });

  it('a separator on a row with text is not blank, so the badge stays on the stripe', async () => {
    const term = makeTerminal(100);
    const marks = new CommandMarks(term as unknown as MarksTerminal);
    marks.commandStart();
    await write(term, 'out 0\r\n');
    marks.commandEnd(0);
    marks.promptStart();
    // No blank row: the prompt is drawn on the separator's own row.
    await write(term, '$ ');
    await agentCommand(term, marks, true);
    expect(separatorRows(marks)).toEqual([1]);
    expect(isBlank(term, 1)).toBe(false);
    const [, cmd2] = marks.stripes();
    expect(cmd2.separatorRow).toBe(1);
    expect(badgeOnSeparator(cmd2.separatorRow, (row) => isBlank(term, row))).toBe(false);
  });
});

import { describe, expect, it } from 'vitest';
import type { IDisposable, IMarker } from '@xterm/xterm';
import { CommandMarks, exitClass } from './commandMarks';
import type { MarksTerminal } from './commandMarks';

/** A marker that records dispose listeners, as xterm's does. */
class FakeMarker {
  line: number;
  private listeners: Array<() => void> = [];
  constructor(line: number) {
    this.line = line;
  }
  onDispose(cb: () => void): IDisposable {
    this.listeners.push(cb);
    return { dispose: () => undefined };
  }
  /** What xterm does when the line is erased. */
  erase(): void {
    this.line = -1;
    for (const cb of this.listeners) {
      cb();
    }
  }
}

interface FakeDecoration {
  marker: FakeMarker;
  height: number;
  render?: (el: HTMLElement) => void;
}

/** Enough of xterm for CommandMarks. Decorations keep the element they are rendered into. */
function fakeTerminal() {
  const markers: FakeMarker[] = [];
  const decorations: FakeDecoration[] = [];
  const active = { baseY: 0, cursorY: 0 };
  const term = {
    cols: 80,
    rows: 24,
    buffer: { active },
    registerMarker: () => {
      const m = new FakeMarker(active.baseY + active.cursorY);
      markers.push(m);
      return m as unknown as IMarker;
    },
    registerDecoration: (opts: { marker: IMarker; height?: number }) => {
      const d: FakeDecoration = { marker: opts.marker as unknown as FakeMarker, height: opts.height ?? 1 };
      decorations.push(d);
      return {
        onRender: (cb: (el: HTMLElement) => void) => {
          d.render = cb;
        },
        onDispose: () => ({ dispose: () => undefined }),
      };
    },
  };
  return {
    term: term as unknown as MarksTerminal,
    markers,
    decorations,
    /** Moves the cursor down n rows, as output does. */
    advance: (n: number) => {
      active.cursorY += n;
    },
  };
}

/** Renders a decoration the way xterm does, and returns the element it painted. */
function render(d: FakeDecoration): { dataset: Record<string, string> } {
  const el = { dataset: {} as Record<string, string> };
  d.render?.(el as unknown as HTMLElement);
  return el;
}

describe('CommandMarks', () => {
  it('paints the exit status of a finished command', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandStart();
    f.advance(3);
    marks.commandEnd(0);
    expect(f.decorations).toHaveLength(1);
    expect(render(f.decorations[0]).dataset).toEqual({ exit: 'ok', agent: 'false' });
  });

  it('a non-zero exit paints as a failure, and the stripe covers the output rows', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandStart();
    f.advance(4);
    marks.commandEnd(1);
    expect(f.decorations[0].height).toBe(5);
    expect(render(f.decorations[0]).dataset.exit).toBe('fail');
  });

  it('a D with no open command draws nothing', () => {
    const f = fakeTerminal();
    new CommandMarks(f.term).commandEnd(0);
    expect(f.decorations).toHaveLength(0);
  });

  it('a second C closes the open command as unknown, and an A does the same', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandStart();
    marks.commandStart();
    expect(render(f.decorations[0]).dataset.exit).toBe('unknown');
    marks.promptStart();
    expect(render(f.decorations[1]).dataset.exit).toBe('unknown');
  });

  it('marks the agent badge by pairing with Rust records in order', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandStart();
    marks.commandEnd(0);
    marks.commandStart();
    marks.commandEnd(1);
    const first = render(f.decorations[0]);
    const second = render(f.decorations[1]);

    marks.recordStarted(true);
    marks.recordStarted(false);

    expect(first.dataset.agent).toBe('true');
    expect(second.dataset.agent).toBe('false');
  });

  it('pairs when the record is reported after its C', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandStart();
    marks.recordStarted(true);
    marks.commandEnd(0);
    expect(render(f.decorations[0]).dataset).toEqual({ exit: 'ok', agent: 'true' });
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
    expect(render(f.decorations[0]).dataset.agent).toBe('true');
    expect(render(f.decorations[1]).dataset.agent).toBe('false');
  });

  it('stops pairing once the observer desyncs, so no badge is put on the wrong command', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    marks.commandStart();
    marks.commandEnd(0);
    marks.observerDesynced();
    marks.commandStart();
    marks.commandEnd(0);
    const later = render(f.decorations[1]);
    marks.recordStarted(true);
    expect(later.dataset.agent).toBe('false');
  });

  it('an erased marker gets no decoration, but its command still counts for pairing', () => {
    const f = fakeTerminal();
    const marks = new CommandMarks(f.term);
    // The first command's line is erased (ESC[3J) before its D arrives.
    marks.commandStart();
    f.markers[0].erase();
    marks.commandEnd(0);
    marks.commandStart();
    marks.commandEnd(1);
    expect(f.decorations).toHaveLength(1);

    // Records still arrive one per command, so the first record belongs to the erased command.
    marks.recordStarted(true);
    marks.recordStarted(false);
    expect(render(f.decorations[0]).dataset).toEqual({ exit: 'fail', agent: 'false' });
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

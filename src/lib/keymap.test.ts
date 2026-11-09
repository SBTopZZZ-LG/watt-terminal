import { describe, expect, it } from 'vitest';
import { createKeyFilter, mapTerminalKey } from './keymap';
import type { KeyLike } from './keymap';
import type { InputState } from './types';

const ALL_STATES: InputState[] = [
  'booting',
  'prompt',
  'submitted',
  'running',
  'continuation',
  'alt',
  'raw',
  'exited',
];
/** The states where the shell (or a program in it) owns the keyboard and the rules apply. */
const SHELL_STATES: InputState[] = ['booting', 'prompt', 'submitted', 'continuation', 'running', 'raw'];

interface Mods {
  alt?: boolean;
  meta?: boolean;
  ctrl?: boolean;
  shift?: boolean;
}

function key(k: string, mods: Mods = {}, extra: Partial<KeyLike> = {}): KeyLike {
  return {
    type: 'keydown',
    key: k,
    code: k,
    keyCode: 0,
    repeat: false,
    isComposing: false,
    altKey: mods.alt ?? false,
    metaKey: mods.meta ?? false,
    ctrlKey: mods.ctrl ?? false,
    shiftKey: mods.shift ?? false,
    ...extra,
  };
}

describe('mapTerminalKey: the rules', () => {
  it('Option+Left sends ESC b (backward word) and Option+Right sends ESC f (forward word)', () => {
    expect(mapTerminalKey(key('ArrowLeft', { alt: true }), 'prompt')).toBe('\x1bb');
    expect(mapTerminalKey(key('ArrowRight', { alt: true }), 'prompt')).toBe('\x1bf');
  });

  it('Cmd+Left sends Ctrl-A (line start), Cmd+Right sends Ctrl-E (line end), Cmd+Backspace sends Ctrl-U', () => {
    expect(mapTerminalKey(key('ArrowLeft', { meta: true }), 'prompt')).toBe('\x01');
    expect(mapTerminalKey(key('ArrowRight', { meta: true }), 'prompt')).toBe('\x05');
    expect(mapTerminalKey(key('Backspace', { meta: true }), 'prompt')).toBe('\x15');
  });

  it('Shift+Enter sends ESC CR while a command runs or the shell has no integration', () => {
    expect(mapTerminalKey(key('Enter', { shift: true }), 'running')).toBe('\x1b\r');
    expect(mapTerminalKey(key('Enter', { shift: true }), 'raw')).toBe('\x1b\r');
  });

  it('Shift+Enter stays a plain Enter (not mapped) at a prompt, while submitted or continuation', () => {
    for (const s of ['booting', 'prompt', 'submitted', 'continuation'] as const) {
      expect(mapTerminalKey(key('Enter', { shift: true }), s), s).toBeNull();
    }
  });

  it('maps every rule in every shell state', () => {
    const cases: Array<[KeyLike, string]> = [
      [key('ArrowLeft', { alt: true }), '\x1bb'],
      [key('ArrowRight', { alt: true }), '\x1bf'],
      [key('ArrowLeft', { meta: true }), '\x01'],
      [key('ArrowRight', { meta: true }), '\x05'],
      [key('Backspace', { meta: true }), '\x15'],
    ];
    for (const s of SHELL_STATES) {
      for (const [ev, data] of cases) {
        expect(mapTerminalKey(ev, s), `${ev.key} in ${s}`).toBe(data);
      }
    }
  });

  it('does not map a key that is not in the table', () => {
    expect(mapTerminalKey(key('ArrowUp', { alt: true }), 'prompt')).toBeNull();
    expect(mapTerminalKey(key('ArrowDown', { meta: true }), 'prompt')).toBeNull();
    expect(mapTerminalKey(key('Backspace', { alt: true }), 'prompt')).toBeNull();
    expect(mapTerminalKey(key('ArrowLeft'), 'prompt')).toBeNull();
    expect(mapTerminalKey(key('a', { meta: true }), 'prompt')).toBeNull();
  });
});

describe('mapTerminalKey: state gating', () => {
  it('never maps in the alternate screen, so full-screen apps get xterm default encoding', () => {
    for (const ev of [
      key('ArrowLeft', { alt: true }),
      key('ArrowRight', { alt: true }),
      key('ArrowLeft', { meta: true }),
      key('ArrowRight', { meta: true }),
      key('Backspace', { meta: true }),
      key('Enter', { shift: true }),
    ]) {
      expect(mapTerminalKey(ev, 'alt'), ev.key).toBeNull();
    }
  });

  it('never maps after the shell exited', () => {
    for (const ev of [
      key('ArrowLeft', { alt: true }),
      key('ArrowLeft', { meta: true }),
      key('Backspace', { meta: true }),
      key('Enter', { shift: true }),
    ]) {
      expect(mapTerminalKey(ev, 'exited'), ev.key).toBeNull();
    }
  });

  it('Shift+Enter is mapped only in running and raw, among all states', () => {
    for (const s of ALL_STATES) {
      const expected = s === 'running' || s === 'raw' ? '\x1b\r' : null;
      expect(mapTerminalKey(key('Enter', { shift: true }), s), s).toBe(expected);
    }
  });

  it('Option+Left is mapped in every shell state, and not in alt or exited', () => {
    for (const s of ALL_STATES) {
      const expected = SHELL_STATES.includes(s) ? '\x1bb' : null;
      expect(mapTerminalKey(key('ArrowLeft', { alt: true }), s), s).toBe(expected);
    }
  });
});

describe('mapTerminalKey: extra modifiers and near misses are not mapped', () => {
  it('rejects Option+Left/Right combined with another modifier', () => {
    expect(mapTerminalKey(key('ArrowLeft', { alt: true, shift: true }), 'prompt')).toBeNull();
    expect(mapTerminalKey(key('ArrowLeft', { alt: true, ctrl: true }), 'prompt')).toBeNull();
    expect(mapTerminalKey(key('ArrowRight', { alt: true, meta: true }), 'prompt')).toBeNull();
  });

  it('rejects Cmd+Left/Right/Backspace combined with another modifier', () => {
    expect(mapTerminalKey(key('ArrowLeft', { meta: true, shift: true }), 'prompt')).toBeNull();
    expect(mapTerminalKey(key('ArrowRight', { meta: true, alt: true }), 'prompt')).toBeNull();
    expect(mapTerminalKey(key('Backspace', { meta: true, ctrl: true }), 'prompt')).toBeNull();
  });

  it('rejects Ctrl-modified arrows and plain arrows', () => {
    expect(mapTerminalKey(key('ArrowLeft', { ctrl: true }), 'prompt')).toBeNull();
    expect(mapTerminalKey(key('ArrowRight', { ctrl: true }), 'prompt')).toBeNull();
    expect(mapTerminalKey(key('ArrowLeft', { shift: true }), 'prompt')).toBeNull();
  });

  it('rejects Shift+Enter combined with another modifier', () => {
    expect(mapTerminalKey(key('Enter', { shift: true, alt: true }), 'running')).toBeNull();
    expect(mapTerminalKey(key('Enter', { shift: true, ctrl: true }), 'running')).toBeNull();
    expect(mapTerminalKey(key('Enter', { shift: true, meta: true }), 'running')).toBeNull();
  });

  it('leaves plain Enter, plain Backspace and Option+Backspace to xterm', () => {
    expect(mapTerminalKey(key('Enter'), 'running')).toBeNull();
    expect(mapTerminalKey(key('Backspace'), 'prompt')).toBeNull();
    expect(mapTerminalKey(key('Backspace', { alt: true }), 'prompt')).toBeNull();
  });
});

describe('mapTerminalKey: event type, composition and repeat', () => {
  it('maps keydown only, never keypress or keyup', () => {
    expect(mapTerminalKey(key('ArrowLeft', { alt: true }, { type: 'keypress' }), 'prompt')).toBeNull();
    expect(mapTerminalKey(key('ArrowLeft', { alt: true }, { type: 'keyup' }), 'prompt')).toBeNull();
  });

  it('does nothing during an IME composition', () => {
    expect(mapTerminalKey(key('ArrowLeft', { alt: true }, { isComposing: true }), 'prompt')).toBeNull();
    expect(mapTerminalKey(key('Enter', { shift: true }, { isComposing: true }), 'running')).toBeNull();
  });

  it('does nothing for the keyCode 229 that IMEs report during composition', () => {
    expect(mapTerminalKey(key('ArrowLeft', { meta: true }, { keyCode: 229 }), 'prompt')).toBeNull();
  });

  it('maps key repeat, so holding Option+Left keeps moving the cursor', () => {
    expect(mapTerminalKey(key('ArrowLeft', { alt: true }, { repeat: true }), 'prompt')).toBe('\x1bb');
    expect(mapTerminalKey(key('Backspace', { meta: true }, { repeat: true }), 'running')).toBe('\x15');
  });
});

describe('createKeyFilter: the mapped key and its keypress and keyup', () => {
  const press = (k: string, mods: Mods, type: string, extra: Partial<KeyLike> = {}) =>
    key(k, mods, { type, ...extra });

  it('consumes a mapped keydown and returns its data', () => {
    const decide = createKeyFilter();
    expect(decide(press('ArrowLeft', { alt: true }, 'keydown'), 'prompt')).toEqual({ data: '\x1bb', consume: true });
  });

  it('consumes the same key keypress and keyup, so nothing is sent twice', () => {
    const decide = createKeyFilter();
    decide(press('Enter', { shift: true }, 'keydown'), 'running');
    expect(decide(press('Enter', { shift: true }, 'keypress'), 'running')).toEqual({ data: null, consume: true });
    expect(decide(press('Enter', { shift: true }, 'keyup'), 'running')).toEqual({ data: null, consume: true });
  });

  it('after the keyup, the same key passes through again', () => {
    const decide = createKeyFilter();
    decide(press('Enter', { shift: true }, 'keydown'), 'running');
    decide(press('Enter', { shift: true }, 'keyup'), 'running');
    expect(decide(press('Enter', {}, 'keypress'), 'running')).toEqual({ data: null, consume: false });
    expect(decide(press('Enter', {}, 'keydown'), 'running')).toEqual({ data: null, consume: false });
  });

  it('keeps consuming during key repeat until the keyup', () => {
    const decide = createKeyFilter();
    decide(press('ArrowLeft', { alt: true }, 'keydown'), 'prompt');
    expect(decide(press('ArrowLeft', { alt: true }, 'keydown', { repeat: true }), 'prompt')).toEqual({
      data: '\x1bb',
      consume: true,
    });
    expect(decide(press('ArrowLeft', { alt: true }, 'keyup'), 'prompt')).toEqual({ data: null, consume: true });
  });

  it('passes every other key through unchanged', () => {
    const decide = createKeyFilter();
    expect(decide(press('a', {}, 'keydown'), 'prompt')).toEqual({ data: null, consume: false });
    expect(decide(press('a', {}, 'keypress'), 'prompt')).toEqual({ data: null, consume: false });
    expect(decide(press('a', {}, 'keyup'), 'prompt')).toEqual({ data: null, consume: false });
    expect(decide(press('Enter', {}, 'keydown'), 'running')).toEqual({ data: null, consume: false });
  });

  it('passes a key through when its mapping is off in the current state', () => {
    const decide = createKeyFilter();
    expect(decide(press('Enter', { shift: true }, 'keydown'), 'prompt')).toEqual({ data: null, consume: false });
    expect(decide(press('Enter', { shift: true }, 'keypress'), 'prompt')).toEqual({ data: null, consume: false });
    expect(decide(press('ArrowLeft', { alt: true }, 'keydown'), 'alt')).toEqual({ data: null, consume: false });
  });

  it('a non-mapped keydown for a swallowed key clears the swallow, in case its keyup was lost', () => {
    const decide = createKeyFilter();
    decide(press('Enter', { shift: true }, 'keydown'), 'running');
    // The keyup never arrived. Plain Enter is pressed later at a prompt.
    expect(decide(press('Enter', {}, 'keydown'), 'prompt')).toEqual({ data: null, consume: false });
    expect(decide(press('Enter', {}, 'keypress'), 'prompt')).toEqual({ data: null, consume: false });
  });

  it('a different key does not clear the swallow of the key still held', () => {
    const decide = createKeyFilter();
    decide(press('ArrowLeft', { alt: true }, 'keydown'), 'prompt');
    decide(press('x', {}, 'keydown'), 'prompt');
    expect(decide(press('ArrowLeft', { alt: true }, 'keyup'), 'prompt')).toEqual({ data: null, consume: true });
  });
});

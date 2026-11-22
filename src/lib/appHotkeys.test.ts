import { describe, expect, it } from 'vitest';
import { appHotkeyFor } from './appHotkeys';
import type { HotkeyEvent } from './appHotkeys';

interface Mods {
  meta?: boolean;
  ctrl?: boolean;
  alt?: boolean;
  shift?: boolean;
  type?: string;
}

function ev(key: string, mods: Mods = {}, code = ''): HotkeyEvent {
  return {
    type: mods.type ?? 'keydown',
    key,
    code,
    metaKey: mods.meta ?? false,
    ctrlKey: mods.ctrl ?? false,
    altKey: mods.alt ?? false,
    shiftKey: mods.shift ?? false,
  };
}

describe('appHotkeyFor: mappings', () => {
  it('maps Cmd+F to find, in either letter case', () => {
    expect(appHotkeyFor(ev('f', { meta: true }))).toBe('find');
    expect(appHotkeyFor(ev('F', { meta: true }))).toBe('find');
  });

  it('maps Cmd+Shift+F to find-terminal, in either letter case', () => {
    expect(appHotkeyFor(ev('F', { meta: true, shift: true }))).toBe('find-terminal');
    expect(appHotkeyFor(ev('f', { meta: true, shift: true }))).toBe('find-terminal');
  });

  it('maps Cmd+K to clear, in either letter case', () => {
    expect(appHotkeyFor(ev('k', { meta: true }))).toBe('clear');
    expect(appHotkeyFor(ev('K', { meta: true }))).toBe('clear');
  });

  it('maps Cmd+= and Cmd+Shift+= (reported as +) to zoom-in', () => {
    expect(appHotkeyFor(ev('=', { meta: true }))).toBe('zoom-in');
    expect(appHotkeyFor(ev('+', { meta: true, shift: true }))).toBe('zoom-in');
    expect(appHotkeyFor(ev('=', { meta: true, shift: true }))).toBe('zoom-in');
  });

  it('maps Cmd+Minus to zoom-out', () => {
    expect(appHotkeyFor(ev('-', { meta: true }))).toBe('zoom-out');
  });

  it('maps Cmd+0 to zoom-reset', () => {
    expect(appHotkeyFor(ev('0', { meta: true }))).toBe('zoom-reset');
  });

  it('maps the numpad plus and minus keys the same way', () => {
    expect(appHotkeyFor(ev('+', { meta: true }, 'NumpadAdd'))).toBe('zoom-in');
    expect(appHotkeyFor(ev('-', { meta: true }, 'NumpadSubtract'))).toBe('zoom-out');
  });
});

describe('appHotkeyFor: settings (comma)', () => {
  it('maps Cmd+, and Ctrl+, to settings', () => {
    expect(appHotkeyFor(ev(',', { meta: true }))).toBe('settings');
    expect(appHotkeyFor(ev(',', { ctrl: true }))).toBe('settings');
  });

  it('does not map comma with Shift, Option, or both Cmd and Ctrl', () => {
    expect(appHotkeyFor(ev(',', { meta: true, shift: true }))).toBeNull();
    expect(appHotkeyFor(ev(',', { meta: true, alt: true }))).toBeNull();
    expect(appHotkeyFor(ev(',', { meta: true, ctrl: true }))).toBeNull();
    expect(appHotkeyFor(ev(',', { type: 'keyup', meta: true }))).toBeNull();
  });

  it('a bare comma is not a hotkey, so the terminal still gets it', () => {
    expect(appHotkeyFor(ev(','))).toBeNull();
  });
});

describe('appHotkeyFor: negatives', () => {
  it('ignores keys without Cmd', () => {
    for (const k of ['f', 'k', '=', '+', '-', '0']) {
      expect(appHotkeyFor(ev(k))).toBeNull();
    }
  });

  it('ignores Ctrl, Option, and Ctrl+Cmd combinations', () => {
    expect(appHotkeyFor(ev('f', { meta: true, ctrl: true }))).toBeNull();
    expect(appHotkeyFor(ev('k', { meta: true, alt: true }))).toBeNull();
    expect(appHotkeyFor(ev('=', { meta: true, alt: true }))).toBeNull();
    expect(appHotkeyFor(ev('-', { meta: true, ctrl: true }))).toBeNull();
    expect(appHotkeyFor(ev('0', { ctrl: true }))).toBeNull();
  });

  it('ignores Shift on every key except zoom-in and find-terminal', () => {
    expect(appHotkeyFor(ev('K', { meta: true, shift: true }))).toBeNull();
    expect(appHotkeyFor(ev('-', { meta: true, shift: true }))).toBeNull();
    expect(appHotkeyFor(ev(')', { meta: true, shift: true }))).toBeNull();
  });

  it('ignores Cmd+Shift+A, which is the agent bar hotkey and handled elsewhere', () => {
    expect(appHotkeyFor(ev('A', { meta: true, shift: true }))).toBeNull();
    expect(appHotkeyFor(ev('a', { meta: true }))).toBeNull();
  });

  it('ignores keyup and other event types', () => {
    expect(appHotkeyFor(ev('f', { meta: true, type: 'keyup' }))).toBeNull();
    expect(appHotkeyFor(ev('=', { meta: true, type: 'keypress' }))).toBeNull();
  });

  it('ignores unrelated keys with Cmd', () => {
    expect(appHotkeyFor(ev('a', { meta: true }))).toBeNull();
    expect(appHotkeyFor(ev('1', { meta: true }))).toBeNull();
    expect(appHotkeyFor(ev('ArrowLeft', { meta: true }))).toBeNull();
    expect(appHotkeyFor(ev('Backspace', { meta: true }))).toBeNull();
  });
});

describe('appHotkeyFor: Windows and Linux (Ctrl, not Cmd)', () => {
  const win = (k: string, mods: Mods = {}, code = '') => appHotkeyFor(ev(k, mods, code), false);

  it('maps Ctrl+Shift+F to find-terminal and Ctrl+Shift+K to clear, in either letter case', () => {
    expect(win('f', { ctrl: true, shift: true })).toBe('find-terminal');
    expect(win('F', { ctrl: true, shift: true })).toBe('find-terminal');
    expect(win('k', { ctrl: true, shift: true })).toBe('clear');
    expect(win('K', { ctrl: true, shift: true })).toBe('clear');
  });

  it('maps Ctrl+F to find (the focus zone), in either letter case', () => {
    expect(win('f', { ctrl: true })).toBe('find');
    expect(win('F', { ctrl: true })).toBe('find');
  });

  it('leaves Ctrl+K to the shell', () => {
    expect(win('k', { ctrl: true })).toBeNull();
    expect(win('K', { ctrl: true })).toBeNull();
  });

  it('maps Ctrl+= and Ctrl+Shift+= (reported as +) to zoom-in, and the numpad keys too', () => {
    expect(win('=', { ctrl: true })).toBe('zoom-in');
    expect(win('+', { ctrl: true, shift: true })).toBe('zoom-in');
    expect(win('=', { ctrl: true, shift: true })).toBe('zoom-in');
    expect(win('+', { ctrl: true }, 'NumpadAdd')).toBe('zoom-in');
  });

  it('maps Ctrl+- to zoom-out and Ctrl+0 to zoom-reset', () => {
    expect(win('-', { ctrl: true })).toBe('zoom-out');
    expect(win('-', { ctrl: true }, 'NumpadSubtract')).toBe('zoom-out');
    expect(win('0', { ctrl: true })).toBe('zoom-reset');
  });

  it('maps Ctrl+, to settings, and Ctrl+Shift+, to nothing', () => {
    expect(win(',', { ctrl: true })).toBe('settings');
    expect(win(',', { ctrl: true, shift: true })).toBeNull();
  });

  it('does not map the Cmd (Windows) key, Option, or Shift on zoom keys, or Ctrl+Shift+A', () => {
    expect(win('f', { meta: true, shift: true })).toBeNull();
    expect(win('k', { meta: true })).toBeNull();
    expect(win('=', { meta: true, ctrl: true })).toBeNull();
    expect(win('=', { ctrl: true, alt: true })).toBeNull();
    expect(win('-', { ctrl: true, shift: true })).toBeNull();
    expect(win('A', { ctrl: true, shift: true })).toBeNull();
  });

  it('maps keydown only', () => {
    expect(win('f', { ctrl: true, shift: true, type: 'keyup' })).toBeNull();
  });
});

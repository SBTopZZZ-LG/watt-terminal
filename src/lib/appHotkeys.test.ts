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

  it('ignores Shift on every key except zoom-in', () => {
    expect(appHotkeyFor(ev('F', { meta: true, shift: true }))).toBeNull();
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

import { describe, expect, it } from 'vitest';
import { isSelectAllKey, selectAllTarget, type SelectAllKeyEvent } from './selectAll';

/** A keydown with the given key and modifiers. */
function key(k: string, mods: Partial<Pick<SelectAllKeyEvent, 'ctrlKey' | 'metaKey' | 'altKey' | 'shiftKey'>> = {}): SelectAllKeyEvent {
  return { type: 'keydown', key: k, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...mods };
}

describe('isSelectAllKey', () => {
  it('is Ctrl+A on Windows and Cmd+A on macOS, with no other modifier', () => {
    expect(isSelectAllKey(key('a', { ctrlKey: true }), false)).toBe(true);
    expect(isSelectAllKey(key('A', { ctrlKey: true }), false)).toBe(true);
    expect(isSelectAllKey(key('a', { metaKey: true }), true)).toBe(true);
    expect(isSelectAllKey(key('a', { ctrlKey: true }), true)).toBe(false);
    expect(isSelectAllKey(key('a', { metaKey: true }), false)).toBe(false);
  });

  it('excludes Ctrl+Shift+A (the agent bar hotkey), Alt chords, other letters and keyup', () => {
    expect(isSelectAllKey(key('a', { ctrlKey: true, shiftKey: true }), false)).toBe(false);
    expect(isSelectAllKey(key('a', { ctrlKey: true, altKey: true }), false)).toBe(false);
    expect(isSelectAllKey(key('f', { ctrlKey: true }), false)).toBe(false);
    expect(isSelectAllKey({ ...key('a', { ctrlKey: true }), type: 'keyup' }, false)).toBe(false);
  });
});

describe('selectAllTarget', () => {
  const none = { editable: false, inTerminal: false, inAgentPanel: false };

  it('selects the transcript when focus is inside the agent panel', () => {
    expect(selectAllTarget({ ...none, inAgentPanel: true })).toBe('agent');
  });

  it('selects the terminal when focus is elsewhere in the page', () => {
    expect(selectAllTarget(none)).toBe('terminal');
  });

  it('is not ours in a field or in the terminal, so the browser or xterm keeps the key', () => {
    expect(selectAllTarget({ ...none, editable: true, inAgentPanel: true })).toBeNull();
    expect(selectAllTarget({ ...none, editable: true })).toBeNull();
    expect(selectAllTarget({ ...none, inTerminal: true })).toBeNull();
  });
});

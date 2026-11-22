import { describe, expect, it } from 'vitest';
import { type BrowserKeyEvent, installBrowserKeyGuard, isBrowserChromeKey } from './browserKeys';

/** A keydown with the given key and modifiers (node has no KeyboardEvent). */
function key(k: string, mods: Partial<Pick<BrowserKeyEvent, 'ctrlKey' | 'metaKey' | 'altKey' | 'shiftKey'>> = {}): BrowserKeyEvent {
  return { type: 'keydown', key: k, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...mods };
}

describe('isBrowserChromeKey on Windows and Linux', () => {
  const win = (ev: BrowserKeyEvent) => isBrowserChromeKey(ev, false);

  it('cancels reload, F5, F7, F3 and the Ctrl page commands', () => {
    expect(win(key('F5'))).toBe(true);
    expect(win(key('F5', { ctrlKey: true }))).toBe(true);
    expect(win(key('r', { ctrlKey: true }))).toBe(true);
    expect(win(key('R', { ctrlKey: true, shiftKey: true }))).toBe(true);
    expect(win(key('p', { ctrlKey: true }))).toBe(true);
    expect(win(key('u', { ctrlKey: true }))).toBe(true);
    expect(win(key('s', { ctrlKey: true }))).toBe(true);
    expect(win(key('g', { ctrlKey: true }))).toBe(true);
    expect(win(key('F7'))).toBe(true);
    expect(win(key('F3'))).toBe(true);
    expect(win(key('F3', { shiftKey: true }))).toBe(true);
  });

  it('leaves Ctrl+F, Ctrl+A, Ctrl+Shift+P, Ctrl+Shift+S and plain letters alone', () => {
    expect(win(key('f', { ctrlKey: true }))).toBe(false);
    expect(win(key('a', { ctrlKey: true }))).toBe(false);
    expect(win(key('p', { ctrlKey: true, shiftKey: true }))).toBe(false);
    expect(win(key('s', { ctrlKey: true, shiftKey: true }))).toBe(false);
    expect(win(key('r'))).toBe(false);
    expect(win(key('s'))).toBe(false);
  });

  it('leaves Alt chords, Cmd chords and keyup alone', () => {
    expect(win(key('r', { ctrlKey: true, altKey: true }))).toBe(false);
    expect(win(key('r', { metaKey: true }))).toBe(false);
    expect(win(key('F5', { altKey: true }))).toBe(false);
    expect(win({ ...key('r', { ctrlKey: true }), type: 'keyup' })).toBe(false);
  });

  it('takes F5 on Windows and leaves it alone on macOS (the macOS list is the Cmd keys)', () => {
    expect(win(key('F5'))).toBe(true);
    expect(isBrowserChromeKey(key('F5'), true)).toBe(false);
  });
});

describe('isBrowserChromeKey on macOS', () => {
  const mac = (ev: BrowserKeyEvent) => isBrowserChromeKey(ev, true);

  it('cancels Cmd+R (and Cmd+Shift+R), Cmd+P, Cmd+S, Cmd+U and Cmd+G', () => {
    expect(mac(key('r', { metaKey: true }))).toBe(true);
    expect(mac(key('R', { metaKey: true, shiftKey: true }))).toBe(true);
    expect(mac(key('p', { metaKey: true }))).toBe(true);
    expect(mac(key('s', { metaKey: true }))).toBe(true);
    expect(mac(key('u', { metaKey: true }))).toBe(true);
    expect(mac(key('g', { metaKey: true }))).toBe(true);
  });

  it('leaves Cmd+F, Cmd+A, Cmd+K and Ctrl chords alone (the terminal and the app use them)', () => {
    expect(mac(key('f', { metaKey: true }))).toBe(false);
    expect(mac(key('a', { metaKey: true }))).toBe(false);
    expect(mac(key('k', { metaKey: true }))).toBe(false);
    expect(mac(key('r', { ctrlKey: true }))).toBe(false);
    expect(mac(key('s', { ctrlKey: true }))).toBe(false);
    expect(mac(key('p', { metaKey: true, shiftKey: true }))).toBe(false);
  });
});

describe('installBrowserKeyGuard', () => {
  /** Dispatches a keydown on the target; reports whether the default action was cancelled. */
  function press(target: EventTarget, ev: BrowserKeyEvent): boolean {
    const event = new Event('keydown', { cancelable: true });
    Object.assign(event, { key: ev.key, ctrlKey: ev.ctrlKey, metaKey: ev.metaKey, altKey: ev.altKey, shiftKey: ev.shiftKey });
    target.dispatchEvent(event);
    return event.defaultPrevented;
  }

  it('cancels the default of a browser key but does not stop it reaching the page', () => {
    const target = new EventTarget();
    const remove = installBrowserKeyGuard(target, false);
    let reached = false;
    target.addEventListener('keydown', () => {
      reached = true;
    });
    expect(press(target, key('r', { ctrlKey: true }))).toBe(true);
    expect(reached).toBe(true);
    remove();
  });

  it('leaves other keys alone', () => {
    const target = new EventTarget();
    const remove = installBrowserKeyGuard(target, false);
    expect(press(target, key('f', { ctrlKey: true }))).toBe(false);
    remove();
  });

  it('removes its listener', () => {
    const target = new EventTarget();
    const remove = installBrowserKeyGuard(target, false);
    remove();
    expect(press(target, key('r', { ctrlKey: true }))).toBe(false);
  });
});

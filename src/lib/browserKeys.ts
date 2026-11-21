// Browser accelerators that would reload the page or open browser UI (Reload, Print, Save, View Source, Find Next, ...).
// The guard only cancels the default action. It does not stop propagation, so the key still reaches the app and xterm:
// xterm reads keydown on its textarea and does not check defaultPrevented (node_modules/@xterm/xterm/src/browser), so
// Ctrl+R, Ctrl+S, Ctrl+U, Ctrl+P and Ctrl+G still reach the shell. Ctrl+F is not here: it is the find key (a later change).

import { IS_MAC } from './platform';

/** The fields of a keyboard event that the predicate reads. A DOM KeyboardEvent satisfies this. */
export interface BrowserKeyEvent {
  type: string;
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

/**
 * True for a keydown that the webview would act on by itself, which the guard cancels. `mac` picks the macOS or the
 * Windows and Linux list. It defaults to the platform the app runs on.
 */
export function isBrowserChromeKey(ev: BrowserKeyEvent, mac: boolean = IS_MAC): boolean {
  if (ev.type !== 'keydown' || ev.altKey) {
    return false;
  }
  const key = ev.key.toLowerCase();
  // Reload. Shift is allowed here only: Ctrl+Shift+R (Cmd+Shift+R in Safari) is a reload that skips the cache.
  const reloadModifier = mac ? ev.metaKey && !ev.ctrlKey : ev.ctrlKey && !ev.metaKey;
  if (key === 'r' && reloadModifier) {
    return true;
  }
  if (mac) {
    // Cmd+P (print), Cmd+S (save), Cmd+U (view source), Cmd+G (find next). Without Shift, and never with Ctrl.
    return ev.metaKey && !ev.ctrlKey && !ev.shiftKey && (key === 'p' || key === 's' || key === 'u' || key === 'g');
  }
  if (ev.metaKey) {
    return false;
  }
  if (ev.ctrlKey && !ev.shiftKey && (key === 'p' || key === 's' || key === 'u' || key === 'g')) {
    return true;
  }
  // Function keys: F5 reload, F7 caret browsing, F3 find next (Shift+F3 find previous). Ctrl+F5 is also a reload.
  return ev.key === 'F5' || ev.key === 'F7' || ev.key === 'F3';
}

/** Cancels the default action of browser chrome keys in the capture phase. Returns the function that removes the listener. */
export function installBrowserKeyGuard(
  target: Pick<EventTarget, 'addEventListener' | 'removeEventListener'> = document,
  mac: boolean = IS_MAC,
): () => void {
  const cancel = (e: Event) => {
    if (isBrowserChromeKey(e as KeyboardEvent, mac)) {
      e.preventDefault();
    }
  };
  target.addEventListener('keydown', cancel, true);
  return () => target.removeEventListener('keydown', cancel, { capture: true });
}

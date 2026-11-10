// App-level shortcuts on Cmd (macOS). Pure: no DOM, so the mapping is unit-tested. App.tsx dispatches the result from its
// window keydown listener, in the capture phase, and stops the event there so xterm and the PTY never see it.
//
// Tauri's default macOS menu does not register these accelerators, so the keydown reaches the webview. Only Cmd is
// matched, except for the settings key: Cmd+, and Ctrl+, both open the AI settings. Ctrl+comma is not a terminal key
// (xterm sends nothing for it), so it does not clash with the shell. Shift is allowed only for zoom-in, because Cmd+Plus
// is Cmd+Shift+= on a US keyboard. Matching is on the produced key, so it follows the keyboard layout.

export type AppHotkey = 'find' | 'clear' | 'zoom-in' | 'zoom-out' | 'zoom-reset' | 'settings';

/** The fields of a keyboard event that the mapping reads. A DOM KeyboardEvent satisfies this. */
export interface HotkeyEvent {
  type: string;
  key: string;
  code: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

/** The hotkey for this keydown, or null when it is not one of ours. Only keydown counts; any other modifier means null. */
export function appHotkeyFor(ev: HotkeyEvent): AppHotkey | null {
  if (ev.type !== 'keydown' || ev.altKey) {
    return null;
  }
  // Exactly one of Cmd and Ctrl, with no Shift: Cmd+, or Ctrl+,.
  if (ev.key === ',' && !ev.shiftKey && ev.metaKey !== ev.ctrlKey) {
    return 'settings';
  }
  if (!ev.metaKey || ev.ctrlKey) {
    return null;
  }
  const key = ev.key.toLowerCase();
  // Cmd+= and Cmd+Shift+= (which reports '+') both zoom in.
  if (ev.key === '=' || ev.key === '+') {
    return 'zoom-in';
  }
  if (ev.shiftKey) {
    return null;
  }
  switch (key) {
    case 'f':
      return 'find';
    case 'k':
      return 'clear';
    case '-':
      return 'zoom-out';
    case '0':
      return 'zoom-reset';
    default:
      return null;
  }
}

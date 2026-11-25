// App-level shortcuts. Pure: no DOM, so the mapping is unit-tested. App.tsx dispatches the result from its
// window keydown listener, in the capture phase, and stops the event there so xterm and the PTY never see it.
//
// On macOS, Cmd is matched (Tauri's default macOS menu does not register these accelerators, so the keydown reaches the
// webview). Only Cmd is matched, except for the settings key: Cmd+, and Ctrl+, both open the AI settings. Ctrl+comma is
// not a terminal key (xterm sends nothing for it), so it does not clash with the shell. Shift is allowed only for zoom-in,
// because Cmd+Plus is Cmd+Shift+= on a US keyboard. Matching is on the produced key, so it follows the keyboard layout.
//
// Elsewhere, Ctrl is matched, with Shift for the terminal find and Clear (Windows Terminal's keys). Ctrl+K and Ctrl+L belong
// to the shell (readline, PSReadLine), so they are never taken there.
//
// Find comes in two keys. 'find' (Cmd+F, Ctrl+F) finds in the focus zone: the chat when focus is in the agent panel, else
// the terminal. 'find-terminal' (Cmd+Shift+F on macOS, Ctrl+Shift+F elsewhere) always finds in the terminal.

import { IS_MAC } from './platform';

export type AppHotkey = 'find' | 'find-terminal' | 'clear' | 'zoom-in' | 'zoom-out' | 'zoom-reset' | 'settings';

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

/**
 * The hotkey for this keydown, or null when it is not one of ours. Only keydown counts; any other modifier means null.
 * `mac` picks the macOS or the Windows and Linux mapping. It defaults to the platform the app runs on.
 * `inTerminal` is true when focus is in the terminal (xterm). Off macOS, plain Ctrl+F then goes to the shell (vim, less,
 * readline forward-char), and only Ctrl+Shift+F finds there. macOS is unchanged.
 */
export function appHotkeyFor(ev: HotkeyEvent, mac: boolean = IS_MAC, inTerminal = false): AppHotkey | null {
  if (ev.type !== 'keydown' || ev.altKey) {
    return null;
  }
  if (mac) {
    return macHotkeyFor(ev);
  }
  if (inTerminal && ev.ctrlKey && !ev.shiftKey && ev.key.toLowerCase() === 'f') {
    return null;
  }
  return ctrlHotkeyFor(ev);
}

function macHotkeyFor(ev: HotkeyEvent): AppHotkey | null {
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
    return key === 'f' ? 'find-terminal' : null;
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

function ctrlHotkeyFor(ev: HotkeyEvent): AppHotkey | null {
  // Exactly Ctrl (the Windows key is not one of ours), with Shift only for Find and Clear.
  if (!ev.ctrlKey || ev.metaKey) {
    return null;
  }
  if (ev.key === ',' && !ev.shiftKey) {
    return 'settings';
  }
  // Ctrl+= and Ctrl+Shift+= (which reports '+') both zoom in.
  if (ev.key === '=' || ev.key === '+') {
    return 'zoom-in';
  }
  if (ev.shiftKey) {
    switch (ev.key.toLowerCase()) {
      case 'f':
        return 'find-terminal';
      case 'k':
        return 'clear';
      default:
        return null;
    }
  }
  if (ev.key.toLowerCase() === 'f') {
    return 'find';
  }
  switch (ev.key) {
    case '-':
      return 'zoom-out';
    case '0':
      return 'zoom-reset';
    default:
      return null;
  }
}

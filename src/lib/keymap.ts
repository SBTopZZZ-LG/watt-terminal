// Keys the terminal rewrites before xterm sees them. Pure: no DOM and no xterm, so the rules are unit-tested.
//
// xterm.js sends ESC[1;3D for Option+Left, which zsh does not bind (it prints "D"). Cmd+Left and Cmd+Right send
// nothing, Cmd+Backspace sends DEL, and Shift+Enter is a plain CR. These keys are rewritten to what zsh and
// programs such as Claude Code understand. The Option and Cmd rules only apply while the shell owns the keyboard: a
// full-screen app (alt) gets xterm's default encoding for them, and an exited shell gets nothing. Shift+Enter sends a
// newline (Ctrl+J) to a running program, the raw state or a full-screen app, where Claude Code reads it as "insert
// newline". At a prompt it is left alone, because Ctrl+J is accept-line in zsh.

import type { InputState } from './types';

/** The fields of a keyboard event that the mapping reads. A DOM KeyboardEvent satisfies this. */
export interface KeyLike {
  type: string;
  key: string;
  code: string;
  keyCode: number;
  repeat: boolean;
  isComposing: boolean;
  altKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
}

const ESC = '\x1b';

/** True when `alt` is the only modifier held. */
function onlyAlt(ev: KeyLike): boolean {
  return ev.altKey && !ev.metaKey && !ev.ctrlKey && !ev.shiftKey;
}

/** True when `meta` (Cmd) is the only modifier held. */
function onlyMeta(ev: KeyLike): boolean {
  return ev.metaKey && !ev.altKey && !ev.ctrlKey && !ev.shiftKey;
}

/** True when Shift is the only modifier held. */
function onlyShift(ev: KeyLike): boolean {
  return ev.shiftKey && !ev.altKey && !ev.metaKey && !ev.ctrlKey;
}

/**
 * The data to send in place of this keydown, or null to let xterm handle it.
 * Only keydown is mapped. Key repeat is mapped too, so holding Option+Left keeps moving.
 */
export function mapTerminalKey(ev: KeyLike, state: InputState): string | null {
  if (ev.type !== 'keydown' || ev.isComposing || ev.keyCode === 229) {
    return null;
  }
  // Shift+Enter sends a newline to a program that owns the keyboard. At a prompt it stays a plain Enter.
  if (onlyShift(ev) && ev.key === 'Enter') {
    return state === 'running' || state === 'raw' || state === 'alt' ? '\n' : null;
  }
  if (state === 'alt' || state === 'exited') {
    return null;
  }
  if (onlyAlt(ev)) {
    if (ev.key === 'ArrowLeft') {
      return `${ESC}b`;
    }
    if (ev.key === 'ArrowRight') {
      return `${ESC}f`;
    }
    return null;
  }
  if (onlyMeta(ev)) {
    if (ev.key === 'ArrowLeft') {
      return '\x01';
    }
    if (ev.key === 'ArrowRight') {
      return '\x05';
    }
    if (ev.key === 'Backspace') {
      return '\x15';
    }
    return null;
  }
  return null;
}

export interface KeyDecision {
  /** Data to send to the PTY in place of the key, or null. */
  data: string | null;
  /** True when xterm must not see the event. */
  consume: boolean;
}

/**
 * Makes one filter per terminal. A mapped keydown is consumed, and so are the same key's keypress and keyup. Without
 * that, xterm would also send the key (Enter as CR on keypress). The swallowed key is cleared on keyup. A keydown
 * for that key that is not mapped also clears it, in case the keyup was lost.
 */
export function createKeyFilter(): (ev: KeyLike, state: InputState) => KeyDecision {
  let swallowed: string | null = null;
  return (ev, state) => {
    if (ev.type === 'keydown') {
      const data = mapTerminalKey(ev, state);
      if (data !== null) {
        swallowed = ev.code;
        return { data, consume: true };
      }
      if (swallowed === ev.code) {
        swallowed = null;
      }
      return { data: null, consume: false };
    }
    if (swallowed !== null && ev.code === swallowed) {
      if (ev.type === 'keyup') {
        swallowed = null;
      }
      return { data: null, consume: true };
    }
    return { data: null, consume: false };
  };
}

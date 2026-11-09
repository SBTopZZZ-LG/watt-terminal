// What Cmd+K does, decided from the xterm buffer and the effective input state. Pure: no xterm, so it is unit-tested.
// terminal.ts's clearTerminal carries out the decision.

import type { InputState } from './types';

export interface ClearActions {
  /** Write ESC[3J locally: drop the scrollback (and the command stripes on it). The shell is not told. */
  local3J: boolean;
  /** Send Ctrl-L to the PTY so the shell redraws its prompt at the top. */
  ctrlL: boolean;
}

/**
 * Full-screen apps (alternate buffer, or the alt state) get nothing: the key belongs to them. Otherwise the scrollback is
 * cleared locally. Ctrl-L is sent only at the prompt, where the shell is idle and redrawing it is safe. Running, continuation,
 * submitted, raw, booting and exited get the local clear alone.
 */
export function clearActions(bufferType: 'normal' | 'alternate', state: InputState): ClearActions {
  if (bufferType === 'alternate' || state === 'alt') {
    return { local3J: false, ctrlL: false };
  }
  return { local3J: true, ctrlL: state === 'prompt' };
}

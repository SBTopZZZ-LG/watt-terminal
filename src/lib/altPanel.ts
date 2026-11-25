// The agent panel while a full-screen program runs (the alternate screen: vim, less, OpenCode). The panel closes when one
// starts, so the program gets the window, and opens again when it ends. Pure: App.tsx feeds it the UI state and the user's own
// panel actions, and applies the open state it returns.
//
// The rules:
// - Entering the alternate screen closes an open panel, and remembers that it did (`hidden`). A panel that was already closed
//   stays closed and is not remembered.
// - Leaving the alternate screen (the program ends, or the shell exits) reopens the panel only when this module closed it.
// - A user action during a full-screen run (show, hide, toggle) takes control: the panel is left alone for the rest of that
//   command, even when the program flaps in and out of the alternate screen. The command ending (back at a prompt, the shell
//   booting or exited) gives control back.
// - The saved layout is the user's, not the hidden one: while hidden, the saved open state is true (see layoutToSave).

import type { InputState } from './types';
import type { PanelLayout } from './panelLayout';

export interface AltPanelState {
  /** The alternate screen was up at the last state event. */
  fullScreen: boolean;
  /** This module closed the panel and has not been told otherwise. */
  hidden: boolean;
  /** The user showed, hid or toggled the panel during the current command. */
  userControl: boolean;
}

export const INITIAL_ALT_PANEL: AltPanelState = { fullScreen: false, hidden: false, userControl: false };

export type AltPanelEvent =
  /** The UI state changed. `open` is the panel's open state now. */
  | { type: 'state'; state: InputState; open: boolean }
  /** The user showed, hid or toggled the panel (a button, the hotkey, a toast, the /history command). */
  | { type: 'user' };

export interface AltPanelStep {
  state: AltPanelState;
  /** The open state to apply to the panel, or null to leave it alone. */
  open: boolean | null;
}

/** The step for one event. Pure: the caller applies `open` and keeps `state`. */
export function altPanelStep(s: AltPanelState, e: AltPanelEvent): AltPanelStep {
  if (e.type === 'user') {
    // Outside a full-screen run the panel is the user's as it always was: nothing to remember.
    if (!s.fullScreen) {
      return { state: s, open: null };
    }
    return { state: { fullScreen: true, hidden: false, userControl: true }, open: null };
  }
  if (e.state === 'alt') {
    if (s.fullScreen) {
      return { state: s, open: null };
    }
    // Entering. A panel the user has taken control of, or one that is already closed, is left as it is.
    if (!e.open || s.userControl) {
      return { state: { fullScreen: true, hidden: false, userControl: s.userControl }, open: null };
    }
    return { state: { fullScreen: true, hidden: true, userControl: false }, open: false };
  }
  // Not on the alternate screen: reopen only what this module hid. The command ends at a prompt, a booting or an exited shell.
  const commandEnded = e.state === 'prompt' || e.state === 'booting' || e.state === 'exited';
  return {
    state: { fullScreen: false, hidden: false, userControl: commandEnded ? false : s.userControl },
    open: s.hidden ? true : null,
  };
}

/** The layout to save. While the panel is hidden for a full-screen program, the saved open state is the user's: open. */
export function layoutToSave(layout: PanelLayout, s: AltPanelState): PanelLayout {
  return s.hidden ? { ...layout, open: true } : layout;
}

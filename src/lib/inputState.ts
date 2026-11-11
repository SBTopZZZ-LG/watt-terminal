// Input routing for the UI. Rust is authoritative for gating (it refuses agent commands outside Prompt). This module
// only decides what the UI does: where keystrokes go, what the input bar offers, and where focus sits.
//
// Inputs: the InputState that Rust reports, and the alternate-screen flag from xterm's buffer. The alt flag is
// an immediate local override, so vim and friends take over the keyboard without waiting for a round trip.

import { foregroundHint, isUnintegrated } from './foreground';
import type { Foreground, InputState } from './types';

export interface InputModel {
  rust: InputState;
  alt: boolean;
  /** The foreground hint Rust reports with the state. Null when there is none (see foreground.ts). */
  foreground: Foreground | null;
}

export const INITIAL_INPUT: InputModel = { rust: 'booting', alt: false, foreground: null };

export type InputAction =
  | { type: 'rust_state'; state: InputState; foreground?: Foreground | null }
  | { type: 'alt'; active: boolean };

function sameForeground(a: Foreground | null, b: Foreground | null): boolean {
  return a === b || (a !== null && b !== null && a.kind === b.kind && a.command === b.command);
}

export function inputReducer(m: InputModel, a: InputAction): InputModel {
  switch (a.type) {
    case 'rust_state': {
      // The hint travels with the state: a state event without one (a prompt, or any state after a command) clears it.
      const foreground = a.foreground ?? null;
      if (m.rust === a.state && sameForeground(m.foreground, foreground)) {
        return m;
      }
      return { ...m, rust: a.state, foreground };
    }
    case 'alt':
      return m.alt === a.active ? m : { ...m, alt: a.active };
  }
}

/** The state the UI acts on. Exited wins; otherwise the alternate screen overrides whatever Rust reports. */
export function effectiveState(m: InputModel): InputState {
  if (m.rust === 'exited') {
    return 'exited';
  }
  if (m.alt) {
    return 'alt';
  }
  return m.rust;
}

/** Where xterm's keystrokes go: to the PTY, or dropped. Only an exited shell drops them. */
export type KeyRoute = 'pty' | 'drop';

/**
 * What the input bar offers:
 * - chat: the agent message box, in every state except the alternate screen and an exited shell
 * - collapsed: a thin strip while a full-screen app runs; a hotkey opens the chat for one message
 * - exited: the shell has gone; a button starts a new one in place of the input
 */
export type BarMode = 'chat' | 'collapsed' | 'exited';

export interface Route {
  keys: KeyRoute;
  bar: BarMode;
  /** What the bar says about the state, or null when there is nothing to say. */
  hint: string | null;
  /**
   * Whether the hint shows as the input's placeholder instead of its own row. The two transient states (a command
   * starting, a command running) use it, so the bar keeps its height while a command runs and the terminal does not
   * resize. Every other hint keeps its own row.
   */
  hintInline: boolean;
}

/**
 * The route for a state. `foreground` only matters while a command runs: a program without shell integration (ssh, a
 * nested shell...) names itself in the hint. An ordinary command keeps the generic hint.
 */
export function routeFor(state: InputState, foreground: Foreground | null = null): Route {
  switch (state) {
    case 'booting':
      return {
        keys: 'pty',
        bar: 'chat',
        hint: 'The shell is starting. The agent cannot run commands yet.',
        hintInline: false,
      };
    case 'prompt':
      return { keys: 'pty', bar: 'chat', hint: null, hintInline: false };
    case 'submitted':
      return {
        keys: 'pty',
        bar: 'chat',
        hint: 'The command is starting. The agent cannot run commands until it finishes.',
        hintInline: true,
      };
    case 'continuation':
      return {
        keys: 'pty',
        bar: 'chat',
        hint: 'The shell is waiting for the rest of a multi-line command. The agent cannot run commands now.',
        hintInline: false,
      };
    case 'unresponsive':
      // Keys still go to the shell, so Ctrl-C reaches it. The agent cannot run commands until the shell answers.
      return {
        keys: 'pty',
        bar: 'chat',
        hint: 'The shell is not answering. Press Ctrl-C or wait.',
        hintInline: false,
      };
    case 'running':
      return {
        keys: 'pty',
        bar: 'chat',
        hint: isUnintegrated(foreground)
          ? foregroundHint(foreground)
          : 'A command is running. The agent cannot run another until it finishes.',
        hintInline: true,
      };
    case 'alt':
      return {
        keys: 'pty',
        bar: 'collapsed',
        hint: 'Full-screen app: keys go to it. Ctrl/Cmd+Shift+A messages the agent.',
        hintInline: false,
      };
    case 'raw':
      return {
        keys: 'pty',
        bar: 'chat',
        hint: 'No shell integration: the agent can read the screen but cannot run commands.',
        hintInline: false,
      };
    case 'exited':
      return { keys: 'drop', bar: 'exited', hint: 'Shell exited.', hintInline: false };
  }
}

/**
 * Focus target when the effective state changes from `prev` to `next`, or null to leave focus alone.
 *
 * Focus does not follow the shell's state. A command starting or finishing, or a prompt coming back, never moves
 * focus, so text being typed in the bar is never taken away. There are two exceptions, both at the ends of a shell:
 * - the shell exits: the bar takes focus, so its restart button can take Enter (the terminal no longer takes keys);
 * - a new shell starts after an exit: the terminal takes focus again.
 * The terminal also takes focus on mount, and the hotkey and Esc move focus explicitly (see App.tsx).
 */
export function focusAfter(prev: InputState, next: InputState): 'xterm' | 'bar' | null {
  if (prev === next) {
    return null;
  }
  if (next === 'exited') {
    return 'bar';
  }
  if (prev === 'exited') {
    return 'xterm';
  }
  return null;
}

/**
 * The hotkey that focuses the agent bar: Ctrl+Shift+A, or Cmd+Shift+A on macOS. Matched on the letter, so the Shift key
 * does not change it. App.tsx handles it in the capture phase, so xterm does not also receive it.
 */
export function isFocusBarHotkey(e: { key: string; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean }): boolean {
  return (e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === 'a';
}

export type BarKey =
  | 'interrupt'
  | 'approve_first'
  | 'deny_first'
  | 'submit'
  | 'close'
  | 'history_prev'
  | 'history_next'
  | 'none';

export interface BarKeyContext {
  inputEmpty: boolean;
  hasPendingApproval: boolean;
}

/**
 * What a key pressed in the input bar means. Ctrl-C is always `interrupt` (the caller sends ^C to the PTY),
 * whatever the state. Enter approves the first pending approval when the input is empty; Escape denies it.
 * Otherwise Escape closes the bar's focus (back to the terminal).
 */
export function barKeyIntent(
  key: string,
  mods: { ctrl: boolean; shift: boolean },
  ctx: BarKeyContext,
): BarKey {
  if (mods.ctrl && !mods.shift && key.toLowerCase() === 'c') {
    return 'interrupt';
  }
  switch (key) {
    case 'Enter':
      return ctx.inputEmpty && ctx.hasPendingApproval ? 'approve_first' : 'submit';
    case 'Escape':
      return ctx.hasPendingApproval ? 'deny_first' : 'close';
    case 'ArrowUp':
      return 'history_prev';
    case 'ArrowDown':
      return 'history_next';
    default:
      return 'none';
  }
}

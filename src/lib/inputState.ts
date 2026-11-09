// Input routing for the UI. Rust is authoritative for gating (it refuses submits outside Prompt). This module
// only decides what the UI does: where keystrokes go, whether the input bar is usable, and where focus sits.
//
// Inputs: the InputState that Rust reports, and the alternate-screen flag from xterm's buffer. The alt flag is
// an immediate local override, so vim and friends take over the keyboard without waiting for a round trip.

import type { InputState } from './types';

export interface InputModel {
  rust: InputState;
  alt: boolean;
}

export const INITIAL_INPUT: InputModel = { rust: 'booting', alt: false };

export type InputAction = { type: 'rust_state'; state: InputState } | { type: 'alt'; active: boolean };

export function inputReducer(m: InputModel, a: InputAction): InputModel {
  switch (a.type) {
    case 'rust_state':
      return m.rust === a.state ? m : { ...m, rust: a.state };
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

/** Where xterm's keystrokes go: to the PTY, or dropped (the input bar owns input at the prompt). */
export type KeyRoute = 'pty' | 'drop';

/**
 * What the input bar offers:
 * - full: shell and agent input
 * - agent_only: agent messages only
 * - collapsed: a thin strip; a hotkey opens it for an agent message
 * - off: disabled, with the hint saying why
 * - exited: the shell has gone; a button starts a new one in place of the input
 */
export type BarMode = 'full' | 'agent_only' | 'collapsed' | 'off' | 'exited';

export interface Route {
  keys: KeyRoute;
  bar: BarMode;
  hint: string | null;
}

export function routeFor(state: InputState): Route {
  switch (state) {
    case 'booting':
      return { keys: 'drop', bar: 'off', hint: 'Starting the shell and its integration...' };
    case 'prompt':
      return { keys: 'drop', bar: 'full', hint: null };
    case 'submitted':
      return { keys: 'drop', bar: 'off', hint: 'Waiting for the command to start...' };
    case 'continuation':
      return {
        keys: 'pty',
        bar: 'agent_only',
        hint: 'The shell is waiting for the rest of a multi-line command. Shell commands are off.',
      };
    case 'running':
      return { keys: 'pty', bar: 'agent_only', hint: 'A command is running. Shell commands are off until it finishes.' };
    case 'alt':
      return { keys: 'pty', bar: 'collapsed', hint: 'Full-screen app: keys go to it. Ctrl+Shift+A messages the agent.' };
    case 'raw':
      return {
        keys: 'pty',
        bar: 'agent_only',
        hint: 'No shell integration: the agent can read the screen but cannot run commands.',
      };
    case 'exited':
      return { keys: 'drop', bar: 'exited', hint: 'Shell exited.' };
  }
}

/**
 * Focus target when the effective state changes from `prev` to `next`, or null to leave focus alone. Focus follows
 * the routing: when keys start going to the PTY, xterm takes them; when the bar comes back (the prompt, or the
 * exited notice with its button), the bar takes focus. A move between two PTY-routed states keeps focus where it is.
 *
 * Why: a stale focus request from the bar must never pull focus away from a running program, so the bar only
 * takes focus through this function (or an explicit hotkey or click while keys are not going to the PTY).
 */
export function focusAfter(prev: InputState, next: InputState): 'xterm' | 'bar' | null {
  if (prev === next) {
    return null;
  }
  if (routeFor(next).keys === 'pty') {
    return routeFor(prev).keys === 'pty' ? null : 'xterm';
  }
  if (next === 'prompt' || next === 'exited') {
    return 'bar';
  }
  return null;
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

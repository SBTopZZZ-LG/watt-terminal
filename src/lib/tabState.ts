// The state one terminal (tab) shows, and the pure steps that change it. No xterm, no DOM: TabController (tab.ts) owns
// the side effects and calls these.

import { agentControlAfter, NO_CONTROL, settleControl } from './agentControl';
import type { AgentControl } from './agentControl';
import { nestedLevels } from './integrate';
import { inputReducer, INITIAL_INPUT } from './inputState';
import type { InputModel } from './inputState';
import type { Cwd, Foreground, IntegrationStatus, LevelInfo, ObserverStatus, SessionEvent, Status, WireEvent } from './types';

export interface TabState {
  input: InputModel;
  integration: IntegrationStatus;
  observer: ObserverStatus;
  cwd: Cwd | null;
  levels: LevelInfo[];
  agentControl: AgentControl;
  /** The title xterm reports (OSC 0/2), null until one arrives. */
  title: string | null;
  /** A restart of this tab's shell is in flight. */
  restarting: boolean;
  /** The chat the terminal belongs to (list_tabs), or null when no chat sees it. */
  owner: string | null;
}

export const INITIAL_TAB_STATE: TabState = {
  input: INITIAL_INPUT,
  integration: 'booting',
  observer: 'ok',
  cwd: null,
  levels: [],
  agentControl: NO_CONTROL,
  title: null,
  restarting: false,
  owner: null,
};

/** Whether a wire event is one of the terminal's own (the tab channel) rather than an agent event (the chat channel). */
export function isSessionEvent(ev: WireEvent): ev is SessionEvent {
  switch (ev.type) {
    case 'state':
    case 'integration':
    case 'observer':
    case 'cwd':
    case 'record_started':
    case 'record_finished':
    case 'levels':
      return true;
    default:
      return false;
  }
}

/** The state after one event from the terminal's channel. Every event goes through the agent-control hold, as before. */
export function reduceTabEvent(s: TabState, ev: SessionEvent): TabState {
  const agentControl = agentControlAfter(s.agentControl, ev);
  switch (ev.type) {
    case 'state':
      return {
        ...s,
        agentControl,
        input: inputReducer(s.input, { type: 'rust_state', state: ev.state, foreground: ev.foreground ?? null }),
      };
    case 'integration':
      return { ...s, agentControl, integration: ev.status };
    case 'observer':
      return { ...s, agentControl, observer: ev.status };
    case 'cwd':
      return { ...s, agentControl, cwd: ev.cwd };
    case 'levels':
      return { ...s, agentControl, levels: nestedLevels(ev.levels) };
    default:
      return agentControl === s.agentControl ? s : { ...s, agentControl };
  }
}

/**
 * The tab's part of a status read. The levels are taken as the status gives them (not through nestedLevels), as before.
 * Nonces and marks are the controller's, not this reducer's.
 */
export function reduceStatus(s: TabState, status: Status): TabState {
  return {
    ...s,
    input: inputReducer(s.input, { type: 'rust_state', state: status.state, foreground: status.foreground ?? null }),
    integration: status.integration,
    observer: status.observer,
    cwd: status.cwd,
    levels: status.levels,
    agentControl: settleControl(s.agentControl, status),
  };
}

/**
 * The title a tab shows: the xterm title when it is not blank, else the foreground command's name, else the working
 * directory's last segment, else `Terminal <index>`.
 */
export function pickTabTitle(p: { xtermTitle: string | null; foreground: Foreground | null | undefined; cwd: Cwd | null; index: number }): string {
  const title = p.xtermTitle?.trim() ?? '';
  if (title !== '') {
    return title;
  }
  const command = p.foreground?.command.trim() ?? '';
  if (command !== '') {
    return command;
  }
  const base = cwdBasename(p.cwd?.path);
  if (base !== '') {
    return base;
  }
  return `Terminal ${p.index}`;
}

function cwdBasename(path: string | undefined): string {
  if (path === undefined) {
    return '';
  }
  const trimmed = path.replace(/[\\/]+$/, '');
  const parts = trimmed.split(/[\\/]/);
  return parts[parts.length - 1] ?? '';
}

/**
 * The nonces one tab accepts for its OSC events: the list Rust sent, blanks dropped. Each tab keeps its own set, so a nonce
 * of one shell is never accepted on another tab.
 */
export function acceptedNonces(list: readonly string[]): ReadonlySet<string> {
  return new Set(list.filter((n) => n !== ''));
}

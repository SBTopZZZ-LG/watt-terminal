// Whether the agent is in control of the shell. It drives the green border around the terminal. Pure: App feeds every
// WireEvent through it, in order.

import type { WireEvent } from './types';

export interface AgentControl {
  /** An approved run_command or send_keys is executing. Rust sends agent_control true before the call, false after. */
  call: boolean;
  /** A command the agent started has not finished yet. Outlives the call when the call timed out and the command kept running. */
  command: boolean;
}

export const NO_CONTROL: AgentControl = { call: false, command: false };

/**
 * The hold after `ev`. The call flag: set by agent_control, cleared by its false. The command flag: set when a record the
 * agent started begins, cleared when that record finishes. Both clear when the shell exits, and the call flag also clears
 * when the agent's turn ends (`done`), so a missed event cannot leave the border on. Other events leave it as it is.
 */
export function agentControlAfter(c: AgentControl, ev: WireEvent): AgentControl {
  switch (ev.type) {
    case 'agent_control':
      return c.call === ev.active ? c : { ...c, call: ev.active };
    case 'done':
      return c.call ? { ...c, call: false } : c;
    case 'record_started':
      return ev.source.kind === 'agent' && !c.command ? { ...c, command: true } : c;
    case 'record_finished':
      return ev.record.source.kind === 'agent' && c.command ? { ...c, command: false } : c;
    case 'state':
      return ev.state === 'exited' ? NO_CONTROL : c;
    default:
      return c;
  }
}

/** The agent holds the shell: the border is on while either hold is. */
export function borderActive(c: AgentControl): boolean {
  return c.call || c.command;
}

/**
 * How strong the border is. 'control': the agent holds the shell (see borderActive). 'working': a turn is in flight
 * (`busy`, the transcript's isBusy, from the send until `done`) but the agent is not holding the shell yet, so it is
 * thinking, streaming, or waiting for an approval. 'off': neither.
 */
export type BorderLevel = 'off' | 'working' | 'control';

export function borderLevel(c: AgentControl, busy: boolean): BorderLevel {
  if (borderActive(c)) {
    return 'control';
  }
  return busy ? 'working' : 'off';
}

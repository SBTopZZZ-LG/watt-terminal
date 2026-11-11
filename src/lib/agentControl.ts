// Whether the agent is in control of the shell. It drives the green border around the terminal. Pure: App feeds every
// WireEvent through it, in order.

import type { WireEvent } from './types';

export interface AgentControl {
  /** An approved run_command or send_keys is executing. Rust sends agent_control true before the call, false after. */
  call: boolean;
  /**
   * The ids of the commands the agent started that have not finished. Several can be open at once: a command run inside a
   * remote shell keeps the parked ssh command open. Outlives the call when the call timed out and the command kept running.
   */
  commands: ReadonlySet<number>;
}

const NO_COMMANDS: ReadonlySet<number> = new Set<number>();

export const NO_CONTROL: AgentControl = { call: false, commands: NO_COMMANDS };

/**
 * The hold after `ev`. The call flag: set by agent_control, cleared by its false. The commands: a record the agent started is
 * added when it begins and removed when that record finishes, so one command finishing leaves the others open. Everything
 * clears when the shell exits, and the call flag also clears when the agent's turn ends (`done`), so a missed event cannot
 * leave the border on. Other events leave it as it is.
 */
export function agentControlAfter(c: AgentControl, ev: WireEvent): AgentControl {
  switch (ev.type) {
    case 'agent_control':
      return c.call === ev.active ? c : { ...c, call: ev.active };
    case 'done':
      return c.call ? { ...c, call: false } : c;
    case 'record_started': {
      if (ev.source.kind !== 'agent' || c.commands.has(ev.id)) {
        return c;
      }
      return { ...c, commands: new Set([...c.commands, ev.id]) };
    }
    case 'record_finished': {
      if (!c.commands.has(ev.record.id)) {
        return c;
      }
      const commands = new Set(c.commands);
      commands.delete(ev.record.id);
      return { ...c, commands };
    }
    case 'state':
      return ev.state === 'exited' ? NO_CONTROL : c;
    default:
      return c;
  }
}

/** The agent holds the shell: the border is on while either hold is. */
export function borderActive(c: AgentControl): boolean {
  return c.call || c.commands.size > 0;
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

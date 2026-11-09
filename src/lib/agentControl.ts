// Whether the agent is in control of the shell. It drives the green border around the terminal. Pure: App feeds every
// WireEvent through it, in order.

import type { WireEvent } from './types';

/**
 * The flag after `ev`. Rust sends `agent_control` true just before an approved run_command or send_keys executes, and
 * false just after it. The flag also clears when the shell exits and when the agent's turn ends (`done`), so a missed
 * event cannot leave the border on. Other events leave the flag as it is.
 */
export function agentControlAfter(active: boolean, ev: WireEvent): boolean {
  switch (ev.type) {
    case 'agent_control':
      return ev.active;
    case 'done':
      return false;
    case 'state':
      return ev.state === 'exited' ? false : active;
    default:
      return active;
  }
}

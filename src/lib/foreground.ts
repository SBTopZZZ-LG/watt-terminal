// The foreground hint as the UI shows it (see core/foreground.rs on the Rust side). A hint names a program with no shell
// integration (ssh, a nested shell, su, a container exec, a multiplexer): the agent can read its screen but cannot run
// commands in it. An ordinary program (`other`) and no hint at all keep the UI exactly as it was.

import type { Foreground, ForegroundKind, InputState } from './types';

/** The longest command the status chip shows. Longer ones are cut and end in an ellipsis (the chip's total is this many). */
export const CHIP_COMMAND_CHARS = 28;

/** What each program is, for the chip's title. `other` is never shown, so it has no entry. */
const WHERE: Record<Exclude<ForegroundKind, 'other'>, string> = {
  ssh: 'a shell on another machine',
  shell: 'a nested shell',
  su: "another user's shell",
  container_exec: 'a shell in a container',
  multiplexer: 'a terminal multiplexer',
};

/** True when the hint names a program without shell integration: every kind but `other`. */
export function isUnintegrated(fg: Foreground | null | undefined): fg is Foreground {
  return fg !== null && fg !== undefined && fg.kind !== 'other';
}

/** The command cut to `max` characters (code points), the last one an ellipsis when it was cut. */
export function truncateCommand(command: string, max: number = CHIP_COMMAND_CHARS): string {
  const chars = [...command];
  return chars.length > max ? `${chars.slice(0, max - 1).join('')}…` : command;
}

/** The status chip's text and title while a program without integration runs, or null when the chip stays as it is. */
export function foregroundChip(state: InputState, fg: Foreground | null | undefined): { label: string; title: string } | null {
  if (state !== 'running' || !isUnintegrated(fg)) {
    return null;
  }
  return {
    label: `RUNNING · ${truncateCommand(fg.command)}`,
    title: `${fg.command}: ${WHERE[fg.kind as Exclude<ForegroundKind, 'other'>]}. It has no shell integration, so the agent can read the screen but cannot run commands here.`,
  };
}

/** The input bar's hint while a running command blocks the agent, when that command is a program without integration. */
export function foregroundHint(fg: Foreground): string {
  return `${fg.command} is running without shell integration. The agent can read the screen but cannot run commands here.`;
}

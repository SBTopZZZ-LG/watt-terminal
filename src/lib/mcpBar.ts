import type { McpServerStatus, McpStatus } from './types';

/** The status dot's colour: off (nothing enabled, or a disabled server), connecting, connected (ok), or failed. */
export type McpTone = 'off' | 'connecting' | 'ok' | 'fail';

/** One server's dot: off when disabled, red when it failed or needs sign-in, amber while it connects, green when connected. */
export function serverTone(server: McpServerStatus): McpTone {
  if (!server.enabled) {
    return 'off';
  }
  switch (server.state.kind) {
    case 'connected':
      return 'ok';
    case 'connecting':
      return 'connecting';
    case 'failed':
    case 'needs_auth':
      return 'fail';
    default:
      return 'off';
  }
}

/** The composer button's dot: red if any enabled server failed, else amber while one connects, else green if one is connected. Off when none is enabled. */
export function barTone(servers: McpServerStatus[]): McpTone {
  const tones = servers.map(serverTone);
  if (tones.includes('fail')) {
    return 'fail';
  }
  if (tones.includes('connecting')) {
    return 'connecting';
  }
  return tones.includes('ok') ? 'ok' : 'off';
}

/** The enabled servers that are not connecting yet (connected or failed), and how many are enabled. */
export function readyCount(servers: McpServerStatus[]): { ready: number; total: number } {
  const enabled = servers.filter((s) => s.enabled);
  return { ready: enabled.filter((s) => s.state.kind !== 'connecting').length, total: enabled.length };
}

/** The loader row's text. The count is shown only when more than one server is enabled. */
export function loaderText(servers: McpServerStatus[]): string {
  const { ready, total } = readyCount(servers);
  return total > 1 ? `Starting MCP servers… (${ready}/${total} ready)` : 'Starting MCP servers…';
}

/** A server's state in a few words, for its row in the panel. */
export function stateLabel(server: McpServerStatus): string {
  const { state } = server;
  switch (state.kind) {
    case 'disabled':
      return 'off';
    case 'connecting':
      return 'connecting…';
    case 'connected':
      return state.tools === 1 ? '1 tool' : `${state.tools} tools`;
    case 'failed':
      return 'failed';
    case 'needs_auth':
      return 'needs sign-in';
  }
}

/** The failure message shown in the row's tooltip, or undefined when the state has none. */
export function stateMessage(server: McpServerStatus): string | undefined {
  const { state } = server;
  return state.kind === 'failed' || state.kind === 'needs_auth' ? state.message : undefined;
}

/** The status with one server's `enabled` set (an optimistic change, before the next read). */
export function withEnabled(status: McpStatus, name: string, enabled: boolean): McpStatus {
  return { ...status, servers: status.servers.map((s) => (s.name === name ? { ...s, enabled } : s)) };
}

export function toggleTip(enabled: boolean): string {
  return enabled ? 'The agent can use this server' : 'Hidden from the agent';
}

export const STARTING_POLL_MS = 1000;
export const OPEN_POLL_MS = 2000;
export const RETRY_POLL_MS = 2000;

/**
 * When to read the status again, in ms, or null to stop. Nothing to read on a fresh mount means a retry (a failed read keeps
 * the last status). Starting polls fast. With servers and the panel open, it polls at the open rate. Otherwise it stops: no
 * servers means one read only.
 */
export function pollDelay(status: McpStatus | null, open: boolean): number | null {
  if (status === null) {
    return RETRY_POLL_MS;
  }
  if (status.starting) {
    return STARTING_POLL_MS;
  }
  if (status.servers.length === 0) {
    return null;
  }
  return open ? OPEN_POLL_MS : null;
}

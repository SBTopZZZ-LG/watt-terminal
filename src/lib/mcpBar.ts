import type { McpServerStatus, McpStatus } from './types';

/** The status dot's colour: off (nothing enabled, or a disabled server), connecting, connected (ok), or failed. */
export type McpTone = 'off' | 'connecting' | 'ok' | 'fail';

/** The names of the servers that have settled this session (connected, failed or needs sign-in while enabled). */
export type Settled = ReadonlySet<string>;

export const NO_SETTLED: Settled = new Set();

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

/**
 * Adds each enabled server that has settled to the set. Returns the same set when nothing is new. A settled server that
 * connects again (the backend retries failed servers) is already in the set, so it does not bring the loader back.
 */
export function settleServers(settled: Settled, servers: McpServerStatus[]): Settled {
  const fresh = servers
    .filter((s) => s.enabled && (s.state.kind === 'connected' || s.state.kind === 'failed' || s.state.kind === 'needs_auth'))
    .map((s) => s.name)
    .filter((name) => !settled.has(name));
  return fresh.length === 0 ? settled : new Set([...settled, ...fresh]);
}

/** Removes one server from the settled set, so its next connect shows the loader once (the user switched it on). */
export function unsettleServer(settled: Settled, name: string): Settled {
  if (!settled.has(name)) {
    return settled;
  }
  const next = new Set(settled);
  next.delete(name);
  return next;
}

/** True while an enabled server that has not settled this session is connecting. Retries of settled servers do not count. */
export function showsLoader(servers: McpServerStatus[], settled: Settled = NO_SETTLED): boolean {
  return servers.some((s) => s.enabled && s.state.kind === 'connecting' && !settled.has(s.name));
}

/** The enabled servers that are not connecting (connected, failed, or settled this session), and how many are enabled. */
export function readyCount(servers: McpServerStatus[], settled: Settled = NO_SETTLED): { ready: number; total: number } {
  const enabled = servers.filter((s) => s.enabled);
  return { ready: enabled.filter((s) => s.state.kind !== 'connecting' || settled.has(s.name)).length, total: enabled.length };
}

/** The loader row's text. The count is shown only when more than one server is enabled. */
export function loaderText(servers: McpServerStatus[], settled: Settled = NO_SETTLED): string {
  const { ready, total } = readyCount(servers, settled);
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
export const IDLE_POLL_MS = 5000;
export const RETRY_POLL_MS = 2000;

/**
 * When to read the status again, in ms. Nothing to read on a fresh mount means a retry (a failed read keeps the last status).
 * Starting polls fast. With servers and the panel open, it polls at the open rate. Otherwise it polls at the idle rate, so
 * changes made elsewhere (Settings, a failed server's retry) reach the dot, and the first server added in Settings shows up.
 */
export function pollDelay(status: McpStatus | null, open: boolean): number | null {
  if (status === null) {
    return RETRY_POLL_MS;
  }
  if (status.starting) {
    return STARTING_POLL_MS;
  }
  return open && status.servers.length > 0 ? OPEN_POLL_MS : IDLE_POLL_MS;
}

import { describe, expect, it } from 'vitest';
import {
  barTone,
  IDLE_POLL_MS,
  loaderText,
  NO_SETTLED,
  OPEN_POLL_MS,
  pollDelay,
  readyCount,
  RETRY_POLL_MS,
  serverTone,
  settleServers,
  showsLoader,
  STARTING_POLL_MS,
  stateLabel,
  stateMessage,
  toggleTip,
  unsettleServer,
  withEnabled,
} from './mcpBar';
import type { McpServerStatus, McpStateView, McpStatus } from './types';

const server = (name: string, state: McpStateView, enabled = true, tools = 0): McpServerStatus => ({ id: name, name, enabled, state, tools });
const status = (servers: McpServerStatus[], starting = servers.some((s) => s.enabled && s.state.kind === 'connecting')): McpStatus => ({ servers, starting });

describe('serverTone', () => {
  it('maps each state to its dot, and a disabled server to off', () => {
    expect(serverTone(server('a', { kind: 'connected', tools: 2 }))).toBe('ok');
    expect(serverTone(server('a', { kind: 'connecting' }))).toBe('connecting');
    expect(serverTone(server('a', { kind: 'failed', message: 'x' }))).toBe('fail');
    expect(serverTone(server('a', { kind: 'needs_auth', message: 'x' }))).toBe('fail');
    expect(serverTone(server('a', { kind: 'connected', tools: 1 }, false))).toBe('off');
    expect(serverTone(server('a', { kind: 'disabled' }))).toBe('off');
  });
});

describe('barTone', () => {
  it('is off when nothing is enabled', () => {
    expect(barTone([])).toBe('off');
    expect(barTone([server('a', { kind: 'disabled' }, false)])).toBe('off');
  });

  it('is green when every enabled server is connected', () => {
    expect(barTone([server('a', { kind: 'connected', tools: 1 }), server('b', { kind: 'connected', tools: 3 })])).toBe('ok');
    expect(barTone([server('a', { kind: 'connected', tools: 1 }), server('b', { kind: 'disabled' }, false)])).toBe('ok');
  });

  it('is amber while one connects, and red when one failed, even with another connecting', () => {
    expect(barTone([server('a', { kind: 'connected', tools: 1 }), server('b', { kind: 'connecting' })])).toBe('connecting');
    expect(barTone([server('a', { kind: 'connecting' }), server('b', { kind: 'failed', message: 'x' })])).toBe('fail');
    expect(barTone([server('a', { kind: 'needs_auth', message: 'x' })])).toBe('fail');
  });
});

describe('readyCount and loaderText', () => {
  const three = [
    server('a', { kind: 'connected', tools: 1 }),
    server('b', { kind: 'failed', message: 'x' }),
    server('c', { kind: 'connecting' }),
    server('d', { kind: 'connecting' }, false),
  ];

  it('counts the enabled servers that are not connecting', () => {
    expect(readyCount(three)).toEqual({ ready: 2, total: 3 });
    expect(readyCount([])).toEqual({ ready: 0, total: 0 });
  });

  it('shows the count only with more than one enabled server', () => {
    expect(loaderText(three)).toBe('Starting MCP servers… (2/3 ready)');
    expect(loaderText([server('a', { kind: 'connecting' })])).toBe('Starting MCP servers…');
  });

  it('initial startup: a server that settled counts as ready, a connecting one does not', () => {
    const startup = [server('a', { kind: 'connected', tools: 1 }), server('b', { kind: 'connecting' })];
    const settled = settleServers(NO_SETTLED, startup);
    expect(showsLoader(startup, settled)).toBe(true);
    expect(loaderText(startup, settled)).toBe('Starting MCP servers… (1/2 ready)');
  });
});

describe('settleServers, unsettleServer and showsLoader', () => {
  const connecting = server('docs', { kind: 'connecting' });
  const failed = server('docs', { kind: 'failed', message: 'spawn ENOENT' });

  it('records enabled servers that connected, failed or need sign-in, and returns the same set when nothing is new', () => {
    const once = settleServers(NO_SETTLED, [failed, server('fetch', { kind: 'connected', tools: 1 }), server('x', { kind: 'needs_auth', message: 'm' })]);
    expect([...once].sort()).toEqual(['docs', 'fetch', 'x']);
    expect(settleServers(once, [failed])).toBe(once);
  });

  it('does not record connecting, disabled or off servers', () => {
    const next = settleServers(NO_SETTLED, [connecting, server('ctx', { kind: 'disabled' }, false), server('y', { kind: 'failed', message: 'x' }, false)]);
    expect(next.size).toBe(0);
  });

  it('initial startup shows the loader while an unsettled server connects', () => {
    expect(showsLoader([connecting])).toBe(true);
    expect(showsLoader([connecting], NO_SETTLED)).toBe(true);
  });

  it('a settled server that retries (failed, then connecting again) does not bring the loader back', () => {
    const settled = settleServers(NO_SETTLED, [failed]);
    expect(showsLoader([connecting], settled)).toBe(false);
  });

  it('a user switch-on removes the server, so its connect shows the loader once', () => {
    const settled = settleServers(NO_SETTLED, [failed]);
    const reenabled = unsettleServer(settled, 'docs');
    expect(reenabled).not.toBe(settled);
    expect(showsLoader([connecting], reenabled)).toBe(true);
    // Once it settles again, a later retry is quiet.
    expect(showsLoader([failed], settleServers(reenabled, [failed]))).toBe(false);
  });

  it('unsettling a server that is not in the set keeps the same set', () => {
    expect(unsettleServer(NO_SETTLED, 'docs')).toBe(NO_SETTLED);
  });

  it('ignores a connecting server that is disabled', () => {
    expect(showsLoader([server('docs', { kind: 'connecting' }, false)])).toBe(false);
  });
});

describe('stateLabel and stateMessage', () => {
  it('says each state in a few words, and singular for one tool', () => {
    expect(stateLabel(server('a', { kind: 'connected', tools: 1 }))).toBe('1 tool');
    expect(stateLabel(server('a', { kind: 'connected', tools: 0 }))).toBe('0 tools');
    expect(stateLabel(server('a', { kind: 'connected', tools: 12 }))).toBe('12 tools');
    expect(stateLabel(server('a', { kind: 'connecting' }))).toBe('connecting…');
    expect(stateLabel(server('a', { kind: 'failed', message: 'boom' }))).toBe('failed');
    expect(stateLabel(server('a', { kind: 'needs_auth', message: 'sign in' }))).toBe('needs sign-in');
    expect(stateLabel(server('a', { kind: 'disabled' }, false))).toBe('off');
  });

  it('gives the failure message only for failed and needs-sign-in states', () => {
    expect(stateMessage(server('a', { kind: 'failed', message: 'boom' }))).toBe('boom');
    expect(stateMessage(server('a', { kind: 'needs_auth', message: 'sign in' }))).toBe('sign in');
    expect(stateMessage(server('a', { kind: 'connected', tools: 1 }))).toBeUndefined();
  });
});

describe('toggleTip and withEnabled', () => {
  it('says what the switch does in each position', () => {
    expect(toggleTip(true)).toBe('The agent can use this server');
    expect(toggleTip(false)).toBe('Hidden from the agent');
  });

  it('sets one server enabled flag and leaves the others alone', () => {
    const before = status([server('a', { kind: 'connected', tools: 1 }), server('b', { kind: 'disabled' }, false)]);
    const after = withEnabled(before, 'b', true);
    expect(after.servers.map((s) => s.enabled)).toEqual([true, true]);
    expect(after.servers[0]).toBe(before.servers[0]);
    expect(withEnabled(before, 'a', false).servers[0].enabled).toBe(false);
    expect(before.servers[1].enabled).toBe(false);
  });
});

describe('pollDelay', () => {
  it('retries a read that has not succeeded yet', () => {
    expect(pollDelay(null, false)).toBe(RETRY_POLL_MS);
  });

  it('polls fast while starting, whether or not the panel is open', () => {
    const starting = status([server('a', { kind: 'connecting' })]);
    expect(pollDelay(starting, false)).toBe(STARTING_POLL_MS);
    expect(pollDelay(starting, true)).toBe(STARTING_POLL_MS);
  });

  it('polls slowly once nothing is starting and the panel is closed, so outside changes reach the dot', () => {
    const settled = status([server('a', { kind: 'connected', tools: 1 })]);
    expect(pollDelay(settled, false)).toBe(IDLE_POLL_MS);
    expect(IDLE_POLL_MS).toBe(5000);
  });

  it('polls at the open rate while the panel is open with servers', () => {
    expect(pollDelay(status([server('a', { kind: 'connected', tools: 1 })]), true)).toBe(OPEN_POLL_MS);
  });

  it('re-checks slowly when no servers are configured, so a first server added in Settings shows up', () => {
    expect(pollDelay(status([]), true)).toBe(IDLE_POLL_MS);
    expect(pollDelay(status([]), false)).toBe(IDLE_POLL_MS);
  });
});

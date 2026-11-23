import { describe, expect, it } from 'vitest';
import { barTone, loaderText, OPEN_POLL_MS, pollDelay, readyCount, RETRY_POLL_MS, serverTone, STARTING_POLL_MS, stateLabel, stateMessage, toggleTip, withEnabled } from './mcpBar';
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

  it('stops once nothing is starting and the panel is closed', () => {
    const settled = status([server('a', { kind: 'connected', tools: 1 })]);
    expect(pollDelay(settled, false)).toBeNull();
  });

  it('polls at the open rate while the panel is open with servers', () => {
    expect(pollDelay(status([server('a', { kind: 'connected', tools: 1 })]), true)).toBe(OPEN_POLL_MS);
  });

  it('does not poll when no servers are configured', () => {
    expect(pollDelay(status([]), true)).toBeNull();
    expect(pollDelay(status([]), false)).toBeNull();
  });
});

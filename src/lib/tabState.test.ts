import { describe, expect, it } from 'vitest';
import { NO_CONTROL } from './agentControl';
import { parseShellPayload } from './osc';
import { acceptedNonces, INITIAL_TAB_STATE, isSessionEvent, pickTabTitle, reduceStatus, reduceTabEvent } from './tabState';
import type { Status, WireEvent } from './types';

const ORIGIN = { ...INITIAL_TAB_STATE };

function status(over: Partial<Status> = {}): Status {
  return {
    state: 'prompt',
    levels: [],
    depth: 1,
    nonces: ['n1'],
    integration: 'live',
    observer: 'ok',
    cwd: { host: 'h', path: '/work' },
    stats: { forged_events: 0, overflow_events: 0, dropped_bytes: 0 },
    auto_approve: false,
    auto_approve_suspended: false,
    agent_busy: false,
    agent_control: false,
    llm: { enabled: false, model: null },
    conversation_id: 'conv',
    settings_state: null,
    ...over,
  };
}

describe('reduceTabEvent', () => {
  it('state moves the input model and keeps the other fields', () => {
    const next = reduceTabEvent(ORIGIN, { type: 'state', state: 'running', foreground: { kind: 'other', command: 'vim' } });
    expect(next.input.rust).toBe('running');
    expect(next.input.foreground).toEqual({ kind: 'other', command: 'vim' });
    expect(next.cwd).toBeNull();
    expect(next.integration).toBe('booting');
  });

  it('a state without a hint clears the foreground', () => {
    const running = reduceTabEvent(ORIGIN, { type: 'state', state: 'running', foreground: { kind: 'ssh', command: 'ssh host' } });
    expect(reduceTabEvent(running, { type: 'state', state: 'prompt' }).input.foreground).toBeNull();
  });

  it('integration, observer and cwd set their own field', () => {
    let s = reduceTabEvent(ORIGIN, { type: 'integration', status: 'raw' });
    s = reduceTabEvent(s, { type: 'observer', status: 'desynced' });
    s = reduceTabEvent(s, { type: 'cwd', cwd: { host: 'h', path: '/tmp' } });
    expect(s.integration).toBe('raw');
    expect(s.observer).toBe('desynced');
    expect(s.cwd).toEqual({ host: 'h', path: '/tmp' });
  });

  it('levels sets the nested levels', () => {
    const s = reduceTabEvent(ORIGIN, { type: 'levels', levels: [], nonces: ['a'] });
    expect(s.levels).toEqual([]);
  });

  it('an agent-started command holds the shell until it finishes; exit clears the hold', () => {
    const started = reduceTabEvent(ORIGIN, { type: 'record_started', id: 7, command: 'ls', source: { kind: 'agent', call_id: 'c1' } });
    expect(started.agentControl.commands.has(7)).toBe(true);
    const exited = reduceTabEvent(started, { type: 'state', state: 'exited' });
    expect(exited.agentControl).toBe(NO_CONTROL);
  });

  it('a user-started record does not take the hold', () => {
    const s = reduceTabEvent(ORIGIN, { type: 'record_started', id: 8, command: 'ls', source: { kind: 'user' } });
    expect(s.agentControl).toBe(ORIGIN.agentControl);
  });

  it('returns the same object for an event that changes nothing', () => {
    const record = { id: 99, command: 'ls', source: { kind: 'user' as const }, exit_code: 0, status: 'complete' as const, used_alt_screen: false, output_bytes: 0 };
    expect(reduceTabEvent(ORIGIN, { type: 'record_finished', record })).toBe(ORIGIN);
  });
});

describe('reduceStatus', () => {
  it('takes the tab part of a status read; levels are as the status gives them', () => {
    const s = reduceStatus(ORIGIN, status({ state: 'running', integration: 'raw', observer: 'desynced', cwd: null }));
    expect(s.input.rust).toBe('running');
    expect(s.integration).toBe('raw');
    expect(s.observer).toBe('desynced');
    expect(s.cwd).toBeNull();
    expect(s.levels).toEqual([]);
  });

  it('a busy agent with the shell not held keeps the control flag from the snapshot', () => {
    const s = reduceStatus(ORIGIN, status({ agent_busy: true, agent_control: true }));
    expect(s.agentControl.call).toBe(true);
  });
});

describe('isSessionEvent', () => {
  it('is true for the terminal events and false for agent events', () => {
    const session: WireEvent[] = [
      { type: 'state', state: 'prompt' },
      { type: 'levels', levels: [], nonces: [] },
    ];
    for (const ev of session) {
      expect(isSessionEvent(ev)).toBe(true);
    }
    expect(isSessionEvent({ type: 'done' } as WireEvent)).toBe(false);
    expect(isSessionEvent({ type: 'agent_control', active: true } as WireEvent)).toBe(false);
  });
});

describe('pickTabTitle', () => {
  const base = { foreground: null, cwd: null, index: 3 };

  it('uses the xterm title when it is not blank', () => {
    expect(pickTabTitle({ ...base, xtermTitle: '  vim notes  ', foreground: { kind: 'other', command: 'vim' } })).toBe('vim notes');
  });

  it('falls back to the foreground command when the xterm title is empty or blank', () => {
    expect(pickTabTitle({ ...base, xtermTitle: '', foreground: { kind: 'other', command: 'npm test' }, cwd: { host: 'h', path: '/x' } })).toBe('npm test');
    expect(pickTabTitle({ ...base, xtermTitle: '   ', foreground: { kind: 'other', command: ' ' }, cwd: null })).toBe('Terminal 3');
  });

  it('falls back to the cwd basename (posix and windows separators, trailing separators ignored)', () => {
    expect(pickTabTitle({ ...base, xtermTitle: null, cwd: { host: 'h', path: '/home/me/project/' } })).toBe('project');
    expect(pickTabTitle({ ...base, xtermTitle: null, cwd: { host: 'h', path: 'C:\\Users\\me\\repo' } })).toBe('repo');
  });

  it('falls back to Terminal N when nothing is known', () => {
    expect(pickTabTitle({ ...base, xtermTitle: null })).toBe('Terminal 3');
    expect(pickTabTitle({ ...base, xtermTitle: null, cwd: { host: 'h', path: '/' } })).toBe('Terminal 3');
  });
});

describe('acceptedNonces (per tab)', () => {
  const A = '0123456789abcdef0123456789abcdef';
  const B = 'fedcba9876543210fedcba9876543210';

  it('drops blanks', () => {
    expect([...acceptedNonces(['', A, ''])]).toEqual([A]);
  });

  it('a nonce accepted on one tab is refused on another tab', () => {
    const tabA = acceptedNonces([A]);
    const tabB = acceptedNonces([B]);
    expect(parseShellPayload(133, `A;n=${A}`, tabA)).toEqual({ kind: 'prompt_start' });
    expect(parseShellPayload(133, `A;n=${A}`, tabB)).toBeNull();
  });
});

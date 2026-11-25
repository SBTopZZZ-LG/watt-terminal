import { describe, expect, it } from 'vitest';
import { agentActivity, agentActivityText, MAX_ACTIVITY_CHARS } from './agentActivity';
import type { TranscriptItem } from './transcript';

const agent = (id: number, text: string): TranscriptItem => ({ id, kind: 'agent', text, streaming: false });
const tool = (id: number, name: string, args: unknown): TranscriptItem => ({
  id,
  kind: 'tool',
  callId: `c${id}`,
  name,
  args,
  status: 'running',
  result: null,
  decision: null,
  reason: null,
});

describe('agentActivityText', () => {
  it('prefers the newest non-empty agent text', () => {
    const items = [agent(1, 'first'), agent(2, 'second'), agent(3, '   ')];
    expect(agentActivityText(items, false, false)).toBe('second');
  });

  it('prefers an older agent message over a newer tool caption', () => {
    const items = [agent(1, 'working on it'), tool(2, 'run_command', { summary: 'List files', command: 'ls' })];
    expect(agentActivityText(items, false, false)).toBe('working on it');
  });

  it('collapses whitespace in the agent text', () => {
    const items = [agent(1, '  line one\n\tline   two  ')];
    expect(agentActivityText(items, false, false)).toBe('line one line two');
  });

  it('caps the agent text at ~160 characters and marks the cut', () => {
    const items = [agent(1, 'x'.repeat(200))];
    expect(agentActivityText(items, false, false)).toBe(`${'x'.repeat(MAX_ACTIVITY_CHARS - 1)}…`);
    expect(agentActivityText(items, false, false).length).toBe(MAX_ACTIVITY_CHARS);
  });

  it('falls back to the newest run_command/send_keys caption as <name> · <caption>', () => {
    const items = [
      tool(1, 'run_command', { summary: 'List files', command: 'ls' }),
      tool(2, 'get_cwd', {}),
      tool(3, 'send_keys', { summary: 'Answer the prompt', text: 'yes', keys: ['Enter'] }),
    ];
    expect(agentActivityText(items, false, false)).toBe('send_keys · Answer the prompt');
  });

  it('skips tool calls with no usable caption', () => {
    const items = [tool(1, 'run_command', { command: 'ls' })];
    expect(agentActivityText(items, false, false)).toBe('Working…');
  });

  it('falls back to Working… for a side command, Thinking… while thinking, else Working…', () => {
    expect(agentActivityText([], false, true)).toBe('Working…');
    expect(agentActivityText([], true, false)).toBe('Thinking…');
    expect(agentActivityText([], false, false)).toBe('Working…');
    expect(agentActivityText([], true, true)).toBe('Working…');
  });
});

describe('agentActivity: the current turn only', () => {
  const user = (id: number, text: string): TranscriptItem => ({ id, kind: 'user', clientId: id, text });

  it('does not show the previous turn reply while the new turn has none yet', () => {
    const items = [user(1, 'first?'), agent(2, 'old reply'), user(3, 'second?')];
    expect(agentActivityText(items, false, false)).toBe('Working…');
    expect(agentActivityText(items, true, false)).toBe('Thinking…');
  });

  it('shows the current turn reply, not an older one', () => {
    const items = [user(1, 'first?'), agent(2, 'old reply'), user(3, 'second?'), agent(4, 'new reply')];
    expect(agentActivityText(items, false, false)).toBe('new reply');
  });

  it('does not fall back to an earlier turn tool caption', () => {
    const items = [user(1, 'first?'), tool(2, 'run_command', { summary: 'Old run', command: 'ls' }), user(3, 'second?')];
    expect(agentActivityText(items, false, false)).toBe('Working…');
  });
});

describe('agentActivity', () => {
  it('gives the whole agent text as full, and the capped one as text', () => {
    const long = 'x'.repeat(200);
    expect(agentActivity([agent(1, long)], false, false)).toEqual({ text: `${'x'.repeat(MAX_ACTIVITY_CHARS - 1)}…`, full: long });
  });

  it('gives the same text for both when nothing was cut', () => {
    expect(agentActivity([agent(1, 'short  reply')], false, false)).toEqual({ text: 'short reply', full: 'short reply' });
    expect(agentActivity([], true, false)).toEqual({ text: 'Thinking…', full: 'Thinking…' });
  });

  it('gives the uncut caption as full for a tool line', () => {
    const summary = 'one two three four five six seven eight nine ten eleven';
    expect(agentActivity([tool(1, 'run_command', { summary, command: 'ls' })], false, false)).toEqual({
      text: 'run_command · one two three four five six seven eight nine ten…',
      full: `run_command · ${summary}`,
    });
  });
});

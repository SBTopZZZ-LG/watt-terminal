import { describe, expect, it } from 'vitest';
import { agentActivityText, MAX_ACTIVITY_CHARS } from './agentActivity';
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

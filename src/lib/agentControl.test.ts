import { describe, expect, it } from 'vitest';
import { agentControlAfter } from './agentControl';
import type { WireEvent } from './types';

const approvalRequest: WireEvent = { type: 'approval_request', call_id: 'call-1', text: 'pwd' };
const toolCall: WireEvent = { type: 'tool_call', call_id: 'call-1', name: 'run_command', args: { command: 'pwd' } };
const toolResult: WireEvent = { type: 'tool_result', call_id: 'call-1', name: 'run_command', ok: true, result: {} };
const started: WireEvent = {
  type: 'record_started',
  id: 1,
  command: 'pwd',
  source: { kind: 'agent', call_id: 'run-1' },
};

/** Feeds the events through the derive function in order and returns the flag after each one. */
function trace(events: WireEvent[], start = false): boolean[] {
  const out: boolean[] = [];
  let active = start;
  for (const ev of events) {
    active = agentControlAfter(active, ev);
    out.push(active);
  }
  return out;
}

describe('agentControlAfter', () => {
  it('is set by agent_control and cleared by its false', () => {
    expect(agentControlAfter(false, { type: 'agent_control', active: true })).toBe(true);
    expect(agentControlAfter(true, { type: 'agent_control', active: false })).toBe(false);
  });

  it('waiting for an approval is not control: the request and the call do not set it', () => {
    expect(trace([toolCall, approvalRequest])).toEqual([false, false]);
  });

  it('brackets an approved command from its start to its result', () => {
    const events: WireEvent[] = [
      toolCall,
      approvalRequest,
      { type: 'agent_control', active: true },
      started,
      { type: 'agent_control', active: false },
      toolResult,
    ];
    expect(trace(events)).toEqual([false, false, true, true, false, false]);
  });

  it('clears when the agent turn ends, even if the control off event was lost', () => {
    expect(trace([{ type: 'agent_control', active: true }, { type: 'done' }])).toEqual([true, false]);
  });

  it('clears when the shell exits mid-command, and only then', () => {
    expect(trace([{ type: 'agent_control', active: true }, { type: 'state', state: 'exited' }])).toEqual([true, false]);
    expect(trace([{ type: 'agent_control', active: true }, { type: 'state', state: 'running' }])).toEqual([true, true]);
  });

  it('keeps the flag through unrelated events', () => {
    expect(trace([{ type: 'text', text: 'hi' }, toolResult], true)).toEqual([true, true]);
  });
});

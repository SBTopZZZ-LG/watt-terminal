import { describe, expect, it } from 'vitest';
import { agentControlAfter, borderActive, borderLevel, NO_CONTROL } from './agentControl';
import type { AgentControl } from './agentControl';
import { EMPTY_TRANSCRIPT, isBusy, transcriptReducer } from './transcript';
import type { TranscriptAction } from './transcript';
import type { Source, WireEvent } from './types';

const approvalRequest: WireEvent = { type: 'approval_request', call_id: 'call-1', name: 'run_command', text: 'pwd' };
const toolCall: WireEvent = { type: 'tool_call', call_id: 'call-1', name: 'run_command', args: { command: 'pwd' } };
const toolResult: WireEvent = {
  type: 'tool_result',
  call_id: 'call-1',
  name: 'run_command',
  ok: true,
  decision: 'manual',
  result: {},
};
const controlOn: WireEvent = { type: 'agent_control', active: true };
const controlOff: WireEvent = { type: 'agent_control', active: false };
const done: WireEvent = { type: 'done' };

const agentSource: Source = { kind: 'agent', call_id: 'run-1' };
const userSource: Source = { kind: 'user' };

function started(source: Source): WireEvent {
  return { type: 'record_started', id: 1, command: 'pwd', source };
}

function finished(source: Source): WireEvent {
  return {
    type: 'record_finished',
    record: { id: 1, command: 'pwd', source, exit_code: 0, status: 'complete', used_alt_screen: false, output_bytes: 0 },
  };
}

/** Feeds the events through the derive function in order and returns the border after each one. */
function trace(events: WireEvent[], start: AgentControl = NO_CONTROL): boolean[] {
  const out: boolean[] = [];
  let c = start;
  for (const ev of events) {
    c = agentControlAfter(c, ev);
    out.push(borderActive(c));
  }
  return out;
}

describe('agentControlAfter', () => {
  it('is set by agent_control and cleared by its false', () => {
    expect(agentControlAfter(NO_CONTROL, controlOn)).toEqual({ call: true, command: false });
    expect(agentControlAfter({ call: true, command: false }, controlOff)).toEqual(NO_CONTROL);
  });

  it('waiting for an approval is not control: the request and the call do not set it', () => {
    expect(trace([toolCall, approvalRequest])).toEqual([false, false]);
  });

  it('brackets an approved command from its start to its result', () => {
    const events: WireEvent[] = [
      toolCall,
      approvalRequest,
      controlOn,
      started(agentSource),
      finished(agentSource),
      controlOff,
      toolResult,
    ];
    expect(trace(events)).toEqual([false, false, true, true, true, false, false]);
  });

  it('stays on after the call returns while the agent command is still running (timed out)', () => {
    // run_command returns TimedOut: control off and the turn ends, but the command has not finished.
    expect(trace([controlOn, started(agentSource), controlOff, done])).toEqual([true, true, true, true]);
  });

  it('clears when the timed-out agent command finishes, and not before', () => {
    expect(trace([controlOn, started(agentSource), controlOff, done, finished(agentSource)])).toEqual([
      true,
      true,
      true,
      true,
      false,
    ]);
  });

  it('an agent command holds the border only while it runs', () => {
    expect(trace([started(agentSource), finished(agentSource)])).toEqual([true, false]);
  });

  it('a user command does not hold the border', () => {
    expect(trace([started(userSource), finished(userSource)])).toEqual([false, false]);
  });

  it('clears when the agent turn ends, even if the control off event was lost', () => {
    expect(trace([controlOn, done])).toEqual([true, false]);
  });

  it('clears both holds when the shell exits mid-command, and only then', () => {
    expect(trace([controlOn, started(agentSource), { type: 'state', state: 'exited' }])).toEqual([true, true, false]);
    expect(trace([controlOn, started(agentSource), { type: 'state', state: 'running' }])).toEqual([true, true, true]);
  });

  it('keeps the flag through unrelated events', () => {
    expect(trace([{ type: 'text', text: 'hi' }, toolResult], { call: true, command: false })).toEqual([true, true]);
  });
});

describe('borderLevel', () => {
  it('is off when nothing holds the shell and no turn is in flight', () => {
    expect(borderLevel(NO_CONTROL, false)).toBe('off');
  });

  it('is working during a turn that has not reached the shell yet', () => {
    expect(borderLevel(NO_CONTROL, true)).toBe('working');
  });

  it('is control while a call or an agent command holds the shell, busy or not', () => {
    const holds: AgentControl[] = [
      { call: true, command: false },
      { call: false, command: true },
      { call: true, command: true },
    ];
    for (const c of holds) {
      expect(borderLevel(c, true)).toBe('control');
      expect(borderLevel(c, false)).toBe('control');
    }
  });

  it('follows the transcript turn: working from the send until done, then off', () => {
    const actions: TranscriptAction[] = [
      { type: 'user_sent', clientId: 1, text: 'what is running' },
      { type: 'agent_event', event: { type: 'thinking', active: true } },
      { type: 'agent_event', event: { type: 'text_delta', text: 'Checking' } },
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a1', name: 'run_command', text: 'ps' } },
    ];
    let t = EMPTY_TRANSCRIPT;
    const level = () => borderLevel(NO_CONTROL, isBusy(t));
    const seen: string[] = [];
    for (const action of actions) {
      t = transcriptReducer(t, action);
      seen.push(level());
    }
    t = transcriptReducer(t, { type: 'agent_event', event: { type: 'done' } });
    seen.push(level());
    expect(seen).toEqual(['working', 'working', 'working', 'working', 'off']);
  });

  it('is off after a session reset and after a conversation reset, even mid-turn', () => {
    const sent = transcriptReducer(EMPTY_TRANSCRIPT, { type: 'user_sent', clientId: 1, text: 'hi' });
    expect(borderLevel(NO_CONTROL, isBusy(transcriptReducer(sent, { type: 'session_reset' })))).toBe('off');
    expect(borderLevel(NO_CONTROL, isBusy(transcriptReducer(sent, { type: 'conversation_reset' })))).toBe('off');
  });

  it('a startup status with a turn running shows working', () => {
    expect(borderLevel(NO_CONTROL, isBusy(transcriptReducer(EMPTY_TRANSCRIPT, { type: 'sync_busy', busy: true })))).toBe('working');
  });
});

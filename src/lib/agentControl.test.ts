import { describe, expect, it } from 'vitest';
import { agentControlAfter, borderActive, borderLevel, NO_CONTROL, settleControl } from './agentControl';
import type { AgentControl, ControlSnapshot } from './agentControl';
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

function startedId(id: number, source: Source): WireEvent {
  return { type: 'record_started', id, command: 'cmd', source };
}

function finishedId(id: number, source: Source): WireEvent {
  return {
    type: 'record_finished',
    record: { id, command: 'cmd', source, exit_code: 0, status: 'complete', used_alt_screen: false, output_bytes: 0 },
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
    expect(agentControlAfter(NO_CONTROL, controlOn)).toEqual({ call: true, commands: new Set() });
    expect(agentControlAfter({ call: true, commands: new Set() }, controlOff)).toEqual(NO_CONTROL);
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

  it('a nested agent command finishing does not clear the border while the parked ssh command is open', () => {
    // The ssh command (7) stays open after request_integration; the agent runs a command inside it (9) that ends first.
    const events: WireEvent[] = [
      startedId(7, agentSource),
      startedId(9, agentSource),
      finishedId(9, agentSource),
      finishedId(7, agentSource),
    ];
    expect(trace(events)).toEqual([true, true, true, false]);
  });

  it('the set holds each open id once, and only a finish of an id in it removes it', () => {
    let c = agentControlAfter(NO_CONTROL, startedId(7, agentSource));
    c = agentControlAfter(c, startedId(7, agentSource));
    expect(c.commands).toEqual(new Set([7]));
    c = agentControlAfter(c, finishedId(8, agentSource));
    expect(c.commands).toEqual(new Set([7]));
    c = agentControlAfter(c, finishedId(7, agentSource));
    expect(c.commands.size).toBe(0);
  });

  it('a shell exit clears every open command', () => {
    expect(trace([startedId(7, agentSource), startedId(9, agentSource), { type: 'state', state: 'exited' }])).toEqual([true, true, false]);
  });

  it('clears when the agent turn ends, even if the control off event was lost', () => {
    expect(trace([controlOn, done])).toEqual([true, false]);
  });

  it('clears both holds when the shell exits mid-command, and only then', () => {
    expect(trace([controlOn, started(agentSource), { type: 'state', state: 'exited' }])).toEqual([true, true, false]);
    expect(trace([controlOn, started(agentSource), { type: 'state', state: 'running' }])).toEqual([true, true, true]);
  });

  it('keeps the flag through unrelated events', () => {
    expect(trace([{ type: 'text', text: 'hi' }, toolResult], { call: true, commands: new Set() })).toEqual([true, true]);
  });
});

describe('settleControl', () => {
  const idlePrompt: ControlSnapshot = { state: 'prompt', levels: [], agent_busy: false, agent_control: false };

  it('clears stale commands at a local idle prompt', () => {
    const c: AgentControl = { call: false, commands: new Set([7]) };
    expect(settleControl(c, idlePrompt)).toEqual(NO_CONTROL);
  });

  it('keeps commands while a command runs, in raw/alt states, with a nested level, or while the agent is busy', () => {
    const c: AgentControl = { call: false, commands: new Set([7]) };
    const snapshots: ControlSnapshot[] = [
      // A command still running.
      { state: 'running', levels: [], agent_busy: false, agent_control: false },
      // raw/alt states keep the hold: the shell is not back at a local prompt.
      { state: 'raw', levels: [], agent_busy: false, agent_control: false },
      { state: 'alt', levels: [], agent_busy: false, agent_control: false },
      // A nested integrated shell keeps the hold: a parked command may still be open inside it.
      { state: 'prompt', levels: [{ key: 1 }], agent_busy: false, agent_control: false },
      // An approval/question wait keeps the hold: the turn may still resume.
      { state: 'prompt', levels: [], agent_busy: true, agent_control: false },
    ];
    for (const snapshot of snapshots) {
      const next = settleControl(c, snapshot);
      expect(next.commands).toEqual(new Set([7]));
    }
  });

  it('sets call from the snapshot', () => {
    expect(settleControl(NO_CONTROL, { ...idlePrompt, agent_control: true })).toEqual({
      call: true,
      commands: new Set(),
    });
    expect(settleControl({ call: true, commands: new Set([7]) }, idlePrompt).call).toBe(false);
  });

  it('clears everything when the shell exited', () => {
    const c: AgentControl = { call: true, commands: new Set([7]) };
    expect(settleControl(c, { state: 'exited', levels: [], agent_busy: false, agent_control: false })).toBe(NO_CONTROL);
  });

  it('returns the input when nothing changes', () => {
    const idle: AgentControl = { call: false, commands: new Set() };
    expect(settleControl(idle, idlePrompt)).toBe(idle);
    const held: AgentControl = { call: false, commands: new Set([7]) };
    const running: ControlSnapshot = { state: 'running', levels: [], agent_busy: false, agent_control: false };
    expect(settleControl(held, running)).toBe(held);
    expect(settleControl(NO_CONTROL, { state: 'exited', levels: [], agent_busy: false, agent_control: false })).toBe(NO_CONTROL);
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
      { call: true, commands: new Set() },
      { call: false, commands: new Set([1]) },
      { call: true, commands: new Set([1, 2]) },
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

import { describe, expect, it } from 'vitest';
import { EMPTY_TRANSCRIPT, firstPendingApproval, isBusy, MAX_ITEMS, transcriptReducer } from './transcript';
import type { Transcript, TranscriptAction } from './transcript';

function run(actions: TranscriptAction[], start: Transcript = EMPTY_TRANSCRIPT): Transcript {
  return actions.reduce(transcriptReducer, start);
}

describe('transcriptReducer: agent events', () => {
  it('adds agent text, and a tool call starts as running', () => {
    const t = run([
      { type: 'agent_event', event: { type: 'text', text: 'Looking.' } },
      { type: 'agent_event', event: { type: 'tool_call', call_id: 'c1', name: 'get_screen', args: {} } },
    ]);
    expect(t.items.map((i) => i.kind)).toEqual(['agent', 'tool']);
    expect(t.items[1]).toMatchObject({ kind: 'tool', callId: 'c1', name: 'get_screen', status: 'running' });
  });

  it('a tool result sets the call status and keeps the result', () => {
    const ok = run([
      { type: 'agent_event', event: { type: 'tool_call', call_id: 'c1', name: 'get_cwd', args: {} } },
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'c1', name: 'get_cwd', ok: true, result: { path: '/' } } },
    ]);
    expect(ok.items[0]).toMatchObject({ kind: 'tool', status: 'ok', result: { path: '/' } });

    const failed = run([
      { type: 'agent_event', event: { type: 'tool_call', call_id: 'c2', name: 'run_command', args: {} } },
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'c2', name: 'run_command', ok: false, result: { error: 'busy' } } },
    ]);
    expect(failed.items[0]).toMatchObject({ kind: 'tool', status: 'failed' });
  });

  it('a tool result without its call is still shown', () => {
    const t = run([
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'orphan', name: 'get_cwd', ok: true, result: null } },
    ]);
    expect(t.items).toHaveLength(1);
    expect(t.items[0]).toMatchObject({ kind: 'tool', callId: 'orphan', status: 'ok' });
  });

  it('an approval request is pending until its tool result arrives', () => {
    const asked = run([
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a1', text: 'pwd' } },
    ]);
    expect(asked.items[0]).toMatchObject({ kind: 'approval', state: 'pending' });
    expect(firstPendingApproval(asked)).toBe('a1');

    const done = transcriptReducer(asked, {
      type: 'agent_event',
      event: { type: 'tool_result', call_id: 'a1', name: 'run_command', ok: true, result: {} },
    });
    expect(done.items[0]).toMatchObject({ kind: 'approval', state: 'resolved' });
    expect(firstPendingApproval(done)).toBeNull();
  });

  it('done ends the in-flight turn', () => {
    const t = run([
      { type: 'user_sent', clientId: 1, text: 'hi' },
      { type: 'agent_event', event: { type: 'text', text: 'hello' } },
    ]);
    expect(isBusy(t)).toBe(true);
    const finished = transcriptReducer(t, { type: 'agent_event', event: { type: 'done' } });
    expect(isBusy(finished)).toBe(false);
  });
});

describe('transcriptReducer: local actions', () => {
  it('a sent message shows as the user, and the turn is busy until done', () => {
    const t = run([{ type: 'user_sent', clientId: 7, text: 'what was the last command' }]);
    expect(t.items[0]).toMatchObject({ kind: 'user', clientId: 7, text: 'what was the last command' });
    expect(isBusy(t)).toBe(true);
  });

  it('a refused message is removed, shows the reason, and does not leave the turn busy', () => {
    const t = run([
      { type: 'user_sent', clientId: 3, text: 'second message' },
      { type: 'user_failed', clientId: 3, message: 'the agent is still working on the previous message' },
    ]);
    expect(t.items.map((i) => i.kind)).toEqual(['notice']);
    expect(t.items[0]).toMatchObject({ text: 'the agent is still working on the previous message' });
    expect(isBusy(t)).toBe(false);
  });

  it('an in-progress turn is not cancelled by a refused second message', () => {
    const t = run([
      { type: 'user_sent', clientId: 1, text: 'first' },
      { type: 'user_sent', clientId: 2, text: 'second' },
      { type: 'user_failed', clientId: 2, message: 'busy' },
    ]);
    expect(isBusy(t)).toBe(true);
    expect(transcriptReducer(t, { type: 'agent_event', event: { type: 'done' } }).inFlight).toBe(0);
  });

  it('approving marks the card as answering, and a failed call returns it to pending', () => {
    const asked = run([{ type: 'agent_event', event: { type: 'approval_request', call_id: 'a1', text: 'pwd' } }]);
    const answering = transcriptReducer(asked, { type: 'approval_answering', callId: 'a1', approve: true });
    expect(answering.items[0]).toMatchObject({ kind: 'approval', state: 'answering', approve: true });
    expect(firstPendingApproval(answering)).toBeNull();

    const back = transcriptReducer(answering, { type: 'approval_failed', callId: 'a1', message: 'no pending approval a1' });
    expect(back.items[0]).toMatchObject({ kind: 'approval', state: 'pending', approve: null });
    expect(back.items[1]).toMatchObject({ kind: 'notice', text: 'no pending approval a1' });
  });

  it('the first pending approval is the oldest one', () => {
    const t = run([
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a1', text: 'x' } },
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a2', text: 'y' } },
    ]);
    expect(firstPendingApproval(t)).toBe('a1');
  });

  it('sync_busy sets the in-flight count from the status snapshot', () => {
    expect(isBusy(run([{ type: 'sync_busy', busy: true }]))).toBe(true);
    expect(isBusy(run([{ type: 'sync_busy', busy: false }]))).toBe(false);
  });

  it('a new session keeps the conversation, closes what the old shell left open, and says so', () => {
    const before = run([
      { type: 'user_sent', clientId: 1, text: 'list files' },
      { type: 'agent_event', event: { type: 'tool_call', call_id: 'c1', name: 'run_command', args: {} } },
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a1', text: 'ls' } },
      { type: 'agent_event', event: { type: 'tool_call', call_id: 'c0', name: 'get_cwd', args: {} } },
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'c0', name: 'get_cwd', ok: true, result: {} } },
    ]);
    const after = transcriptReducer(before, { type: 'session_reset' });

    expect(after.items.map((i) => i.kind)).toEqual(['user', 'tool', 'approval', 'tool', 'notice']);
    expect(after.items[1]).toMatchObject({ kind: 'tool', callId: 'c1', status: 'failed' });
    expect(after.items[2]).toMatchObject({ kind: 'approval', state: 'resolved' });
    expect(after.items[3]).toMatchObject({ kind: 'tool', callId: 'c0', status: 'ok' });
    expect(after.items[after.items.length - 1]).toMatchObject({ kind: 'notice', text: 'New shell session started.' });
    expect(firstPendingApproval(after)).toBeNull();
    expect(isBusy(after)).toBe(false);
  });

  it('the transcript stays bounded, and ids keep increasing', () => {
    const actions: TranscriptAction[] = Array.from({ length: MAX_ITEMS + 25 }, (_, i) => ({
      type: 'notice',
      text: `n${i}`,
    }));
    const t = run(actions);
    expect(t.items).toHaveLength(MAX_ITEMS);
    expect(t.items[0]).toMatchObject({ text: 'n25' });
    const ids = t.items.map((i) => i.id);
    expect(ids).toEqual([...ids].sort((a, b) => a - b));
  });
});

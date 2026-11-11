import { describe, expect, it } from 'vitest';
import { approvalTitle, EMPTY_TRANSCRIPT, firstPendingApproval, isBusy, MAX_ITEMS, transcriptReducer } from './transcript';
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
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'c1', name: 'get_cwd', ok: true, decision: 'read', result: { path: '/' } } },
    ]);
    expect(ok.items[0]).toMatchObject({ kind: 'tool', status: 'ok', result: { path: '/' } });

    const failed = run([
      { type: 'agent_event', event: { type: 'tool_call', call_id: 'c2', name: 'run_command', args: {} } },
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'c2', name: 'run_command', ok: false, decision: 'refused', result: { error: 'busy' } } },
    ]);
    expect(failed.items[0]).toMatchObject({ kind: 'tool', status: 'failed' });
  });

  it('a tool result without its call is still shown', () => {
    const t = run([
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'orphan', name: 'get_cwd', ok: true, decision: 'read', result: null } },
    ]);
    expect(t.items).toHaveLength(1);
    expect(t.items[0]).toMatchObject({ kind: 'tool', callId: 'orphan', status: 'ok' });
  });

  it('an approval request is pending until its tool result arrives', () => {
    const asked = run([
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a1', name: 'run_command', text: 'pwd' } },
    ]);
    expect(asked.items[0]).toMatchObject({ kind: 'approval', state: 'pending' });
    expect(firstPendingApproval(asked)).toBe('a1');

    const done = transcriptReducer(asked, {
      type: 'agent_event',
      event: { type: 'tool_result', call_id: 'a1', name: 'run_command', ok: true, decision: 'manual', result: {} },
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
    const asked = run([
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a1', name: 'run_command', text: 'pwd' } },
    ]);
    const answering = transcriptReducer(asked, { type: 'approval_answering', callId: 'a1', approve: true });
    expect(answering.items[0]).toMatchObject({ kind: 'approval', state: 'answering', approve: true });
    expect(firstPendingApproval(answering)).toBeNull();

    const back = transcriptReducer(answering, { type: 'approval_failed', callId: 'a1', message: 'no pending approval a1' });
    expect(back.items[0]).toMatchObject({ kind: 'approval', state: 'pending', approve: null });
    expect(back.items[1]).toMatchObject({ kind: 'notice', text: 'no pending approval a1' });
  });

  it('an approval card for a nested shell carries its location; a local one has none', () => {
    const t = run([
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a1', name: 'run_command', text: 'pwd', level: 1, location: 'ssh ubuntu@lidia' } },
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a2', name: 'run_command', text: 'pwd', level: 0 } },
    ]);
    expect(t.items[0]).toMatchObject({ kind: 'approval', location: 'ssh ubuntu@lidia' });
    expect(t.items[1]).toMatchObject({ kind: 'approval', location: null });
  });

  it('the first pending approval is the oldest one', () => {
    const t = run([
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a1', name: 'run_command', text: 'x' } },
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a2', name: 'run_command', text: 'y' } },
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
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a1', name: 'run_command', text: 'ls' } },
      { type: 'agent_event', event: { type: 'tool_call', call_id: 'c0', name: 'get_cwd', args: {} } },
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'c0', name: 'get_cwd', ok: true, decision: 'read', result: {} } },
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

const delta = (text: string): TranscriptAction => ({ type: 'agent_event', event: { type: 'text_delta', text } });
const thinking = (active: boolean): TranscriptAction => ({ type: 'agent_event', event: { type: 'thinking', active } });
const done: TranscriptAction = { type: 'agent_event', event: { type: 'done' } };
const toolCall = (callId: string): TranscriptAction => ({
  type: 'agent_event',
  event: { type: 'tool_call', call_id: callId, name: 'get_cwd', args: {} },
});

describe('transcriptReducer: streamed text', () => {
  it('deltas accumulate into one agent message', () => {
    const t = run([delta('Hel'), delta('lo'), delta(', world')]);
    expect(t.items).toHaveLength(1);
    expect(t.items[0]).toMatchObject({ kind: 'agent', text: 'Hello, world', streaming: true });
  });

  it('a delta after a tool call starts a new agent message', () => {
    const t = run([delta('Checking.'), toolCall('c1'), delta('It is'), delta(' /tmp.')]);
    expect(t.items.map((i) => i.kind)).toEqual(['agent', 'tool', 'agent']);
    expect(t.items[0]).toMatchObject({ kind: 'agent', text: 'Checking.', streaming: false });
    expect(t.items[2]).toMatchObject({ kind: 'agent', text: 'It is /tmp.', streaming: true });
  });

  it('an approval, notice, or user item closes the open message', () => {
    const approval = run([delta('A'), { type: 'agent_event', event: { type: 'approval_request', call_id: 'a', name: 'run_command', text: 'ls' } }, delta('B')]);
    expect(approval.items.map((i) => i.kind)).toEqual(['agent', 'approval', 'agent']);
    expect(approval.items[0]).toMatchObject({ text: 'A', streaming: false });

    const notice = run([delta('A'), { type: 'notice', text: 'n' }, delta('B')]);
    expect(notice.items.map((i) => i.kind)).toEqual(['agent', 'notice', 'agent']);
    expect(notice.items[2]).toMatchObject({ text: 'B' });

    const user = run([delta('A'), { type: 'user_sent', clientId: 1, text: 'next' }, delta('B')]);
    expect(user.items.map((i) => i.kind)).toEqual(['agent', 'user', 'agent']);
    expect(user.items[0]).toMatchObject({ text: 'A', streaming: false });
  });

  it('done closes the open message, so the next turn starts a new one', () => {
    const t = run([{ type: 'user_sent', clientId: 1, text: 'hi' }, delta('Hello'), done, delta('Again')]);
    expect(t.items.map((i) => i.kind)).toEqual(['user', 'agent', 'agent']);
    expect(t.items[1]).toMatchObject({ text: 'Hello', streaming: false });
    expect(t.items[2]).toMatchObject({ text: 'Again', streaming: true });
  });

  it('an empty delta changes nothing', () => {
    const start = run([delta('a')]);
    expect(run([delta('')], start)).toEqual(start);
    expect(run([delta('')])).toEqual(EMPTY_TRANSCRIPT);
  });

  it('a whole text event creates its own message and does not join a streamed one', () => {
    const t = run([delta('part'), { type: 'agent_event', event: { type: 'text', text: 'whole' } }]);
    expect(t.items).toHaveLength(2);
    expect(t.items[0]).toMatchObject({ kind: 'agent', text: 'part', streaming: false });
    expect(t.items[1]).toMatchObject({ kind: 'agent', text: 'whole', streaming: false });
  });

  it('a whole text event after another whole text stays separate', () => {
    const t = run([
      { type: 'agent_event', event: { type: 'text', text: 'one' } },
      { type: 'agent_event', event: { type: 'text', text: 'two' } },
    ]);
    expect(t.items.map((i) => (i.kind === 'agent' ? i.text : ''))).toEqual(['one', 'two']);
  });

  it('a streamed message at the cap is extended, then closed by the next item, and trimming does not break it', () => {
    const fill: TranscriptAction[] = Array.from({ length: MAX_ITEMS - 1 }, (_, i) => ({ type: 'notice', text: `n${i}` }));
    const atCap = run([...fill, delta('a'), delta('b')]);
    expect(atCap.items).toHaveLength(MAX_ITEMS);
    expect(atCap.items[MAX_ITEMS - 1]).toMatchObject({ kind: 'agent', text: 'ab', streaming: true });

    const past = run([...fill, delta('a'), delta('b'), { type: 'notice', text: 'after' }]);
    expect(past.items).toHaveLength(MAX_ITEMS);
    expect(past.items[MAX_ITEMS - 2]).toMatchObject({ kind: 'agent', text: 'ab', streaming: false });
    expect(past.items[MAX_ITEMS - 1]).toMatchObject({ kind: 'notice', text: 'after' });

    // Trimming while a streamed message is open, then more deltas: nothing crashes and the message keeps growing.
    const manyDeltas = run([...Array.from({ length: MAX_ITEMS + 5 }, (_, i) => ({ type: 'notice' as const, text: `n${i}` })), delta('x'), delta('y')]);
    expect(manyDeltas.items).toHaveLength(MAX_ITEMS);
    expect(manyDeltas.items[MAX_ITEMS - 1]).toMatchObject({ kind: 'agent', text: 'xy', streaming: true });
  });
});

describe('transcriptReducer: thinking', () => {
  it('thinking true shows the indicator, and false clears it', () => {
    expect(run([thinking(true)]).thinking).toBe(true);
    expect(run([thinking(true), thinking(false)]).thinking).toBe(false);
  });

  it('the first text_delta clears thinking', () => {
    const t = run([thinking(true), delta('H')]);
    expect(t.thinking).toBe(false);
    expect(t.items[0]).toMatchObject({ text: 'H' });
  });

  it('done clears thinking', () => {
    expect(run([thinking(true), done]).thinking).toBe(false);
  });

  it('a session reset clears thinking', () => {
    expect(run([thinking(true), { type: 'session_reset' }]).thinking).toBe(false);
  });

  it('thinking is not cleared by a whole text event or a tool call', () => {
    expect(run([thinking(true), { type: 'agent_event', event: { type: 'text', text: 'x' } }]).thinking).toBe(true);
    expect(run([thinking(true), toolCall('c1')]).thinking).toBe(true);
  });
});

describe('transcriptReducer: approvals and decisions', () => {
  it('an approval request stores the tool name', () => {
    const t = run([{ type: 'agent_event', event: { type: 'approval_request', call_id: 'a1', name: 'send_keys', text: 'ls' } }]);
    expect(t.items[0]).toMatchObject({ kind: 'approval', name: 'send_keys', text: 'ls', state: 'pending' });
  });

  it('a tool result stores its decision on the tool item', () => {
    const t = run([
      toolCall('c1'),
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'c1', name: 'get_cwd', ok: true, decision: 'read', result: {} } },
    ]);
    expect(t.items[0]).toMatchObject({ kind: 'tool', decision: 'read', status: 'ok' });
  });

  it('a tool call has no decision until its result arrives', () => {
    expect(run([toolCall('c1')]).items[0]).toMatchObject({ kind: 'tool', decision: null });
  });

  it('an orphan result keeps its decision', () => {
    const t = run([{ type: 'agent_event', event: { type: 'tool_result', call_id: 'o', name: 'run_command', ok: false, decision: 'denied', result: {} } }]);
    expect(t.items[0]).toMatchObject({ kind: 'tool', decision: 'denied', status: 'failed' });
  });
});

describe('transcriptReducer: approval reasons', () => {
  it('an approval request carries its reason, and null without one', () => {
    const t = run([
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a1', name: 'run_command', text: 'ls | wc', reason: 'gate_chars' } },
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a2', name: 'send_keys', text: 'ls' } },
    ]);
    expect(t.items[0]).toMatchObject({ kind: 'approval', reason: 'gate_chars' });
    expect(t.items[1]).toMatchObject({ kind: 'approval', reason: null });
  });

  it('the reason stays on the card after it is resolved', () => {
    const t = run([
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a1', name: 'run_command', text: 'ls', reason: 'no_rule' } },
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'a1', name: 'run_command', ok: true, decision: 'manual', reason: 'no_rule', result: {} } },
    ]);
    expect(t.items[0]).toMatchObject({ kind: 'approval', state: 'resolved', reason: 'no_rule' });
  });

  it('a tool result carries its reason onto the tool item, and a call has none until then', () => {
    const before = run([toolCall('c1')]);
    expect(before.items[0]).toMatchObject({ kind: 'tool', reason: null });
    const after = run([
      toolCall('c1'),
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'c1', name: 'run_command', ok: true, decision: 'auto', reason: 'rule', result: {} } },
    ]);
    expect(after.items[0]).toMatchObject({ kind: 'tool', decision: 'auto', reason: 'rule' });
  });

  it('a result without a reason (a read tool) leaves it null', () => {
    const t = run([
      toolCall('c1'),
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'c1', name: 'get_cwd', ok: true, decision: 'read', result: {} } },
    ]);
    expect(t.items[0]).toMatchObject({ kind: 'tool', reason: null });
  });

  it('an orphan result keeps its reason', () => {
    const t = run([
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'o', name: 'run_command', ok: false, decision: 'denied', reason: 'deny_rule', result: {} } },
    ]);
    expect(t.items[0]).toMatchObject({ kind: 'tool', reason: 'deny_rule' });
  });
});

describe('transcriptReducer: conversation_reset', () => {
  it('empties the items and the thinking flag, and keeps ids increasing', () => {
    const before = run([
      { type: 'user_sent', clientId: 1, text: 'hi' },
      delta('Hello'),
      done,
      thinking(true),
    ]);
    const after = transcriptReducer(before, { type: 'conversation_reset' });
    expect(after.items).toEqual([]);
    expect(after.thinking).toBe(false);
    expect(after.inFlight).toBe(0);

    const next = transcriptReducer(after, { type: 'notice', text: 'fresh' });
    expect(next.items[0].id).toBeGreaterThan(before.nextId - 1);
  });
});

describe('approvalTitle', () => {
  it('names run_command and send_keys', () => {
    expect(approvalTitle('run_command')).toBe('The agent wants to run a command');
    expect(approvalTitle('send_keys')).toBe('The agent wants to type into a running command');
  });

  it('names any other tool, escaping the name', () => {
    expect(approvalTitle('fetch_url')).toBe('The agent wants to use fetch_url');
    expect(approvalTitle('x‮y')).toBe('The agent wants to use x\\u{202E}y');
    expect(approvalTitle('')).toBe('The agent wants to use ');
  });
});

describe('transcriptReducer: conversation_loaded', () => {
  const loaded = (...items: Array<{ kind: 'user'; clientId: number; text: string } | { kind: 'agent'; text: string; streaming: boolean }>) =>
    ({ type: 'conversation_loaded', items }) as TranscriptAction;

  it('replaces the items, with ids from nextId, and clears busy and thinking', () => {
    const before = run([
      { type: 'user_sent', clientId: 1, text: 'live' },
      { type: 'agent_event', event: { type: 'thinking', active: true } },
    ]);
    const after = transcriptReducer(
      before,
      loaded({ kind: 'user', clientId: 0, text: 'old question' }, { kind: 'agent', text: 'old answer', streaming: false }),
    );
    expect(after.items.map((i) => i.kind)).toEqual(['user', 'agent']);
    expect(after.items[0]).toMatchObject({ text: 'old question' });
    expect(after.items[0].id).toBe(before.nextId);
    expect(after.items[1].id).toBe(before.nextId + 1);
    expect(after.nextId).toBe(before.nextId + 2);
    expect(after.inFlight).toBe(0);
    expect(after.thinking).toBe(false);
    expect(isBusy(after)).toBe(false);
  });

  it('no loaded item is streaming, even if the input says so', () => {
    const after = transcriptReducer(EMPTY_TRANSCRIPT, loaded({ kind: 'agent', text: 'half', streaming: true }));
    expect(after.items[0]).toMatchObject({ kind: 'agent', text: 'half', streaming: false });
  });

  it('keeps the newest MAX_ITEMS, and ids keep increasing past the cap', () => {
    const many = Array.from({ length: MAX_ITEMS + 10 }, (_, i) => ({ kind: 'user' as const, clientId: 0, text: `u${i}` }));
    const after = transcriptReducer(EMPTY_TRANSCRIPT, loaded(...many));
    expect(after.items).toHaveLength(MAX_ITEMS);
    expect(after.items[0]).toMatchObject({ text: 'u10' });
    expect(after.items[MAX_ITEMS - 1]).toMatchObject({ text: `u${MAX_ITEMS + 9}` });
    const ids = after.items.map((i) => i.id);
    expect(ids).toEqual([...ids].sort((a, b) => a - b));
    expect(new Set(ids).size).toBe(ids.length);
    expect(after.nextId).toBe(EMPTY_TRANSCRIPT.nextId + MAX_ITEMS + 10);
  });

  it('an empty conversation empties the transcript', () => {
    const after = transcriptReducer(run([{ type: 'notice', text: 'x' }]), loaded());
    expect(after.items).toEqual([]);
  });

  it('a notice after a load gets a fresh id', () => {
    const after = run([loaded({ kind: 'user', clientId: 0, text: 'a' }), { type: 'notice', text: 'n' }]);
    expect(after.items[1].id).toBeGreaterThan(after.items[0].id);
  });
});

describe('transcriptReducer: todo list', () => {
  const todo = (content: string, status: 'pending' | 'in_progress' | 'completed' = 'pending') => ({ content, status });
  const call = (callId: string, name: string): TranscriptAction => ({
    type: 'agent_event',
    event: { type: 'tool_call', call_id: callId, name, args: { items: [] } },
  });
  const result = (callId: string, name: string, ok = true): TranscriptAction => ({
    type: 'agent_event',
    event: { type: 'tool_result', call_id: callId, name, ok, decision: 'auto', result: 'todo list updated' },
  });
  const update = (items: Array<{ content: string; status: 'pending' | 'in_progress' | 'completed' }>): TranscriptAction => ({
    type: 'agent_event',
    event: { type: 'todo_update', items },
  });

  it('starts empty', () => {
    expect(EMPTY_TRANSCRIPT.todos).toEqual([]);
  });

  it('todo_update replaces the whole list', () => {
    const first = run([update([todo('a', 'in_progress'), todo('b')])]);
    expect(first.todos).toEqual([todo('a', 'in_progress'), todo('b')]);
    const second = transcriptReducer(first, update([todo('b', 'in_progress')]));
    expect(second.todos).toEqual([todo('b', 'in_progress')]);
  });

  it('todo_update does not add a transcript item or change the in-flight count', () => {
    const before = run([{ type: 'user_sent', clientId: 1, text: 'plan' }]);
    const after = transcriptReducer(before, update([todo('a', 'in_progress')]));
    expect(after.items).toEqual(before.items);
    expect(after.inFlight).toBe(before.inFlight);
  });

  it('conversation_reset clears the list', () => {
    const after = run([update([todo('a')]), { type: 'conversation_reset' }]);
    expect(after.todos).toEqual([]);
  });

  it('conversation_loaded sets the list from the loaded todos', () => {
    const after = run([
      update([todo('live')]),
      { type: 'conversation_loaded', items: [], todos: [todo('saved', 'in_progress')] },
    ]);
    expect(after.todos).toEqual([todo('saved', 'in_progress')]);
  });

  it('conversation_loaded without todos leaves the list empty', () => {
    const after = run([update([todo('live')]), { type: 'conversation_loaded', items: [] }]);
    expect(after.todos).toEqual([]);
  });

  it('session_reset keeps the list, since the conversation is kept', () => {
    const after = run([update([todo('a', 'in_progress')]), { type: 'session_reset' }]);
    expect(after.todos).toEqual([todo('a', 'in_progress')]);
  });

  it('todo_write calls and results are not items, and a running todo_write is not in flight', () => {
    const before = run([{ type: 'user_sent', clientId: 1, text: 'plan' }]);
    const after = run([call('c1', 'todo_write'), result('c1', 'todo_write')], before);
    expect(after.items).toEqual(before.items);
    expect(after.inFlight).toBe(before.inFlight);
    expect(after.nextId).toBe(before.nextId);
  });

  it('a todo_write result with no call is not shown as an orphan row', () => {
    const after = run([result('c9', 'todo_write', false)]);
    expect(after.items).toEqual([]);
  });

  it('other tools are still shown next to a todo_write call', () => {
    const after = run([call('c1', 'todo_write'), call('c2', 'run_command'), result('c2', 'run_command'), result('c1', 'todo_write')]);
    expect(after.items.map((i) => (i.kind === 'tool' ? i.name : i.kind))).toEqual(['run_command']);
    expect(after.items[0]).toMatchObject({ kind: 'tool', status: 'ok' });
  });
});

describe('transcriptReducer: integration requests', () => {
  it('the integration request events add no rows: the tool call and its result are the row', () => {
    const before = run([{ type: 'agent_event', event: { type: 'text', text: 'hi' } }]);
    const after = run(
      [
        { type: 'agent_event', event: { type: 'integration_request', call_id: 'c1', phase: 'asking', label: 'ssh ubuntu@lidia' } },
        { type: 'agent_event', event: { type: 'integration_request', call_id: 'c1', phase: 'running', label: 'ssh ubuntu@lidia' } },
        { type: 'agent_event', event: { type: 'integration_request', call_id: 'c1', phase: 'done', outcome: { outcome: 'declined' } } },
      ],
      before,
    );
    expect(after.items).toEqual(before.items);
    expect(after.inFlight).toBe(before.inFlight);
  });
});

describe('transcriptReducer: send_keys cards and tool start times', () => {
  it('an approval card keeps the decoded display of a send_keys request', () => {
    const t = run([
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'k1', name: 'send_keys', text: 'echo hi⏎', display: 'echo hi⏎', reason: 'unintegrated_shell' } },
    ]);
    expect(t.items[0]).toMatchObject({ kind: 'approval', display: 'echo hi⏎', reason: 'unintegrated_shell' });
  });

  it('an approval card without a display (other tools) has null', () => {
    const t = run([{ type: 'agent_event', event: { type: 'approval_request', call_id: 'a1', name: 'run_command', text: 'ls' } }]);
    expect(t.items[0]).toMatchObject({ kind: 'approval', display: null });
  });

  it('a tool call records when it started, from the arrival time of the event', () => {
    const t = run([{ type: 'agent_event', event: { type: 'tool_call', call_id: 'c1', name: 'wait', args: {} }, at: 5000 }]);
    expect(t.items[0]).toMatchObject({ kind: 'tool', startedAt: 5000 });
  });

  it('a session reset clears the finished-command memory', () => {
    const t = run([
      { type: 'record_finished', record: { id: 4, command: 'x', source: { kind: 'agent', call_id: 'r' }, exit_code: 0, status: 'complete', used_alt_screen: false, output_bytes: 0 } },
      { type: 'session_reset' },
    ]);
    expect(t.finished).toEqual([]);
  });
});

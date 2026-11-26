import { describe, expect, it } from 'vitest';
import { activityLabel, approvalTitle, EMPTY_TRANSCRIPT, firstPendingApproval, hasOlder, isBusy, MAX_ITEMS, REDO_NOTE, TRIM_NOTE, transcriptReducer } from './transcript';
import { itemsFromMessages } from './historyView';
import { APPROVAL_WINDOW_MS } from './approvalTime';
import type { ChatMessage, QuestionSpec } from './types';
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

  it('an approval request stamps its expiry two minutes after it arrives', () => {
    const stamped = run([
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a1', name: 'run_command', text: 'pwd' }, at: 1_000 },
    ]);
    expect(stamped.items[0]).toMatchObject({ kind: 'approval', expiresAt: 1_000 + APPROVAL_WINDOW_MS });
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

  it('a notice event adds a muted notice item', () => {
    const t = run([
      { type: 'agent_event', event: { type: 'notice', text: 'Using project instructions from /r/AGENTS.md' } },
    ]);
    expect(t.items).toHaveLength(1);
    expect(t.items[0]).toMatchObject({ kind: 'notice', text: 'Using project instructions from /r/AGENTS.md', muted: true });
  });
});

describe('transcriptReducer: context usage', () => {
  it('a context_usage event is not a transcript item (it sets the meter value only)', () => {
    const before = run([{ type: 'agent_event', event: { type: 'text', text: 'Hi.' } }]);
    const after = run([
      { type: 'agent_event', event: { type: 'text', text: 'Hi.' } },
      {
        type: 'agent_event',
        event: {
          type: 'context_usage',
          limit: 8000,
          used: 1234,
          reserved_output: 4096,
          trimmed: false,
          calibrated: true,
          categories: [{ id: 'system', tokens: 1000 }, { id: 'user', tokens: 234 }],
        },
      },
    ]);
    expect(after.items).toEqual(before.items);
    expect(after.items.map((i) => i.kind)).toEqual(['agent']);
  });

  it('a context_usage event with no limit does not disturb a running turn', () => {
    const t = run([
      { type: 'user_sent', clientId: 1, text: 'look' },
      {
        type: 'agent_event',
        event: { type: 'context_usage', limit: null, used: 10, reserved_output: 4096, trimmed: false, calibrated: false, categories: [] },
      },
    ]);
    expect(isBusy(t)).toBe(true);
    expect(t.items.map((i) => i.kind)).toEqual(['user']);
  });
});

describe('transcriptReducer: steered messages', () => {
  const done: TranscriptAction = { type: 'agent_event', event: { type: 'done' } };
  const injected = (text: string, attachments: { kind: 'image' | 'file'; name: string; bytes: number }[] = []): TranscriptAction => ({
    type: 'agent_event',
    event: { type: 'steer_injected', ids: [1], text, attachments },
  });

  it('a steer taken mid-turn does not count a turn in flight, so the turn still ends idle at done', () => {
    // The trap: user_sent counts a turn, done ends one. Reusing user_sent for a steer would leave the UI busy forever.
    const t = run([{ type: 'user_sent', clientId: 1, text: 'start' }, injected('steer'), done]);
    expect(t.inFlight).toBe(0);
    expect(isBusy(t)).toBe(false);
  });

  it('a steer alone (user_steered) leaves inFlight where it was', () => {
    const before = run([{ type: 'user_sent', clientId: 1, text: 'start' }]);
    const after = run([{ type: 'user_steered', text: 'more' }], before);
    expect(after.inFlight).toBe(1);
    expect(run([{ type: 'user_steered', text: 'more' }]).inFlight).toBe(0);
  });

  it('a steered message shows as a user message at the step it went in, with its attachments', () => {
    const attachments = [{ kind: 'file' as const, name: 'notes.txt', bytes: 12 }];
    const t = run([{ type: 'user_sent', clientId: 1, text: 'start' }, injected('look at this', attachments)]);
    expect(t.items[1]).toEqual({ id: t.items[1].id, kind: 'user', text: 'look at this', attachments });
    expect(t.items[1]).not.toHaveProperty('clientId');
  });

  it('a steer with no attachments shows no attachments field', () => {
    const t = run([injected('plain')]);
    expect(t.items[0]).toEqual({ id: t.items[0].id, kind: 'user', text: 'plain' });
  });

  it('a steer taken after a tool call lands after that call', () => {
    const t = run([
      { type: 'user_sent', clientId: 1, text: 'start' },
      { type: 'agent_event', event: { type: 'tool_call', call_id: 'c1', name: 'get_cwd', args: {} } },
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'c1', name: 'get_cwd', ok: true, decision: 'read', result: {} } },
      injected('next'),
    ]);
    expect(t.items.map((i) => i.kind)).toEqual(['user', 'tool', 'user']);
  });

  it('steer_returned adds nothing to the transcript', () => {
    const before = run([{ type: 'user_sent', clientId: 1, text: 'start' }]);
    const after = run([{ type: 'agent_event', event: { type: 'steer_returned', ids: [1] } }], before);
    expect(after).toBe(before);
  });

  it('a failed send does not remove a steered message (it has no clientId to match)', () => {
    const t = run([injected('steered'), { type: 'user_failed', clientId: 0, message: 'no' }]);
    expect(t.items.map((i) => i.kind)).toEqual(['user', 'notice']);
  });
});

describe('transcriptReducer: local actions', () => {
  it('a sent message shows as the user, and the turn is busy until done', () => {
    const t = run([{ type: 'user_sent', clientId: 7, text: 'what was the last command' }]);
    expect(t.items[0]).toMatchObject({ kind: 'user', clientId: 7, text: 'what was the last command' });
    expect(isBusy(t)).toBe(true);
  });

  it('a sent message carries its attachments; without any, the item has no attachments field', () => {
    const attachments = [{ kind: 'image' as const, name: 'shot.png', bytes: 2048, thumb: 'data:image/jpeg;base64,AAAA' }];
    const withFiles = run([{ type: 'user_sent', clientId: 8, text: '', attachments }]);
    expect(withFiles.items[0]).toMatchObject({ kind: 'user', text: '', attachments });
    const none = run([{ type: 'user_sent', clientId: 9, text: 'plain', attachments: [] }]);
    expect(none.items[0]).not.toHaveProperty('attachments');
    const absent = run([{ type: 'user_sent', clientId: 10, text: 'plain' }]);
    expect(absent.items[0]).not.toHaveProperty('attachments');
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

  it('an accepted answer resolves the card as Approved while the command runs, and tool_result settles it again', () => {
    const asked = run([
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a1', name: 'run_command', text: 'npm install' } },
    ]);
    const answering = transcriptReducer(asked, { type: 'approval_answering', callId: 'a1', approve: true });
    const sent = transcriptReducer(answering, { type: 'approval_sent', callId: 'a1' });
    expect(sent.items[0]).toMatchObject({ kind: 'approval', state: 'resolved', approve: true });
    expect(firstPendingApproval(sent)).toBeNull();
    // A later tool_result changes nothing more on the card.
    const done = transcriptReducer(sent, {
      type: 'agent_event',
      event: { type: 'tool_result', call_id: 'a1', name: 'run_command', ok: true, decision: 'manual', result: {} },
    });
    expect(done.items[0]).toMatchObject({ kind: 'approval', state: 'resolved', approve: true });
  });

  it('an accepted answer does not touch a card that is not answering', () => {
    const asked = run([
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a1', name: 'run_command', text: 'pwd' } },
    ]);
    expect(transcriptReducer(asked, { type: 'approval_sent', callId: 'a1' }).items[0]).toMatchObject({ state: 'pending' });
    const answering = transcriptReducer(asked, { type: 'approval_answering', callId: 'a1', approve: false });
    expect(transcriptReducer(answering, { type: 'approval_sent', callId: 'other' }).items[0]).toMatchObject({ state: 'answering' });
  });

  it('an approval card for a nested shell carries its location; a local one has none', () => {
    const t = run([
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a1', name: 'run_command', text: 'pwd', level: 1, location: 'ssh ubuntu@lidia' } },
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a2', name: 'run_command', text: 'pwd', level: 0 } },
    ]);
    expect(t.items[0]).toMatchObject({ kind: 'approval', location: 'ssh ubuntu@lidia' });
    expect(t.items[1]).toMatchObject({ kind: 'approval', location: null });
  });

  it('an approval card carries the title of its terminal when the caller gives one; without it there is no terminal field', () => {
    const t = run([
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a1', name: 'run_command', text: 'pwd' }, terminal: 'Terminal 2' },
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'a2', name: 'run_command', text: 'pwd' } },
    ]);
    expect(t.items[0]).toMatchObject({ kind: 'approval', terminal: 'Terminal 2' });
    expect(t.items[1]).not.toHaveProperty('terminal');
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

  it('a notice keeps its muted flag, and defaults to unmuted', () => {
    const muted = run([{ type: 'notice', text: 'info', muted: true }]);
    expect(muted.items[0]).toMatchObject({ kind: 'notice', text: 'info', muted: true });
    const plain = run([{ type: 'notice', text: 'warn' }]);
    expect(plain.items[0]).toMatchObject({ kind: 'notice', text: 'warn' });
    expect(plain.items[0]).not.toHaveProperty('muted');
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

describe('transcriptReducer: clarification questions', () => {
  const questions: QuestionSpec[] = [
    {
      id: 'q1',
      question: 'Which capa?',
      header: 'Capa',
      options: [
        { label: 'a', description: 'first', recommended: true },
        { label: 'b', description: '', recommended: false },
      ],
      multiple: false,
    },
  ];
  const request: TranscriptAction = { type: 'agent_event', event: { type: 'question_request', call_id: 'q-call', questions } };
  const answers = [{ question_id: 'q1', selected: [0], skipped: false }];
  const resultValue = { answers: [{ question_id: 'q1', question: 'Which capa?', header: 'Capa', status: 'answered', selected: ['a'] }] };
  const resultOk: TranscriptAction = {
    type: 'agent_event',
    event: { type: 'tool_result', call_id: 'q-call', name: 'ask_user', ok: true, decision: 'read', result: resultValue },
  };

  it('a question request adds a pending card, and no tool row', () => {
    const t = run([
      request,
      { type: 'agent_event', event: { type: 'tool_call', call_id: 'q-call', name: 'ask_user', args: {} } },
    ]);
    expect(t.items.map((i) => i.kind)).toEqual(['question']);
    expect(t.items[0]).toMatchObject({ kind: 'question', callId: 'q-call', state: 'pending', questions, answers: null, result: null, error: null });
  });

  it('a question is not a pending approval and does not make the transcript busy', () => {
    const t = run([request]);
    expect(firstPendingApproval(t)).toBeNull();
    expect(isBusy(t)).toBe(false);
  });

  it('answering marks the card, and its result resolves it with the parsed outcome', () => {
    const sending = transcriptReducer(run([request]), { type: 'question_answering', callId: 'q-call', answers });
    expect(sending.items[0]).toMatchObject({ kind: 'question', state: 'answering', answers });
    const done = transcriptReducer(sending, resultOk);
    expect(done.items[0]).toMatchObject({ kind: 'question', state: 'resolved', result: resultValue, error: null });
    expect(done.items.map((i) => i.kind)).toEqual(['question']);
  });

  it('a result with an unparsable body fails the card instead', () => {
    const t = run([
      request,
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'q-call', name: 'ask_user', ok: true, decision: 'read', result: 'oops' } },
    ]);
    expect(t.items[0]).toMatchObject({ kind: 'question', state: 'failed' });
  });

  it('a failed result (an invalid call) fails the card with the error, and adds no tool row', () => {
    const t = run([
      request,
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'q-call', name: 'ask_user', ok: false, decision: 'read', result: { error: 'no questions' } } },
    ]);
    expect(t.items.map((i) => i.kind)).toEqual(['question']);
    expect(t.items[0]).toMatchObject({ kind: 'question', state: 'failed', error: 'no questions' });
  });

  it('an ask_user result with no card (an invalid call, which never asked) shows nothing', () => {
    const t = run([
      { type: 'agent_event', event: { type: 'tool_call', call_id: 'bad', name: 'ask_user', args: {} } },
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'bad', name: 'ask_user', ok: false, decision: 'read', result: { error: 'no questions' } } },
    ]);
    expect(t.items).toEqual([]);
  });

  it('a failed send marks the card failed with the reason, and a retry sends again', () => {
    const sending = transcriptReducer(run([request]), { type: 'question_answering', callId: 'q-call', answers });
    const failed = transcriptReducer(sending, { type: 'question_failed', callId: 'q-call', message: 'no pending question q-call' });
    expect(failed.items[0]).toMatchObject({ kind: 'question', state: 'failed', error: 'no pending question q-call' });
    expect(failed.items).toHaveLength(1);
    const retry = transcriptReducer(failed, { type: 'question_answering', callId: 'q-call', answers });
    expect(retry.items[0]).toMatchObject({ kind: 'question', state: 'answering', error: null });
  });

  it('done closes a still-pending question, and leaves a resolved one alone', () => {
    const pending = run([request, done]);
    expect(pending.items[0]).toMatchObject({ kind: 'question', state: 'closed', note: 'The turn ended before this question was answered.' });
    expect(pending.items[0]).not.toMatchObject({ kind: 'tool' });
    const resolved = run([request, resultOk, done]);
    expect(resolved.items[0]).toMatchObject({ kind: 'question', state: 'resolved' });
    expect(isBusy(resolved)).toBe(false);
  });

  it('a failed send stays retryable through done', () => {
    const sending = transcriptReducer(run([request]), { type: 'question_answering', callId: 'q-call', answers });
    const failed = transcriptReducer(sending, { type: 'question_failed', callId: 'q-call', message: 'no pending question q-call' });
    const after = transcriptReducer(failed, done);
    expect(after.items[0]).toMatchObject({ kind: 'question', state: 'failed', error: 'no pending question q-call' });
  });

  it('a session reset closes a waiting question, and keeps a resolved one and a failed send', () => {
    const before = run([request, resultOk]);
    const asked = run([{ type: 'agent_event', event: { type: 'question_request', call_id: 'q2', questions } }], before);
    const after = transcriptReducer(asked, { type: 'session_reset' });
    expect(after.items[0]).toMatchObject({ kind: 'question', state: 'resolved' });
    expect(after.items[1]).toMatchObject({ kind: 'question', state: 'closed', note: 'The shell restarted before this question was answered.' });
    const sending = transcriptReducer(run([request]), { type: 'question_answering', callId: 'q-call', answers });
    const failed = transcriptReducer(sending, { type: 'question_failed', callId: 'q-call', message: 'gone' });
    const reset = transcriptReducer(failed, { type: 'session_reset' });
    expect(reset.items[0]).toMatchObject({ kind: 'question', state: 'failed', error: 'gone' });
    const resetWhileSending = transcriptReducer(sending, { type: 'session_reset' });
    expect(resetWhileSending.items[0]).toMatchObject({ kind: 'question', state: 'closed' });
  });

  it('a conversation reset clears the cards normally', () => {
    const after = run([request, resultOk, { type: 'conversation_reset' }]);
    expect(after.items).toEqual([]);
  });
});

describe('approvalTitle', () => {
  it('names the file writes', () => {
    expect(approvalTitle('write_file', 'fs_write')).toBe('The agent wants to write a file');
    expect(approvalTitle('edit_file', 'fs_write')).toBe('The agent wants to edit a file');
  });

  it('names run_command and send_keys', () => {
    expect(approvalTitle('run_command')).toBe('The agent wants to run a command');
    expect(approvalTitle('send_keys')).toBe('The agent wants to type into a running command');
  });

  it('names a skill load, and a file read from a skill, from the card display', () => {
    expect(approvalTitle('skill', 'skill_read', 'Load skill "demo"')).toBe('The agent wants to load a skill');
    expect(approvalTitle('skill', 'skill_read', 'Read "scripts/check.sh" from skill "demo"')).toBe('The agent wants to read a file from a skill');
    expect(approvalTitle('skill')).toBe('The agent wants to load a skill');
  });

  it('names any other tool, escaping the name', () => {
    expect(approvalTitle('fetch_url')).toBe('The agent wants to use fetch_url');
    expect(approvalTitle('x‮y')).toBe('The agent wants to use x\\u{202E}y');
    expect(approvalTitle('')).toBe('The agent wants to use ');
  });

  it('names the server and tool of an external tool from its display (server: tool)', () => {
    expect(approvalTitle('mcp__fetch__fetch', 'external_tool', 'fetch: fetch')).toBe('The agent wants to use fetch from fetch');
    expect(approvalTitle('mcp__you-com__search', 'external_tool', 'you-com: search')).toBe('The agent wants to use search from you-com');
    // The server is the part before the first ': '; the tool keeps the rest.
    expect(approvalTitle('mcp__ctx__a', 'external_tool', 'ctx: a: b')).toBe('The agent wants to use a: b from ctx');
  });

  it('falls back to the name when the display is missing or has no server part', () => {
    expect(approvalTitle('mcp__fetch__fetch', 'external_tool', null)).toBe('The agent wants to use mcp__fetch__fetch');
    expect(approvalTitle('mcp__fetch__fetch', 'external_tool', 'no separator')).toBe('The agent wants to use mcp__fetch__fetch');
    expect(approvalTitle('mcp__fetch__fetch', 'external_tool', ': tool')).toBe('The agent wants to use mcp__fetch__fetch');
  });

  it('ignores the display for other reasons, so a send_keys display is not a title', () => {
    expect(approvalTitle('send_keys', 'send_keys', 'echo hi')).toBe('The agent wants to type into a running command');
    expect(approvalTitle('fetch_url', 'manual_mode', 'a: b')).toBe('The agent wants to use fetch_url');
  });

  it('escapes the server and tool in the title', () => {
    expect(approvalTitle('mcp__x__y', 'external_tool', 'x‮: y')).toBe('The agent wants to use y from x\\u{202E}');
  });
});

describe('transcriptReducer: conversation_loaded', () => {
  const loaded = (...messages: ChatMessage[]) => ({ type: 'conversation_loaded', messages }) as TranscriptAction;

  it('replaces the items, with ids from nextId, and clears busy and thinking', () => {
    const before = run([
      { type: 'user_sent', clientId: 1, text: 'live' },
      { type: 'agent_event', event: { type: 'thinking', active: true } },
    ]);
    const after = transcriptReducer(before, loaded({ role: 'user', content: 'old question' }, { role: 'assistant', content: 'old answer' }));
    expect(after.items.map((i) => i.kind)).toEqual(['user', 'agent']);
    expect(after.items[0]).toMatchObject({ text: 'old question' });
    expect(after.items[0].id).toBe(before.nextId);
    expect(after.items[1].id).toBe(before.nextId + 1);
    expect(after.nextId).toBe(before.nextId + 2);
    expect(after.inFlight).toBe(0);
    expect(after.thinking).toBe(false);
    expect(isBusy(after)).toBe(false);
  });

  it('no loaded item is streaming', () => {
    const after = transcriptReducer(EMPTY_TRANSCRIPT, loaded({ role: 'assistant', content: 'half' }));
    expect(after.items[0]).toMatchObject({ kind: 'agent', text: 'half', streaming: false });
  });

  it('a conversation within one chunk keeps the newest MAX_ITEMS, and ids keep increasing past the cap', () => {
    // 60 messages (one chunk, so not paged) of 10 tool calls each: 600 items, of which the newest 500 are kept.
    const calls: ChatMessage[] = Array.from({ length: 60 }, (_, m) => ({
      role: 'assistant',
      content: '',
      tool_calls: Array.from({ length: 10 }, (_, c) => ({ id: `m${m}c${c}`, type: 'function', function: { name: 'read_file', arguments: '{}' } })),
    }));
    const after = transcriptReducer(EMPTY_TRANSCRIPT, loaded(...calls));
    expect(after.items).toHaveLength(MAX_ITEMS);
    expect(after.items[0]).toMatchObject({ callId: 'm10c0' });
    expect(after.items[MAX_ITEMS - 1]).toMatchObject({ callId: 'm59c9' });
    const ids = after.items.map((i) => i.id);
    expect(ids).toEqual([...ids].sort((a, b) => a - b));
    expect(new Set(ids).size).toBe(ids.length);
    expect(after.nextId).toBe(EMPTY_TRANSCRIPT.nextId + 600);
    expect(after.paged).toBe(false);
  });

  it('an empty conversation empties the transcript', () => {
    const after = transcriptReducer(run([{ type: 'notice', text: 'x' }]), loaded());
    expect(after.items).toEqual([]);
  });

  it('a notice after a load gets a fresh id', () => {
    const after = run([loaded({ role: 'user', content: 'a' }), { type: 'notice', text: 'n' }]);
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

  it('todo_update adds only its set line and does not change the in-flight count', () => {
    const before = run([{ type: 'user_sent', clientId: 1, text: 'plan' }]);
    const after = transcriptReducer(before, update([todo('a', 'in_progress')]));
    expect(after.items.filter((i) => i.kind !== 'todo_set')).toEqual(before.items);
    expect(after.inFlight).toBe(before.inFlight);
  });

  it('conversation_reset clears the list', () => {
    const after = run([update([todo('a')]), { type: 'conversation_reset' }]);
    expect(after.todos).toEqual([]);
  });

  it('conversation_loaded sets the list from the loaded todos', () => {
    const after = run([
      update([todo('live')]),
      { type: 'conversation_loaded', messages: [], todos: [todo('saved', 'in_progress')] },
    ]);
    expect(after.todos).toEqual([todo('saved', 'in_progress')]);
  });

  it('conversation_loaded without todos leaves the list empty', () => {
    const after = run([update([todo('live')]), { type: 'conversation_loaded', messages: [] }]);
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

  describe('the set line', () => {
    const lines = (t: Transcript) => t.items.filter((i) => i.kind === 'todo_set');
    const last = (t: Transcript) => t.items[t.items.length - 1];

    it('the first set adds a line with the exact items', () => {
      const after = run([update([todo('a', 'in_progress'), todo('b')])]);
      expect(after.items).toEqual([{ id: 1, kind: 'todo_set', items: [todo('a', 'in_progress'), todo('b')] }]);
    });

    it('a status-only tick adds no line, but the list is still updated', () => {
      const before = run([update([todo('a', 'in_progress'), todo('b')])]);
      const after = transcriptReducer(before, update([todo('a', 'completed'), todo('b', 'in_progress')]));
      expect(lines(after)).toHaveLength(1);
      expect(after.nextId).toBe(before.nextId);
      expect(after.todos).toEqual([todo('a', 'completed'), todo('b', 'in_progress')]);
    });

    it('a renamed, reordered, added or removed item adds a line', () => {
      const base = [todo('a'), todo('b')];
      const before = run([update(base)]);
      expect(last(transcriptReducer(before, update([todo('a'), todo('c')])))).toMatchObject({ items: [todo('a'), todo('c')] });
      expect(last(transcriptReducer(before, update([todo('b'), todo('a')])))).toMatchObject({ items: [todo('b'), todo('a')] });
      expect(last(transcriptReducer(before, update([...base, todo('c')])))).toMatchObject({ items: [...base, todo('c')] });
      expect(last(transcriptReducer(before, update([todo('a')])))).toMatchObject({ items: [todo('a')] });
    });

    it('a clear adds a line with no items, and a clear on an empty list adds none', () => {
      const before = run([update([todo('a')])]);
      const after = transcriptReducer(before, update([]));
      expect(last(after)).toEqual({ id: before.nextId, kind: 'todo_set', items: [] });
      expect(after.todos).toEqual([]);
      expect(transcriptReducer(EMPTY_TRANSCRIPT, update([])).items).toEqual([]);
    });

    it('an identical resend adds no line', () => {
      const before = run([update([todo('a', 'in_progress')])]);
      const after = transcriptReducer(before, update([todo('a', 'in_progress')]));
      expect(after.items).toEqual(before.items);
    });

    it('a conversation reset makes the next set a first set again (the reset clears the items too)', () => {
      const after = run([update([todo('a')]), { type: 'conversation_reset' }, update([todo('a')])]);
      expect(lines(after)).toEqual([{ id: 2, kind: 'todo_set', items: [todo('a')] }]);
    });

    it('a session reset keeps the list, so the same list adds no line', () => {
      const after = run([update([todo('a')]), { type: 'session_reset' }, update([todo('a', 'completed')])]);
      expect(lines(after)).toHaveLength(1);
    });

    it('a set line keeps its place among the other items', () => {
      const after = run([
        { type: 'user_sent', clientId: 1, text: 'plan' },
        update([todo('a')]),
        { type: 'agent_event', event: { type: 'text', text: 'working' } },
      ]);
      expect(after.items.map((i) => i.kind)).toEqual(['user', 'todo_set', 'agent']);
    });
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

describe('transcriptReducer: the context meter and the trim note', () => {
  const usage = (over: Partial<{ trimmed: boolean; used: number; calibrated: boolean; limit: number | null }> = {}): TranscriptAction => ({
    type: 'agent_event',
    event: {
      type: 'context_usage',
      limit: over.limit === undefined ? 8000 : over.limit,
      used: over.used ?? 1234,
      reserved_output: 4096,
      trimmed: over.trimmed ?? false,
      calibrated: over.calibrated ?? false,
      categories: [{ id: 'system', tokens: 1000 }, { id: 'user', tokens: 234 }],
    },
  });
  const notices = (t: Transcript) => t.items.filter((i) => i.kind === 'notice');

  it('stores the usage as the meter value, without the event type', () => {
    const t = run([usage({ used: 1234, calibrated: true })]);
    expect(t.context).toEqual({
      limit: 8000,
      used: 1234,
      reserved_output: 4096,
      trimmed: false,
      calibrated: true,
      categories: [{ id: 'system', tokens: 1000 }, { id: 'user', tokens: 234 }],
    });
  });

  it('starts with no meter value and no note flag', () => {
    expect(EMPTY_TRANSCRIPT.context).toBeNull();
    expect(EMPTY_TRANSCRIPT.trimNoted).toBe(false);
  });

  it('an untrimmed request adds no note', () => {
    const t = run([{ type: 'user_sent', clientId: 1, text: 'hi' }, usage()]);
    expect(notices(t)).toEqual([]);
    expect(t.trimNoted).toBe(false);
  });

  it('a trimmed request adds one muted note, after the user message', () => {
    const t = run([{ type: 'user_sent', clientId: 1, text: 'hi' }, usage({ trimmed: true })]);
    expect(t.items.map((i) => i.kind)).toEqual(['user', 'notice']);
    expect(t.items[1]).toMatchObject({ kind: 'notice', text: TRIM_NOTE, muted: true });
    expect(t.trimNoted).toBe(true);
  });

  it('the second event of a trimmed request does not add a second note', () => {
    const t = run([{ type: 'user_sent', clientId: 1, text: 'hi' }, usage({ trimmed: true }), usage({ trimmed: true, calibrated: true })]);
    expect(notices(t)).toHaveLength(1);
    expect(t.context?.calibrated).toBe(true);
  });

  it('later requests of the same turn (a tool round) add no further note', () => {
    const t = run([
      { type: 'user_sent', clientId: 1, text: 'hi' },
      usage({ trimmed: true }),
      { type: 'agent_event', event: { type: 'tool_call', call_id: 'c1', name: 'get_cwd', args: {} } },
      usage({ trimmed: true }),
      usage({ trimmed: false }),
    ]);
    expect(notices(t)).toHaveLength(1);
  });

  it('a note is shown again in the next turn', () => {
    const t = run([
      { type: 'user_sent', clientId: 1, text: 'one' },
      usage({ trimmed: true }),
      done,
      { type: 'user_sent', clientId: 2, text: 'two' },
      usage({ trimmed: true }),
    ]);
    expect(notices(t)).toHaveLength(2);
  });

  it('a turn that ends with no trimmed request keeps the flag clear for the next turn', () => {
    const t = run([{ type: 'user_sent', clientId: 1, text: 'one' }, usage({ trimmed: false }), done]);
    expect(t.trimNoted).toBe(false);
  });

  it('a restored conversation takes its saved usage and adds no note', () => {
    const saved = { limit: 128000, used: 48213, reserved_output: 4096, trimmed: true, calibrated: true, categories: [{ id: 'user', tokens: 48213 }] };
    const t = run([{ type: 'conversation_loaded', messages: [{ role: 'user', content: 'x' }], context: saved }]);
    expect(t.context).toEqual(saved);
    expect(notices(t)).toHaveLength(0);
    expect(t.trimNoted).toBe(false);
  });

  it('a restored conversation with no saved usage has no meter value', () => {
    expect(run([{ type: 'conversation_loaded', messages: [], context: null }]).context).toBeNull();
    expect(run([{ type: 'conversation_loaded', messages: [] }]).context).toBeNull();
  });

  it('a reset clears the meter value and the note flag, so the next trimmed request notes again', () => {
    const t = run([
      { type: 'user_sent', clientId: 1, text: 'one' },
      usage({ trimmed: true }),
      { type: 'conversation_reset' },
    ]);
    expect(t.context).toBeNull();
    expect(t.trimNoted).toBe(false);
    const next = run([{ type: 'user_sent', clientId: 2, text: 'two' }, usage({ trimmed: true })], t);
    expect(notices(next)).toHaveLength(1);
  });

  it('a sent message clears the flag left by an earlier turn that did not end', () => {
    const t = run([{ type: 'user_sent', clientId: 1, text: 'one' }, usage({ trimmed: true }), { type: 'user_sent', clientId: 2, text: 'two' }]);
    expect(t.trimNoted).toBe(false);
  });
});

describe('transcriptReducer: reasoning', () => {
  const reasoning = (text: string, at?: number): TranscriptAction => ({ type: 'agent_event', event: { type: 'reasoning_delta', text }, at });
  const textAt = (text: string, at: number): TranscriptAction => ({ type: 'agent_event', event: { type: 'text_delta', text }, at });

  it('reasoning deltas accumulate into one open thinking block, timed from its first delta', () => {
    const t = run([reasoning('Let me ', 100), reasoning('think.', 200)]);
    expect(t.items).toHaveLength(1);
    expect(t.items[0]).toMatchObject({ kind: 'thinking', text: 'Let me think.', streaming: true, startedAt: 100 });
    expect(t.items[0]).not.toHaveProperty('endedAt');
  });

  it('leading whitespace is dropped, and whitespace alone starts no block', () => {
    expect(run([reasoning('\n\n  Hmm')]).items[0]).toMatchObject({ text: 'Hmm' });
    expect(run([reasoning(' \n ')]).items).toHaveLength(0);
  });

  it('answer text closes the block with its end time', () => {
    const t = run([reasoning('Thinking it over.', 100), textAt('Hi', 900)]);
    expect(t.items.map((i) => i.kind)).toEqual(['thinking', 'agent']);
    expect(t.items[0]).toMatchObject({ streaming: false, startedAt: 100, endedAt: 900 });
  });

  it('a tool call closes the block, and the next call reasoning opens a second one', () => {
    const t = run([reasoning('first'), toolCall('c1'), reasoning('second')]);
    expect(t.items.map((i) => i.kind)).toEqual(['thinking', 'tool', 'thinking']);
    expect(t.items[0]).toMatchObject({ text: 'first', streaming: false });
    expect(t.items[2]).toMatchObject({ text: 'second', streaming: true });
  });

  it('the end of the model call (thinking false) and done close the block', () => {
    expect(run([reasoning('a', 1), thinking(false)]).items[0]).toMatchObject({ streaming: false });
    expect(run([reasoning('a'), done]).items[0]).toMatchObject({ streaming: false });
  });

  it('thinking true does not end the block, and reasoning does not touch the thinking flag or the in-flight count', () => {
    const t = run([thinking(true), reasoning('a'), thinking(true), reasoning('b')]);
    expect(t.items).toHaveLength(1);
    expect(t.items[0]).toMatchObject({ text: 'ab', streaming: true });
    expect(t.thinking).toBe(true);
    expect(t.inFlight).toBe(0);
  });

  it('a session reset ends an open block', () => {
    expect(run([reasoning('a'), { type: 'session_reset' }]).items[0]).toMatchObject({ kind: 'thinking', streaming: false });
  });
});

describe('activityLabel', () => {
  const approval: TranscriptAction = { type: 'agent_event', event: { type: 'approval_request', call_id: 'c1', name: 'run_command', text: 'ls' } };

  it('says what the agent is doing, in order: approval, running tool, thinking, working', () => {
    expect(activityLabel(EMPTY_TRANSCRIPT)).toBe('Working');
    expect(activityLabel(run([thinking(true)]))).toBe('Thinking');
    expect(activityLabel(run([thinking(true), toolCall('c1')]))).toBe('Running get_cwd');
    expect(activityLabel(run([thinking(true), toolCall('c1'), approval]))).toBe('Waiting for your approval');
  });

  it('a finished tool is no longer running', () => {
    const t = run([toolCall('c1'), { type: 'agent_event', event: { type: 'tool_result', call_id: 'c1', name: 'get_cwd', ok: true, decision: 'read', result: {} } }]);
    expect(activityLabel(t)).toBe('Working');
  });

  it('a long tool name is cut with an ellipsis', () => {
    const name = 'x'.repeat(60);
    const t = run([{ type: 'agent_event', event: { type: 'tool_call', call_id: 'c2', name, args: {} } }]);
    const label = activityLabel(t);
    expect(label.startsWith('Running ')).toBe(true);
    expect(label.endsWith('…')).toBe(true);
    expect([...label.slice('Running '.length)]).toHaveLength(40);
  });
});

describe('transcriptReducer: reply_discarded', () => {
  const discarded: TranscriptAction = { type: 'agent_event', event: { type: 'reply_discarded' } };
  const reply = (text: string): TranscriptAction => ({ type: 'agent_event', event: { type: 'text', text } });

  it('removes the streamed text of the call on screen, adds the muted note, and clears thinking', () => {
    const t = run([thinking(true), delta('Let me '), delta('check'), discarded]);
    expect(t.items.some((i) => i.kind === 'agent')).toBe(false);
    expect(t.items[t.items.length - 1]).toMatchObject({ kind: 'notice', text: REDO_NOTE, muted: true });
    expect(t.thinking).toBe(false);
  });

  it('removes the thinking block of the call, and keeps the items from before it', () => {
    const t = run([
      reply('Earlier answer.'),
      toolCall('c1'),
      thinking(true),
      { type: 'agent_event', event: { type: 'reasoning_delta', text: 'Checking the shell' } },
      delta('Let me check'),
      discarded,
    ]);
    expect(t.items.map((i) => i.kind)).toEqual(['agent', 'tool', 'notice']);
    expect(t.items[0]).toMatchObject({ kind: 'agent', text: 'Earlier answer.' });
  });

  it('removes the call\'s thinking when the reply was not streamed', () => {
    const t = run([thinking(true), { type: 'agent_event', event: { type: 'reasoning_delta', text: 'Hmm' } }, reply('Done.'), discarded]);
    expect(t.items.map((i) => i.kind)).toEqual(['notice']);
  });

  it('a redone call shows only its own text', () => {
    const t = run([thinking(true), delta('Let me'), discarded, thinking(true), delta('Done.'), done]);
    expect(t.items.filter((i) => i.kind === 'agent').map((i) => (i.kind === 'agent' ? i.text : ''))).toEqual(['Done.']);
  });
});

describe('transcriptReducer: paged conversations', () => {
  const user = (text: string): ChatMessage => ({ role: 'user', content: text });
  const assistant = (text: string): ChatMessage => ({ role: 'assistant', content: text });
  /** `n` turns of a user message and its answer: two messages and two items each. */
  const turns = (n: number): ChatMessage[] => {
    const out: ChatMessage[] = [];
    for (let i = 0; i < n; i++) {
      out.push(user(`q${i}`), assistant(`a${i}`));
    }
    return out;
  };
  /** `n` turns of a user message and an assistant message with `calls` tool calls: one user item and `calls` tool items each. */
  const toolTurns = (n: number, calls: number): ChatMessage[] => {
    const out: ChatMessage[] = [];
    for (let t = 0; t < n; t++) {
      const tool_calls = Array.from({ length: calls }, (_, c) => ({ id: `t${t}c${c}`, type: 'function', function: { name: 'read_file', arguments: '{}' } }));
      out.push(user(`q${t}`), { role: 'assistant', content: '', tool_calls });
    }
    return out;
  };
  const load = (messages: ChatMessage[]): Transcript => transcriptReducer(EMPTY_TRANSCRIPT, { type: 'conversation_loaded', messages });
  const older = (t: Transcript): Transcript => transcriptReducer(t, { type: 'older_loaded' });
  const textsOf = (items: ReadonlyArray<{ kind: string; text?: string }>) => items.map((i) => i.text ?? i.kind);

  it('a long conversation shows only its newest chunk, and keeps the older messages raw', () => {
    const t = load(turns(100));
    expect(t.items).toHaveLength(60);
    expect(t.items[0]).toMatchObject({ kind: 'user', text: 'q70' });
    expect(t.paged).toBe(true);
    expect(hasOlder(t)).toBe(true);
    expect(t.older).toHaveLength(140);
  });

  it('a conversation within one chunk is not paged, and has nothing older', () => {
    const t = load(turns(10));
    expect(t.items).toHaveLength(20);
    expect(t.paged).toBe(false);
    expect(hasOlder(t)).toBe(false);
  });

  it('older_loaded prepends the next chunk, with ids counting down below the items already shown', () => {
    const shown = load(turns(100));
    const t = older(shown);
    expect(t.items).toHaveLength(120);
    expect(t.items[0]).toMatchObject({ kind: 'user', text: 'q40' });
    expect(t.items[0].id).toBe(-60);
    expect(t.items[59].id).toBe(-1);
    expect(t.items[60]).toMatchObject({ text: 'q70' });
    expect(t.items[60].id).toBe(shown.items[0].id);
    const ids = t.items.map((i) => i.id);
    expect(ids).toEqual([...ids].sort((a, b) => a - b));
    expect(new Set(ids).size).toBe(ids.length);
    expect(t.older).toHaveLength(80);
  });

  it('loading every older chunk shows every message once, in order, as a full conversation does', () => {
    const messages = turns(100);
    let t = load(messages);
    while (hasOlder(t)) {
      t = older(t);
    }
    expect(textsOf(t.items)).toEqual(textsOf(itemsFromMessages(messages)));
    const ids = t.items.map((i) => i.id);
    expect(ids).toEqual([...ids].sort((a, b) => a - b));
    expect(new Set(ids).size).toBe(ids.length);
    expect(t.older).toEqual([]);
    expect(t.paged).toBe(true);
  });

  it('older_loaded does nothing when nothing is older', () => {
    const t = load(turns(10));
    expect(older(t)).toBe(t);
  });

  it('a paged transcript is not trimmed from the front, and a live item is added at the end', () => {
    // 30 turns of 20 tool calls in the newest chunk: 630 items, past the cap, so the transcript is paged.
    const shown = load(toolTurns(60, 20));
    expect(shown.items.length).toBeGreaterThan(MAX_ITEMS);
    const t = transcriptReducer(shown, { type: 'notice', text: 'live' });
    expect(t.items).toHaveLength(shown.items.length + 1);
    expect(t.items[0]).toBe(shown.items[0]);
  });

  it('an unpaged transcript keeps the cap for live items', () => {
    const many = Array.from({ length: MAX_ITEMS + 5 }, (_, i) => ({ type: 'notice' as const, text: `n${i}` }));
    expect(run(many).items).toHaveLength(MAX_ITEMS);
  });

  it('reply_discarded after paging removes only the discarded call\'s items, not the older ones', () => {
    const shown = older(load(turns(100)));
    const t = run(
      [
        { type: 'agent_event', event: { type: 'thinking', active: true } },
        { type: 'agent_event', event: { type: 'text_delta', text: 'draft' } },
        { type: 'agent_event', event: { type: 'reply_discarded' } },
      ],
      shown,
    );
    expect(textsOf(t.items)).not.toContain('draft');
    expect(t.items).toHaveLength(shown.items.length + 1);
    expect(t.items[0]).toBe(shown.items[0]);
  });

  it('a reset or a new conversation clears the paging state', () => {
    const paged = load(turns(100));
    const reset = transcriptReducer(paged, { type: 'conversation_reset' });
    expect(reset).toMatchObject({ items: [], older: [], paged: false, prevId: 0 });
    const next = transcriptReducer(paged, { type: 'conversation_loaded', messages: turns(10) });
    expect(next).toMatchObject({ older: [], paged: false, prevId: 0 });
    expect(next.items).toHaveLength(20);
    // After the reset the cap applies to live items again.
    const many = Array.from({ length: MAX_ITEMS + 5 }, (_, i) => ({ type: 'notice' as const, text: `n${i}` }));
    expect(run(many, reset).items).toHaveLength(MAX_ITEMS);
  });
});

describe('transcriptReducer: replace', () => {
  it('puts a saved transcript back as it was, whatever the live one holds', () => {
    const saved = run([{ type: 'user_sent', clientId: 1, text: 'hi' }, { type: 'agent_event', event: { type: 'text', text: 'hello' } }]);
    const live = run([{ type: 'notice', text: 'other chat' }, { type: 'user_sent', clientId: 2, text: 'x' }]);
    expect(transcriptReducer(live, { type: 'replace', transcript: saved })).toBe(saved);
    expect(isBusy(transcriptReducer(live, { type: 'replace', transcript: EMPTY_TRANSCRIPT }))).toBe(false);
  });
});

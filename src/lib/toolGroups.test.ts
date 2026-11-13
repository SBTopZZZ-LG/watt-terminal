import { describe, expect, it } from 'vitest';
import { groupTranscript, toolGroupSummary } from './toolGroups';
import type { GroupItem } from './toolGroups';
import { EMPTY_TRANSCRIPT, transcriptReducer } from './transcript';
import type { TranscriptAction, TranscriptItem } from './transcript';

function items(actions: TranscriptAction[]): TranscriptItem[] {
  return actions.reduce(transcriptReducer, EMPTY_TRANSCRIPT).items;
}

const call = (callId: string, name = 'get_cwd'): TranscriptAction => ({
  type: 'agent_event',
  event: { type: 'tool_call', call_id: callId, name, args: {} },
});

const callArgs = (callId: string, name: string, args: unknown): TranscriptAction => ({
  type: 'agent_event',
  event: { type: 'tool_call', call_id: callId, name, args },
});

const result = (
  callId: string,
  name: string,
  ok: boolean,
  decision: 'read' | 'manual' | 'denied' | 'refused' = 'read',
): TranscriptAction => ({
  type: 'agent_event',
  event: { type: 'tool_result', call_id: callId, name, ok, decision, result: {} },
});

/** A call that has finished successfully. */
const finished = (callId: string, name = 'get_cwd'): TranscriptAction[] => [call(callId, name), result(callId, name, true)];

const approval = (callId: string, name = 'run_command'): TranscriptAction => ({
  type: 'agent_event',
  event: { type: 'approval_request', call_id: callId, name, text: 'ls' },
});

const text = (t: string): TranscriptAction => ({ type: 'agent_event', event: { type: 'text', text: t } });

/** The shape of a grouped list: an item's kind, or a group as the ids of its members. */
function shape(entries: ReturnType<typeof groupTranscript>): unknown[] {
  return entries.map((e) => {
    if (e.kind === 'item') {
      return e.item.kind;
    }
    if (e.kind === 'interactive') {
      return `interactive:${e.group.recordId}`;
    }
    return e.items.map((i) => (i.kind === 'tool' ? i.callId : `${i.kind}:${i.callId}`));
  });
}

describe('groupTranscript', () => {
  it('an empty list has no entries', () => {
    expect(groupTranscript([])).toEqual([]);
  });

  it('three tool calls in a row become one group', () => {
    const entries = groupTranscript(items([call('c1'), call('c2'), call('c3')]));
    expect(entries).toHaveLength(1);
    expect(entries[0].kind).toBe('tools');
    expect(shape(entries)).toEqual([['c1', 'c2', 'c3']]);
  });

  it('a single tool call renders exactly as an individual item', () => {
    const list = items([call('c1')]);
    expect(groupTranscript(list)).toEqual([{ kind: 'item', item: list[0] }]);
  });

  it('a single tool call with a resolved approval collapses into a group', () => {
    const entries = groupTranscript(items([call('c1', 'run_command'), approval('c1'), result('c1', 'run_command', true, 'manual')]));
    expect(shape(entries)).toEqual([['c1', 'approval:c1']]);
  });

  it('a single tool call with a pending approval is not grouped: the approval is its own card', () => {
    const entries = groupTranscript(items([call('c1', 'run_command'), approval('c1')]));
    expect(shape(entries)).toEqual(['tool', 'approval']);
    expect(entries[1]).toMatchObject({ kind: 'item', item: { kind: 'approval', state: 'pending' } });
  });

  it('a resolved approval with no tool call in its run renders as an individual item', () => {
    // A new shell closes the pending approval (it becomes resolved) and adds a notice after it.
    const entries = groupTranscript(items([approval('a1'), { type: 'session_reset' }]));
    expect(shape(entries)).toEqual(['approval', 'notice']);
    expect(entries[0]).toMatchObject({ kind: 'item', item: { kind: 'approval', state: 'resolved' } });
  });

  it('a pending approval is never inside a group: it ends the run, and later tools start a new one', () => {
    const entries = groupTranscript(items([call('c1'), call('c2'), approval('a1'), call('c3'), call('c4')]));
    expect(shape(entries)).toEqual([['c1', 'c2'], 'approval', ['c3', 'c4']]);
    expect(entries[1]).toMatchObject({ kind: 'item', item: { kind: 'approval', state: 'pending' } });
  });

  it('a pending approval between two single tools leaves both tools as individual items', () => {
    const entries = groupTranscript(items([call('c1'), approval('a1'), call('c2')]));
    expect(shape(entries)).toEqual(['tool', 'approval', 'tool']);
  });

  it('a resolved approval stays inside the group', () => {
    const entries = groupTranscript(items([call('c1'), approval('c1'), result('c1', 'run_command', true, 'manual'), call('c2')]));
    expect(shape(entries)).toEqual([['c1', 'approval:c1', 'c2']]);
    const [group] = entries;
    expect(group.kind === 'tools' && group.items[1]).toMatchObject({ kind: 'approval', state: 'resolved' });
  });

  it('agent text between two runs splits them into two groups', () => {
    const entries = groupTranscript(items([call('c1'), call('c2'), text('between'), call('c3'), call('c4')]));
    expect(shape(entries)).toEqual([['c1', 'c2'], 'agent', ['c3', 'c4']]);
  });

  it('a user message or a notice also ends a run', () => {
    const withUser = groupTranscript(
      items([call('c1'), call('c2'), { type: 'user_sent', clientId: 1, text: 'next' }, call('c3'), call('c4')]),
    );
    expect(shape(withUser)).toEqual([['c1', 'c2'], 'user', ['c3', 'c4']]);

    const withNotice = groupTranscript(items([call('c1'), call('c2'), { type: 'notice', text: 'n' }, call('c3')]));
    expect(shape(withNotice)).toEqual([['c1', 'c2'], 'notice', 'tool']);
  });

  it('a single tool call between two text items stays a plain item', () => {
    const entries = groupTranscript(items([text('a'), call('c1'), text('b')]));
    expect(shape(entries)).toEqual(['agent', 'tool', 'agent']);
  });

  it('the key is the id of the first item of the run, and it stays the same as the run grows', () => {
    const two = items([call('c1'), call('c2')]);
    const [first] = groupTranscript(two);
    expect(first.kind === 'tools' && first.key).toBe(two[0].id);

    const three = items([call('c1'), call('c2'), call('c3')]);
    const [grown] = groupTranscript(three);
    expect(grown.kind === 'tools' && grown.key).toBe(two[0].id);
    expect(grown.kind === 'tools' && grown.items).toHaveLength(3);
  });
});

describe('toolGroupSummary', () => {
  function group(actions: TranscriptAction[]): GroupItem[] {
    const [entry] = groupTranscript(items(actions));
    if (entry.kind !== 'tools') {
      throw new Error('expected a group');
    }
    return entry.items;
  }

  it('a single tool call is counted in the singular', () => {
    const g = group([call('c1', 'run_command'), approval('c1'), result('c1', 'run_command', true, 'manual')]);
    expect(toolGroupSummary(g)).toBe('1 tool call: run_command');
  });

  it('a single tool call that was denied keeps the denied note', () => {
    const g = group([call('c1', 'run_command'), approval('c1'), result('c1', 'run_command', false, 'denied')]);
    expect(toolGroupSummary(g)).toBe('1 tool call: run_command · 1 failed · denied');
  });

  it('shows the count and the tool names with repeat counts', () => {
    const g = group([
      ...finished('c1', 'get_cwd'),
      ...finished('c2', 'get_command'),
      ...finished('c3', 'get_command'),
      ...finished('c4', 'run_command'),
    ]);
    expect(toolGroupSummary(g)).toBe('4 tool calls: get_cwd, get_command ×2, run_command');
  });

  it('a call still running adds the name of the last running one', () => {
    const g = group([
      ...finished('c1', 'get_cwd'),
      call('c2', 'run_command'),
      call('c3', 'get_screen'),
    ]);
    expect(toolGroupSummary(g)).toBe('3 tool calls: get_cwd, run_command, get_screen · running get_screen');
  });

  it('failed calls add a count of failures', () => {
    const g = group([
      call('c1'),
      result('c1', 'get_cwd', false),
      call('c2'),
      result('c2', 'get_cwd', false),
      ...finished('c3'),
    ]);
    expect(toolGroupSummary(g)).toBe('3 tool calls: get_cwd ×3 · 2 failed');
  });

  it('a denied decision adds denied', () => {
    const g = group([call('c1', 'run_command'), result('c1', 'run_command', false, 'denied'), ...finished('c2')]);
    expect(toolGroupSummary(g)).toBe('2 tool calls: run_command, get_cwd · 1 failed · denied');
  });

  it('running, failed and denied come in that order', () => {
    const g = group([
      call('c1', 'run_command'),
      result('c1', 'run_command', false, 'denied'),
      call('c2', 'get_cwd'),
      result('c2', 'get_cwd', false),
      call('c3', 'run_command'),
    ]);
    expect(toolGroupSummary(g)).toBe('3 tool calls: run_command ×2, get_cwd · running run_command · 2 failed · denied');
  });

  it('escapes a tool name the model made up', () => {
    const g = group([...finished('c1', 'x‮y'), ...finished('c2', 'x‮y')]);
    expect(toolGroupSummary(g)).toBe('2 tool calls: x\\u{202E}y ×2');
  });

  it('appends the first caption after the tool names', () => {
    const g = group([
      callArgs('c1', 'run_command', { command: 'git status', summary: 'Check the working tree' }),
      result('c1', 'run_command', true),
      ...finished('c2', 'get_cwd'),
    ]);
    expect(toolGroupSummary(g)).toBe('2 tool calls: run_command, get_cwd · Check the working tree');
  });

  it('marks hidden captions with an ellipsis, before the running note', () => {
    const g = group([
      callArgs('c1', 'run_command', { command: 'git status', summary: 'Check the working tree' }),
      result('c1', 'run_command', true),
      callArgs('c2', 'run_command', { command: 'git push', summary: 'Push the branch' }),
    ]);
    expect(toolGroupSummary(g)).toBe('2 tool calls: run_command ×2 · Check the working tree … · running run_command');
  });

  it('a group without captions is unchanged', () => {
    const g = group([...finished('c1', 'get_cwd'), ...finished('c2', 'get_cwd')]);
    expect(toolGroupSummary(g)).toBe('2 tool calls: get_cwd ×2');
  });
});

describe('toolGroupSummary: declined integration requests', () => {
  function group(actions: TranscriptAction[]): GroupItem[] {
    const [entry] = groupTranscript(items(actions));
    if (entry.kind !== 'tools') {
      throw new Error('expected a group');
    }
    return entry.items;
  }

  it('a declined request_integration is counted as declined, not as failed or denied', () => {
    const g = group([
      call('c1', 'request_integration'),
      result('c1', 'request_integration', false, 'denied'),
      call('c2', 'get_screen'),
      result('c2', 'get_screen', true, 'read'),
    ]);
    expect(toolGroupSummary(g)).toBe('2 tool calls: request_integration, get_screen · declined');
  });

  it('a denied run_command still says denied, and a failed run still counts', () => {
    const g = group([
      call('c1', 'request_integration'),
      result('c1', 'request_integration', false, 'denied'),
      call('c2', 'run_command'),
      result('c2', 'run_command', false, 'denied'),
    ]);
    expect(toolGroupSummary(g)).toBe('2 tool calls: request_integration, run_command · 1 failed · denied · declined');
  });
});

describe('groupTranscript: interactive groups', () => {
  const runningCommand = (callId: string, recordId: number, command = 'ssh ubuntu@lidia'): TranscriptAction[] => [
    { type: 'agent_event', event: { type: 'tool_call', call_id: callId, name: 'run_command', args: { command } } },
    {
      type: 'agent_event',
      event: { type: 'tool_result', call_id: callId, name: 'run_command', ok: true, decision: 'auto', result: { outcome: 'running', record_id: recordId } },
    },
  ];
  const waitRow = (callId: string): TranscriptAction[] => [
    { type: 'agent_event', event: { type: 'tool_call', call_id: callId, name: 'wait', args: {} } },
    { type: 'agent_event', event: { type: 'tool_result', call_id: callId, name: 'wait', ok: true, decision: 'read', result: { reason: 'screen_changed' } } },
  ];
  const recordEnd = (id: number, exit: number): TranscriptAction => ({
    type: 'record_finished',
    record: { id, command: 'x', source: { kind: 'agent', call_id: 'run' }, exit_code: exit, status: 'complete', used_alt_screen: false, output_bytes: 0 },
  });

  it('the run_command row becomes the group header, not a row inside it; its later rows are nested', () => {
    const entries = groupTranscript(items([...runningCommand('c1', 7), ...waitRow('w1'), ...waitRow('w2')]));
    expect(entries).toHaveLength(1);
    const [entry] = entries;
    expect(entry.kind).toBe('interactive');
    if (entry.kind !== 'interactive') {
      return;
    }
    expect(entry.group).toMatchObject({ recordId: 7, command: 'ssh ubuntu@lidia', ended: null });
    // Two wait rows in a row collapse into one group inside the wrapper, as they do in the transcript.
    expect(entry.group.entries.map((e) => e.kind)).toEqual(['tools']);
  });

  it('consecutive rows inside the group collapse the same way the transcript collapses them', () => {
    const entries = groupTranscript(items([...runningCommand('c1', 7), ...waitRow('w1'), ...waitRow('w2'), recordEnd(7, 0)]));
    const [entry] = entries;
    expect(entry.kind === 'interactive' && entry.group.ended).toEqual({ exit: 0 });
    expect(entry.kind === 'interactive' && entry.group.entries.map((e) => e.kind)).toEqual(['tools']);
  });

  it('the items after the end are outside the group', () => {
    const entries = groupTranscript(items([...runningCommand('c1', 7), recordEnd(7, 0), { type: 'agent_event', event: { type: 'text', text: 'done' } }]));
    expect(entries.map((e) => e.kind)).toEqual(['interactive', 'item']);
    expect(entries[1].kind === 'item' && entries[1].item.kind).toBe('agent');
  });

  it('the group key is the id of the opening row, and it stays the same as the group grows', () => {
    const before = groupTranscript(items(runningCommand('c1', 7)));
    const after = groupTranscript(items([...runningCommand('c1', 7), ...waitRow('w1')]));
    expect(before[0].kind === 'interactive' && before[0].key).toBe(after[0].kind === 'interactive' && after[0].key);
  });

  it('a group with nothing in it still shows its header', () => {
    const [entry] = groupTranscript(items(runningCommand('c1', 7)));
    expect(entry.kind === 'interactive' && entry.group.entries).toEqual([]);
  });
});

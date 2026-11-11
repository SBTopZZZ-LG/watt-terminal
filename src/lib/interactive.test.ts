import { describe, expect, it } from 'vitest';
import { buildFrames, endedBy, formatElapsed, interactiveHeader, openedBy, openRecordIds } from './interactive';
import { EMPTY_TRANSCRIPT, transcriptReducer } from './transcript';
import type { Transcript, TranscriptAction, TranscriptItem } from './transcript';
import type { RecordSummary } from './types';

const T0 = 1_000_000;

const call = (callId: string, name: string, args: unknown, at = T0): TranscriptAction => ({
  type: 'agent_event',
  event: { type: 'tool_call', call_id: callId, name, args },
  at,
});
const result = (callId: string, name: string, res: unknown, decision: 'auto' | 'manual' | 'read' = 'auto'): TranscriptAction => ({
  type: 'agent_event',
  event: { type: 'tool_result', call_id: callId, name, ok: true, decision, result: res },
});
const text = (t: string): TranscriptAction => ({ type: 'agent_event', event: { type: 'text', text: t } });
const done: TranscriptAction = { type: 'agent_event', event: { type: 'done' } };
const approval = (callId: string, name: string, reason?: 'unintegrated_shell'): TranscriptAction => ({
  type: 'agent_event',
  event: { type: 'approval_request', call_id: callId, name, text: 'echo hi⏎', display: 'echo hi⏎', reason, level: 0 },
});
const session: TranscriptAction = { type: 'session_reset' };

function record(id: number, exit: number | null, source: RecordSummary['source'] = { kind: 'agent', call_id: 'run' }): RecordSummary {
  return { id, command: 'cmd', source, exit_code: exit, status: 'complete', used_alt_screen: false, output_bytes: 0 };
}
const finished = (id: number, exit: number | null = 0, source?: RecordSummary['source']): TranscriptAction => ({
  type: 'record_finished',
  record: record(id, exit, source),
});

/** The run_command call that leaves its command running (outcome 'running'), as the backend returns it. */
function runCommand(callId: string, recordId: number, command: string, at = T0): TranscriptAction[] {
  return [
    call(callId, 'run_command', { command }, at),
    result(callId, 'run_command', { outcome: 'running', reason: 'timeout', record_id: recordId, call_id: callId, state: 'running', waited_ms: 5000, screen_tail: [], hint: 'wait' }),
  ];
}

const run = (actions: TranscriptAction[], start: Transcript = EMPTY_TRANSCRIPT): Transcript => actions.reduce(transcriptReducer, start);

/** The shell levels now, the local one included (the levels event's count). */
const levels = (depth: number): TranscriptAction => ({ type: 'levels_changed', depth });

/** The top-level group's frame of the transcript, by its record id. */
function frameOf(items: TranscriptItem[], recordId: number) {
  const found = buildFrames(items).children.find((c) => c.kind === 'frame' && c.frame.recordId === recordId);
  return found !== undefined && found.kind === 'frame' ? found.frame : undefined;
}

describe('openedBy and endedBy', () => {
  const item = (over: Partial<Extract<TranscriptItem, { kind: 'tool' }>>): TranscriptItem => ({
    id: 1,
    kind: 'tool',
    callId: 'c',
    name: 'run_command',
    args: { command: 'ssh host' },
    status: 'ok',
    result: null,
    decision: 'auto',
    reason: 'rule',
    ...over,
  });

  it('a running run_command result with a record id opens a group on that record', () => {
    expect(openedBy(item({ result: { outcome: 'running', record_id: 7 } }))).toEqual({ recordId: 7, command: 'ssh host' });
  });

  it('a finished result, a result with no record id yet, and a reopened text result open nothing', () => {
    expect(openedBy(item({ result: { outcome: 'finished', record: record(7, 0) } }))).toBeNull();
    expect(openedBy(item({ result: { outcome: 'running', record_id: null } }))).toBeNull();
    expect(openedBy(item({ result: '{"outcome":"running","record_id":7}' }))).toBeNull();
  });

  it('only run_command opens a group', () => {
    expect(openedBy(item({ name: 'wait', result: { outcome: 'running', record_id: 7 } }))).toBeNull();
  });

  it('a record_end item ends its record, and a tool result with a record ends that record with its exit code', () => {
    expect(endedBy({ id: 2, kind: 'record_end', recordId: 7, exit: 3 })).toEqual({ recordId: 7, exit: 3 });
    expect(endedBy(item({ name: 'wait', result: { reason: 'command_finished', record: record(7, 2) } }))).toEqual({ recordId: 7, exit: 2 });
    expect(endedBy(item({ name: 'wait', result: { reason: 'timeout' } }))).toBeNull();
  });
});

describe('openRecordIds: opening and ending', () => {
  it('a running run_command opens a group that is open until its command ends', () => {
    const t = run(runCommand('c1', 7, 'ssh ubuntu@lidia'));
    expect([...openRecordIds(t.items)]).toEqual([7]);
    const ended = run([finished(7, 0)], t);
    expect(openRecordIds(ended.items).size).toBe(0);
    expect(ended.items[ended.items.length - 1]).toMatchObject({ kind: 'record_end', recordId: 7, exit: 0 });
  });

  it('a record_finished for a command with no open group adds no item', () => {
    const t = run([call('c1', 'run_command', { command: 'ls' }), result('c1', 'run_command', { outcome: 'finished', record: record(3, 0) })]);
    const after = run([finished(3, 0)], t);
    expect(after.items).toBe(t.items);
    expect(openRecordIds(after.items).size).toBe(0);
  });

  it('a user command finishing never opens or closes a group', () => {
    const t = run([...runCommand('c1', 7, 'ssh host'), finished(9, 0, { kind: 'user' })]);
    expect([...openRecordIds(t.items)]).toEqual([7]);
  });

  it('a result that names the record ends the group, with the exit code, and the row belongs to the group', () => {
    const t = run([
      ...runCommand('c1', 7, 'ssh host'),
      call('w1', 'wait', { id: 7 }),
      result('w1', 'wait', { reason: 'command_finished', record: record(7, 2) }),
    ]);
    expect(openRecordIds(t.items).size).toBe(0);
    const { children, open } = buildFrames(t.items);
    expect(open).toEqual([]);
    expect(children).toHaveLength(1);
    const frame = children[0];
    expect(frame.kind).toBe('frame');
    if (frame.kind !== 'frame') {
      return;
    }
    expect(frame.frame.ended).toEqual({ exit: 2 });
    expect(frame.frame.children.map((c) => (c.kind === 'item' && c.item.kind === 'tool' ? c.item.name : c.kind))).toEqual(['wait']);
  });

  it('the group spans turns: a turn ending does not close it', () => {
    const t = run([...runCommand('c1', 7, 'ssh host'), text('the shell is open'), done, { type: 'user_sent', clientId: 2, text: 'next' }, done]);
    expect([...openRecordIds(t.items)]).toEqual([7]);
  });

  it('a session reset ends every open group, with no exit code', () => {
    const t = run([...runCommand('c1', 7, 'ssh host'), session]);
    expect(openRecordIds(t.items).size).toBe(0);
    expect(t.items.some((i) => i.kind === 'record_end' && i.recordId === 7 && i.exit === null)).toBe(true);
  });

  it('a command that finished before its run_command result arrived opens closed, with its exit code', () => {
    // The command finished while the call was still returning: record_finished comes first, then the running result.
    const early = run([finished(7, 4)]);
    const t = run(runCommand('c1', 7, 'make'), early);
    expect(openRecordIds(t.items).size).toBe(0);
    const frame = buildFrames(t.items).children[0];
    expect(frame.kind === 'frame' && frame.frame.ended).toEqual({ exit: 4 });
  });

  it('a result that opens after its command finished, with nothing in between, leaves no open group', () => {
    const t = run([finished(7, null), ...runCommand('c1', 7, 'make')]);
    expect(openRecordIds(t.items).size).toBe(0);
  });

  it('a late-opening command closes at its own row, so the later items are not inside it', () => {
    const early = run([finished(7, 0)]);
    const t = run([...runCommand('c1', 7, 'make'), text('after')], early);
    const { children } = buildFrames(t.items);
    expect(children.map((c) => c.kind)).toEqual(['frame', 'item']);
  });
});

describe('openRecordIds: nesting', () => {
  it('a second command started while one is open is nested in it, and both are open', () => {
    const t = run([...runCommand('c1', 7, 'ssh ubuntu@lidia'), ...runCommand('c2', 9, 'sleep 60')]);
    expect([...openRecordIds(t.items)].sort()).toEqual([7, 9]);
    const outer = buildFrames(t.items).children[0];
    expect(outer.kind).toBe('frame');
    if (outer.kind !== 'frame') {
      return;
    }
    expect(outer.frame.children.map((c) => (c.kind === 'frame' ? c.frame.recordId : 'item'))).toEqual([9]);
  });

  it('items go to the latest group still open: after the inner command ends they go back to the outer one', () => {
    const t = run([
      ...runCommand('c1', 7, 'ssh host'),
      ...runCommand('c2', 9, 'sleep 60'),
      finished(9, 0),
      text('back in the shell'),
    ]);
    const outer = buildFrames(t.items).children[0];
    expect(outer.kind === 'frame' && outer.frame.children.map((c) => c.kind)).toEqual(['frame', 'item']);
    expect([...openRecordIds(t.items)]).toEqual([7]);
  });

  it('when the outer command ends first, the inner one stays open', () => {
    const t = run([...runCommand('c1', 7, 'ssh host'), ...runCommand('c2', 9, 'sleep 60'), finished(7, 0)]);
    expect([...openRecordIds(t.items)]).toEqual([9]);
  });
});

describe('a level pushed above a group ends it as integrated', () => {
  it('the group records the shell depth its running result arrived at, and the transcript keeps the depth', () => {
    expect(run(runCommand('c1', 7, 'ssh host')).depth).toBe(1);
    const t = run([levels(2), ...runCommand('c1', 7, 'ssh host')]);
    expect(t.depth).toBe(2);
    expect(frameOf(t.items, 7)?.levelDepth).toBe(2);
    expect(frameOf(run(runCommand('c1', 7, 'ssh host')).items, 7)?.levelDepth).toBe(1);
  });

  it('a level pushed above the command ends the group with no exit code, and says integrated', () => {
    const t = run([...runCommand('c1', 7, 'ssh ubuntu@lidia'), levels(2)]);
    expect(openRecordIds(t.items).size).toBe(0);
    expect(t.items[t.items.length - 1]).toMatchObject({ kind: 'record_end', recordId: 7, exit: null, integrated: true });
    const frame = frameOf(t.items, 7);
    expect(frame?.ended).toEqual({ exit: null, integrated: true });
    expect(frame && interactiveHeader(frame, T0 + 9_000)).toBe('Interactive · ssh ubuntu@lidia · integrated');
  });

  it('a record_finished after the integration adds no item: the command ends later, the group does not', () => {
    const t = run([...runCommand('c1', 7, 'ssh host'), levels(2)]);
    const after = run([finished(7, 0)], t);
    expect(after.items).toBe(t.items);
    expect(frameOf(after.items, 7)?.ended).toEqual({ exit: null, integrated: true });
  });

  it('the agent commands after the push are outside the group, and their results do not reopen it', () => {
    const t = run([
      ...runCommand('c1', 7, 'ssh ubuntu@lidia'),
      call('g1', 'get_screen', {}),
      result('g1', 'get_screen', {}),
      levels(2),
      call('a1', 'run_command', { command: 'sudo apt update' }),
      result('a1', 'run_command', { outcome: 'finished', record: record(8, 0) }),
    ]);
    expect(openRecordIds(t.items).size).toBe(0);
    const { children } = buildFrames(t.items);
    expect(children.map((c) => c.kind)).toEqual(['frame', 'item']);
    const frame = children[0];
    expect(frame.kind === 'frame' && frame.frame.children.map((c) => (c.kind === 'item' && c.item.kind === 'tool' ? c.item.name : c.kind))).toEqual(['get_screen']);
    expect(children[1].kind === 'item' && children[1].item.kind === 'tool' && children[1].item.name).toBe('run_command');
  });

  it('the push ends it whichever row came first: a request_integration row alone does not end it, the levels change does', () => {
    const t = run([
      ...runCommand('c1', 7, 'ssh host'),
      call('ri1', 'request_integration', {}),
      result('ri1', 'request_integration', { outcome: 'integrated', label: 'ssh host' }),
    ]);
    expect([...openRecordIds(t.items)]).toEqual([7]);
    expect(openRecordIds(run([levels(2)], t).items).size).toBe(0);
  });

  it('a manual Integrate pushes a level too: the same levels change ends the group, with no request row at all', () => {
    const t = run([...runCommand('c1', 7, 'ssh host'), levels(2)]);
    expect(frameOf(t.items, 7)?.ended).toEqual({ exit: null, integrated: true });
  });

  it('a group opened at the depth the levels already are is not ended by a levels change to that depth, but a deeper one ends it', () => {
    const t = run([levels(2), ...runCommand('c1', 7, 'ssh host')]);
    expect([...openRecordIds(run([levels(2)], t).items)]).toEqual([7]);
    expect([...openRecordIds(run([levels(3)], t).items)]).toEqual([]);
  });

  it('a level popped (a shallower depth) ends nothing: the command the group is for may still run', () => {
    const t = run([levels(2), ...runCommand('c1', 7, 'ssh host'), levels(1)]);
    expect(t.depth).toBe(1);
    expect([...openRecordIds(t.items)]).toEqual([7]);
  });

  it('a deeper-then-back sequence: the push ends the group once, the pop after it ends nothing, a new group at the shallow depth ends at the next push', () => {
    const t = run([
      ...runCommand('c1', 7, 'ssh host'),
      levels(2),
      levels(1),
      ...runCommand('c2', 9, 'make'),
      levels(2),
    ]);
    expect(openRecordIds(t.items).size).toBe(0);
    expect(t.items.filter((i) => i.kind === 'record_end' && i.recordId === 7)).toHaveLength(1);
    expect(frameOf(t.items, 9)?.ended).toEqual({ exit: null, integrated: true });
  });

  it('a long-running command in the nested shell opens its own group after the push, and its record_finished closes it', () => {
    const t = run([...runCommand('c1', 7, 'ssh ubuntu@lidia'), levels(2), ...runCommand('c2', 9, 'sleep 60')]);
    expect([...openRecordIds(t.items)]).toEqual([9]);
    const { children } = buildFrames(t.items);
    expect(children.map((c) => (c.kind === 'frame' ? c.frame.recordId : 'item'))).toEqual([7, 9]);
    const closed = run([finished(9, 0)], t);
    expect(openRecordIds(closed.items).size).toBe(0);
    const inner = frameOf(closed.items, 9);
    expect(inner && interactiveHeader(inner, T0 + 5_000)).toBe('Interactive · sleep 60 · finished · exit 0');
    expect(frameOf(closed.items, 7)?.ended).toEqual({ exit: null, integrated: true });
  });
});

describe('the approval and the other steps inside a group', () => {
  it('the approval of the run_command call that opened the group stays above it, not inside', () => {
    const t = run([
      call('c1', 'run_command', { command: 'ssh host' }),
      approval('c1', 'run_command'),
      result('c1', 'run_command', { outcome: 'running', record_id: 7 }),
      call('g1', 'get_screen', {}),
      result('g1', 'get_screen', {}),
    ]);
    const { children } = buildFrames(t.items);
    expect(children.map((c) => (c.kind === 'frame' ? 'frame' : c.item.kind))).toEqual(['approval', 'frame']);
    const frame = children[1];
    expect(frame.kind === 'frame' && frame.frame.children.map((c) => (c.kind === 'item' ? c.item.kind : 'frame'))).toEqual(['tool']);
  });

  it('an approval inside an open group keeps the group open', () => {
    const t = run([...runCommand('c1', 7, 'ssh host'), approval('s1', 'send_keys', 'unintegrated_shell')]);
    expect([...openRecordIds(t.items)]).toEqual([7]);
  });

  it('an approval card is not a tool row, so it does not end the group', () => {
    const t = run([...runCommand('c1', 7, 'ssh host'), approval('s1', 'send_keys')]);
    expect(openRecordIds(t.items).has(7)).toBe(true);
  });
});

describe('formatElapsed', () => {
  it('shows seconds, then minutes and seconds, then hours and minutes', () => {
    expect(formatElapsed(0)).toBe('0s');
    expect(formatElapsed(42_900)).toBe('42s');
    expect(formatElapsed(65_000)).toBe('1m 05s');
    expect(formatElapsed(3 * 60_000)).toBe('3m 00s');
    expect(formatElapsed(3_600_000 + 7 * 60_000)).toBe('1h 07m');
  });

  it('reads a negative span as zero', () => {
    expect(formatElapsed(-5_000)).toBe('0s');
  });
});

describe('interactiveHeader', () => {
  it('is running with the elapsed time while the command runs', () => {
    expect(interactiveHeader({ command: 'ssh ubuntu@lidia', startedAt: T0, ended: null }, T0 + 185_000)).toBe(
      'Interactive · ssh ubuntu@lidia · running 3m 05s',
    );
  });

  it('says finished with the exit code, or ended when there is none', () => {
    expect(interactiveHeader({ command: 'ssh host', startedAt: T0, ended: { exit: 0 } }, T0 + 9_000)).toBe(
      'Interactive · ssh host · finished · exit 0',
    );
    expect(interactiveHeader({ command: 'ssh host', startedAt: T0, ended: { exit: null } }, T0 + 9_000)).toBe('Interactive · ssh host · ended');
  });

  it('caps a long command and shows a newline as \\n, so the header stays on one line', () => {
    const header = interactiveHeader({ command: `a\n${'x'.repeat(200)}`, startedAt: null, ended: null }, T0);
    expect(header).toMatch(/^Interactive · a\\nx+… · running$/);
    expect(header.length).toBeLessThan(120);
  });

  it('says integrated when a shell level was pushed above the command, and not finished', () => {
    expect(interactiveHeader({ command: 'ssh ubuntu@lidia', startedAt: T0, ended: { exit: null, integrated: true } }, T0 + 9_000)).toBe(
      'Interactive · ssh ubuntu@lidia · integrated',
    );
  });

  it('shows running without a time when the start is not known', () => {
    expect(interactiveHeader({ command: 'top', startedAt: null, ended: null }, T0)).toBe('Interactive · top · running');
  });
});

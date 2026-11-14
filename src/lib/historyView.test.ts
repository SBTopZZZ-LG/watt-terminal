import { describe, expect, it } from 'vitest';
import {
  COMPACT_SUMMARY_PREFIX,
  DENIED_PREFIX,
  filterMetas,
  formatWhen,
  groupByDate,
  groupByWorkspace,
  itemsFromMessages,
  NO_RESULT_TEXT,
  relativeTime,
  safeText,
  UNANSWERED_NOTE,
  UNKNOWN_DATE,
} from './historyView';
import type { ChatMessage, ConversationMeta, Workspace } from './types';

/** Local time, so these tests pass in any time zone. Months are 0-based, as in Date. */
const local = (y: number, mo: number, d: number, h = 0, mi = 0) => Math.floor(new Date(y, mo, d, h, mi, 0).getTime() / 1000);

const ROOT_A: Workspace = { host: 'mac', root: '/Users/me/alpha', label: 'alpha' };
const ROOT_B: Workspace = { host: 'mac', root: '/Users/me/beta', label: 'beta' };
const UNKNOWN: Workspace = { host: '', root: '', label: 'Unknown' };

function meta(id: string, updated: number, workspace: Workspace = ROOT_A, title = `title ${id}`, messages = 2): ConversationMeta {
  return { id, title, created_at: updated, updated_at: updated, message_count: messages, workspace, model: null };
}

describe('itemsFromMessages', () => {
  it('turns user text and assistant text into user and agent items, in order', () => {
    const items = itemsFromMessages([
      { role: 'user', content: 'what is in /tmp' },
      { role: 'assistant', content: 'Let me look.' },
    ]);
    expect(items).toEqual([
      { kind: 'user', clientId: 0, text: 'what is in /tmp' },
      { kind: 'agent', text: 'Let me look.', streaming: false },
    ]);
  });

  it('a compacted summary seed is a muted notice, not a user bubble', () => {
    const summary = `${COMPACT_SUMMARY_PREFIX}\n\nEarlier the user asked about retries.`;
    const items = itemsFromMessages([
      { role: 'user', content: summary },
      { role: 'assistant', content: 'Got it.' },
    ]);
    expect(items).toEqual([
      { kind: 'notice', text: summary, muted: true },
      { kind: 'agent', text: 'Got it.', streaming: false },
    ]);
  });

  it('joins a tool call with its tool message and parses the arguments', () => {
    const messages: ChatMessage[] = [
      { role: 'user', content: 'ls' },
      {
        role: 'assistant',
        content: null,
        tool_calls: [{ id: 'c1', type: 'function', function: { name: 'run_command', arguments: '{"command":"ls"}' } }],
      },
      { role: 'tool', tool_call_id: 'c1', content: 'a.txt\nb.txt' },
    ];
    const items = itemsFromMessages(messages);
    expect(items).toHaveLength(2);
    expect(items[1]).toEqual({
      kind: 'tool',
      callId: 'c1',
      name: 'run_command',
      args: { command: 'ls' },
      status: 'ok',
      result: 'a.txt\nb.txt',
      decision: null,
      reason: null,
    });
  });

  it('an assistant message with text and calls gives the agent item first, then the tool items', () => {
    const items = itemsFromMessages([
      {
        role: 'assistant',
        content: 'Checking.',
        tool_calls: [
          { id: 'c1', type: 'function', function: { name: 'get_cwd', arguments: '{}' } },
          { id: 'c2', type: 'function', function: { name: 'get_screen', arguments: '{}' } },
        ],
      },
      { role: 'tool', tool_call_id: 'c1', content: '/tmp' },
      { role: 'tool', tool_call_id: 'c2', content: 'screen' },
    ]);
    expect(items.map((i) => i.kind)).toEqual(['agent', 'tool', 'tool']);
    expect(items[1]).toMatchObject({ callId: 'c1', status: 'ok' });
    expect(items[2]).toMatchObject({ callId: 'c2', status: 'ok' });
  });

  it('a tool call with no tool message is failed, with the fallback result text', () => {
    const items = itemsFromMessages([
      { role: 'assistant', content: null, tool_calls: [{ id: 'c9', type: 'function', function: { name: 'run_command', arguments: '{}' } }] },
    ]);
    expect(items).toEqual([
      { kind: 'tool', callId: 'c9', name: 'run_command', args: {}, status: 'failed', result: NO_RESULT_TEXT, decision: null, reason: null },
    ]);
  });

  it('arguments that are not valid JSON are kept as the raw string', () => {
    const items = itemsFromMessages([
      { role: 'assistant', content: null, tool_calls: [{ id: 'c1', type: 'function', function: { name: 'run_command', arguments: '{oops' } }] },
      { role: 'tool', tool_call_id: 'c1', content: 'ok' },
    ]);
    expect(items[0]).toMatchObject({ kind: 'tool', args: '{oops', status: 'ok' });
  });

  it('a result starting with "error:" or the DENIED prefix is failed', () => {
    expect(DENIED_PREFIX).toBe('not_approved');
    const items = itemsFromMessages([
      {
        role: 'assistant',
        content: null,
        tool_calls: [
          { id: 'e', type: 'function', function: { name: 'run_command', arguments: '{}' } },
          { id: 'd', type: 'function', function: { name: 'run_command', arguments: '{}' } },
          { id: 'p', type: 'function', function: { name: 'get_cwd', arguments: '{}' } },
        ],
      },
      { role: 'tool', tool_call_id: 'e', content: 'error: shell is busy' },
      { role: 'tool', tool_call_id: 'd', content: `${DENIED_PREFIX}: the user did not approve this call` },
      { role: 'tool', tool_call_id: 'p', content: 'pwd: /tmp' },
    ]);
    expect(items.map((i) => (i.kind === 'tool' ? i.status : null))).toEqual(['failed', 'failed', 'ok']);
  });

  it('a result starting with "skipped:" (the backend close text) is failed, and one that only contains it is not', () => {
    const items = itemsFromMessages([
      {
        role: 'assistant',
        content: null,
        tool_calls: [
          { id: 'n', type: 'function', function: { name: 'run_command', arguments: '{}' } },
          { id: 't', type: 'function', function: { name: 'run_command', arguments: '{}' } },
          { id: 'o', type: 'function', function: { name: 'get_cwd', arguments: '{}' } },
        ],
      },
      { role: 'tool', tool_call_id: 'n', content: 'skipped: not recorded' },
      { role: 'tool', tool_call_id: 't', content: 'skipped: turn ended' },
      { role: 'tool', tool_call_id: 'o', content: 'the output said skipped: twice' },
    ]);
    expect(items.map((i) => (i.kind === 'tool' ? i.status : null))).toEqual(['failed', 'failed', 'ok']);
    expect(items[0]).toMatchObject({ kind: 'tool', result: 'skipped: not recorded' });
  });

  it('an empty or whitespace-only assistant message gives no agent item but keeps its calls', () => {
    const items = itemsFromMessages([
      { role: 'assistant', content: '   ', tool_calls: [{ id: 'c1', type: 'function', function: { name: 'get_cwd', arguments: '{}' } }] },
      { role: 'tool', tool_call_id: 'c1', content: '/' },
      { role: 'assistant', content: '' },
    ]);
    expect(items.map((i) => i.kind)).toEqual(['tool']);
  });

  it('tool messages that answer no call are dropped, and malformed calls are skipped', () => {
    const items = itemsFromMessages([
      { role: 'tool', tool_call_id: 'orphan', content: 'x' },
      {
        role: 'assistant',
        content: null,
        tool_calls: [{ type: 'function', function: { name: 'no_id', arguments: '{}' } }, 'not an object'],
      },
      { role: 'system', content: 'ignored' },
    ]);
    expect(items).toEqual([]);
  });

  it('a call with a missing function name is shown as unknown', () => {
    const items = itemsFromMessages([{ role: 'assistant', content: null, tool_calls: [{ id: 'c1' }] }, { role: 'tool', tool_call_id: 'c1', content: 'ok' }]);
    expect(items[0]).toMatchObject({ name: 'unknown', args: null });
  });
});

describe('groupByDate', () => {
  // now is local noon on 10 November 2026.
  const now = local(2026, 10, 10, 12, 0);

  it('groups Today, Yesterday, Previous 7 days, then by month, newest first', () => {
    const groups = groupByDate(
      [
        meta('old', local(2026, 9, 3)),
        meta('today', local(2026, 10, 10, 9)),
        meta('week', local(2026, 10, 5)),
        meta('yesterday', local(2026, 10, 9, 23)),
      ],
      now,
    );
    expect(groups.map((g) => g.label)).toEqual(['Today', 'Yesterday', 'Previous 7 days', 'October 2026']);
    expect(groups.map((g) => g.items.map((m) => m.id))).toEqual([['today'], ['yesterday'], ['week'], ['old']]);
  });

  it('local midnight starts today: 00:00:00 is today, one second earlier is yesterday', () => {
    const atMidnight = groupByDate([meta('a', local(2026, 10, 10, 0, 0))], now);
    expect(atMidnight[0].label).toBe('Today');
    const justBefore = groupByDate([meta('a', local(2026, 10, 10, 0, 0) - 1)], now);
    expect(justBefore[0].label).toBe('Yesterday');
  });

  it('yesterday starts at the local midnight before today', () => {
    expect(groupByDate([meta('a', local(2026, 10, 9, 0, 0))], now)[0].label).toBe('Yesterday');
    expect(groupByDate([meta('a', local(2026, 10, 8, 23, 59))], now)[0].label).toBe('Previous 7 days');
  });

  it('Previous 7 days covers the seven days before yesterday, and its edge is eight days back at midnight', () => {
    // With today 10 Nov, the group runs from 2 Nov (eight days back) up to 9 Nov, yesterday's midnight.
    const first = groupByDate([meta('a', local(2026, 10, 2, 0, 0))], now)[0];
    expect(first.label).toBe('Previous 7 days');
    // One second before 2 Nov midnight is 1 Nov, which is outside the group and shown by month.
    const beforeEdge = groupByDate([meta('a', local(2026, 10, 2, 0, 0) - 1)], now)[0];
    expect(beforeEdge.label).toBe('November 2026');
  });

  it('an update time after now (clock skew) counts as today', () => {
    expect(groupByDate([meta('a', now + 3600)], now)[0].label).toBe('Today');
  });

  it('a time that cannot be read is in one last group, "Unknown date", and never a NaN label', () => {
    const groups = groupByDate(
      [meta('bad', Number.NaN), meta('inf', Number.POSITIVE_INFINITY), meta('huge', 1e20), meta('today', local(2026, 10, 10, 9))],
      now,
    );
    expect(groups.map((g) => g.label)).toEqual(['Today', UNKNOWN_DATE]);
    expect(groups[1].items.map((m) => m.id)).toEqual(['bad', 'inf', 'huge']);
    expect(groups.every((g) => !g.label.includes('NaN') && !g.key.includes('NaN'))).toBe(true);
  });

  it('omits empty groups and orders the months newest first', () => {
    const groups = groupByDate([meta('a', local(2025, 11, 20)), meta('b', local(2026, 0, 4)), meta('c', local(2026, 4, 1))], now);
    expect(groups.map((g) => g.label)).toEqual(['May 2026', 'January 2026', 'December 2025']);
  });

  it('keeps the newest first within a group', () => {
    const groups = groupByDate([meta('early', local(2026, 10, 10, 1)), meta('late', local(2026, 10, 10, 11))], now);
    expect(groups[0].items.map((m) => m.id)).toEqual(['late', 'early']);
  });

  it('does not reorder the input array', () => {
    const input = [meta('a', 1), meta('b', 2)];
    groupByDate(input, now);
    expect(input.map((m) => m.id)).toEqual(['a', 'b']);
  });
});

describe('groupByWorkspace', () => {
  it('one group per workspace, most recently active first', () => {
    const groups = groupByWorkspace([
      meta('a1', 100, ROOT_A),
      meta('b1', 300, ROOT_B),
      meta('a2', 200, ROOT_A),
    ]);
    expect(groups.map((g) => g.label)).toEqual(['beta', 'alpha']);
    expect(groups[0]).toMatchObject({ root: ROOT_B.root, host: ROOT_B.host });
    expect(groups[1].items.map((m) => m.id)).toEqual(['a2', 'a1']);
  });

  it('the Unknown workspace is last, even when it is the most recent', () => {
    const groups = groupByWorkspace([meta('u', 900, UNKNOWN), meta('a', 100, ROOT_A), meta('b', 50, ROOT_B)]);
    expect(groups.map((g) => g.label)).toEqual(['alpha', 'beta', 'Unknown']);
  });

  it('two workspaces with the same label but different roots are different groups', () => {
    const other: Workspace = { host: 'mac', root: '/Users/me/work/alpha', label: 'alpha' };
    expect(groupByWorkspace([meta('a', 1, ROOT_A), meta('b', 2, other)])).toHaveLength(2);
  });

  it('a remote host with the same root is its own group', () => {
    const remote: Workspace = { host: 'build-box', root: '/Users/me/alpha', label: 'alpha' };
    expect(groupByWorkspace([meta('a', 1, ROOT_A), meta('b', 2, remote)])).toHaveLength(2);
  });
});

describe('filterMetas', () => {
  const metas = [
    meta('1', 1, ROOT_A, 'Fix the build script'),
    meta('2', 2, ROOT_B, 'Explain the log'),
    meta('3', 3, { host: 'mac', root: '/Users/me/gamma', label: 'gamma' }, 'Rename things'),
  ];

  it('an empty or blank query keeps everything, in order', () => {
    expect(filterMetas(metas, '')).toBe(metas);
    expect(filterMetas(metas, '   ').map((m) => m.id)).toEqual(['1', '2', '3']);
  });

  it('matches case-insensitively over the title', () => {
    expect(filterMetas(metas, 'BUILD').map((m) => m.id)).toEqual(['1']);
  });

  it('matches the workspace label and root', () => {
    expect(filterMetas(metas, 'beta').map((m) => m.id)).toEqual(['2']);
    expect(filterMetas(metas, '/users/me/gamma').map((m) => m.id)).toEqual(['3']);
  });

  it('every whitespace-separated term must match, in any field', () => {
    expect(filterMetas(metas, 'fix alpha').map((m) => m.id)).toEqual(['1']);
    expect(filterMetas(metas, 'fix beta')).toEqual([]);
    expect(filterMetas(metas, '  log   beta ').map((m) => m.id)).toEqual(['2']);
  });
});

describe('formatWhen', () => {
  const now = local(2026, 10, 10, 15, 30);

  it('shows HH:MM for the same local day', () => {
    expect(formatWhen(local(2026, 10, 10, 9, 5), now)).toBe('09:05');
    expect(formatWhen(local(2026, 10, 10, 0, 0), now)).toBe('00:00');
  });

  it('shows the month and day for the same year', () => {
    expect(formatWhen(local(2026, 10, 8, 14, 0), now)).toBe('Nov 8');
    expect(formatWhen(local(2026, 0, 3, 14, 0), now)).toBe('Jan 3');
  });

  it('adds the year for another year', () => {
    expect(formatWhen(local(2025, 10, 8, 14, 0), now)).toBe('Nov 8, 2025');
  });

  it('a time that cannot be read is "Unknown date"', () => {
    expect(formatWhen(Number.NaN, now)).toBe(UNKNOWN_DATE);
    expect(formatWhen(1e20, now)).toBe(UNKNOWN_DATE);
  });
});

describe('relativeTime', () => {
  const now = local(2026, 10, 10, 15, 30);

  it('is "just now" under a minute, and for a time slightly ahead', () => {
    expect(relativeTime(now, now)).toBe('just now');
    expect(relativeTime(now - 59, now)).toBe('just now');
    expect(relativeTime(now + 30, now)).toBe('just now');
    expect(relativeTime(now + 60, now)).toBe('just now');
  });

  it('a time more than a minute ahead is the absolute time, not "just now"', () => {
    const ahead = now + 3600;
    expect(relativeTime(ahead, now)).toBe(formatWhen(ahead, now));
    expect(relativeTime(ahead, now)).not.toBe('just now');
    expect(relativeTime(now + 61, now)).toBe(formatWhen(now + 61, now));
  });

  it('a time that cannot be read is "Unknown date", never NaN', () => {
    expect(relativeTime(Number.NaN, now)).toBe(UNKNOWN_DATE);
    expect(relativeTime(now, Number.NaN)).toBe(UNKNOWN_DATE);
    expect(relativeTime(1e20, now)).toBe(UNKNOWN_DATE);
  });

  it('is minutes, then hours, then days, under a week', () => {
    expect(relativeTime(now - 5 * 60, now)).toBe('5m ago');
    expect(relativeTime(now - 3 * 3600, now)).toBe('3h ago');
    expect(relativeTime(now - 2 * 86400, now)).toBe('2d ago');
    expect(relativeTime(now - (7 * 86400 - 1), now)).toBe('6d ago');
  });

  it('is the clock time from a week on', () => {
    const old = now - 7 * 86400;
    expect(relativeTime(old, now)).toBe(formatWhen(old, now));
  });
});

describe('safeText', () => {
  it('leaves ordinary text, and emoji with joiners, unchanged', () => {
    expect(safeText('Fix the build')).toBe('Fix the build');
    expect(safeText('ünïcode 日本語')).toBe('ünïcode 日本語');
    // ZWJ (U+200D) and ZWNJ (U+200C) are needed by emoji sequences and some scripts, so they stay.
    expect(safeText('👩‍💻')).toBe('👩‍💻');
    expect(safeText('ب‌ا')).toBe('ب‌ا');
  });

  it('removes each bidi control and invisible format character', () => {
    for (const cp of [0x200b, 0x200e, 0x200f, 0x202a, 0x202b, 0x202c, 0x202d, 0x202e, 0x2060, 0x2066, 0x2067, 0x2068, 0x2069, 0xfeff]) {
      const ch = String.fromCodePoint(cp);
      expect(safeText(`a${ch}b`)).toBe('ab');
    }
  });

  it('a right-to-left override in a title does not reverse the text that follows it', () => {
    expect(safeText('evil‮gnirts')).toBe('evilgnirts');
  });

  it('control characters become spaces, so words do not run together', () => {
    expect(safeText('line one\ntwo\u0007three')).toBe('line one two three');
  });
});

describe('itemsFromMessages: todo_write', () => {
  const todoCall = (id: string) => ({ id, type: 'function', function: { name: 'todo_write', arguments: '{"items":[]}' } });

  it('a todo_write call and its tool message give no item, and the rest of the turn is kept', () => {
    const items = itemsFromMessages([
      { role: 'user', content: 'plan it' },
      { role: 'assistant', content: 'Planning.', tool_calls: [todoCall('t1')] },
      { role: 'tool', tool_call_id: 't1', content: 'todo list updated: 0 done, 0 in progress, 2 pending' },
      { role: 'assistant', content: '', tool_calls: [{ id: 'c2', type: 'function', function: { name: 'run_command', arguments: '{"command":"ls"}' } }] },
      { role: 'tool', tool_call_id: 'c2', content: 'ok' },
    ]);
    expect(items.map((i) => (i.kind === 'tool' ? `tool:${i.name}` : i.kind))).toEqual(['user', 'agent', 'tool:run_command']);
  });

  it('a todo_write call with no tool message gives no item and no failed row', () => {
    const items = itemsFromMessages([{ role: 'assistant', content: '', tool_calls: [todoCall('t1')] }]);
    expect(items).toEqual([]);
  });
});

describe('itemsFromMessages: a user message with attachments (array content)', () => {  const PNG_THUMB = 'data:image/png;base64,iVBORw0KGgo=';
  const sha = 'a'.repeat(64);
  /** The first attachment of the first item, as a plain record (the item is a union, so it is read through unknown). */
  const firstAttachment = (item: unknown): Record<string, unknown> => (item as { attachments: Array<Record<string, unknown>> }).attachments[0];

  it('turns the text part into the text, and each attachment part into an attachment', () => {
    const items = itemsFromMessages([
      {
        role: 'user',
        content: [
          { type: 'text', text: 'what is in these' },
          { type: 'watt_image', sha256: sha, mime: 'image/png', name: 'shot.png', bytes: 2048, w: 10, h: 10, thumb: PNG_THUMB },
          { type: 'watt_file', name: 'notes.md', bytes: 300, text: 'stored text, not shown here' },
        ],
      },
    ]);
    expect(items).toEqual([
      {
        kind: 'user',
        clientId: 0,
        text: 'what is in these',
        attachments: [
          { kind: 'image', name: 'shot.png', bytes: 2048, thumb: PNG_THUMB },
          { kind: 'file', name: 'notes.md', bytes: 300 },
        ],
      },
    ]);
  });

  it('never shows the array as JSON: the text is the text part, and nothing else leaks into it', () => {
    const [item] = itemsFromMessages([{ role: 'user', content: [{ type: 'text', text: 'hello' }, { type: 'watt_file', name: 'a.txt', bytes: 1, text: 'x' }] }]);
    expect(item).toMatchObject({ kind: 'user', text: 'hello' });
    expect(JSON.stringify(item)).not.toContain('watt_file"}');
  });

  it('an image without a stored thumbnail is an image attachment with no thumb', () => {
    const [item] = itemsFromMessages([{ role: 'user', content: [{ type: 'text', text: 'x' }, { type: 'watt_image', sha256: sha, mime: 'image/jpeg', name: 'p.jpg', bytes: 9 }] }]);
    expect(item).toMatchObject({ kind: 'user', attachments: [{ kind: 'image', name: 'p.jpg', bytes: 9 }] });
    expect(firstAttachment(item)).not.toHaveProperty('thumb');
  });

  it('a thumbnail that is not a PNG or JPEG data URL is dropped', () => {
    const [item] = itemsFromMessages([
      { role: 'user', content: [{ type: 'watt_image', sha256: sha, mime: 'image/png', name: 'a.png', bytes: 1, thumb: 'https://example.test/t.png' }] },
    ]);
    expect(firstAttachment(item)).not.toHaveProperty('thumb');
  });

  it('unknown parts are ignored, and a missing name or size has a safe default', () => {
    const items = itemsFromMessages([
      { role: 'user', content: [{ type: 'text', text: 'hi' }, { type: 'mystery', x: 1 }, { type: 'watt_file', text: 'body' }] },
    ]);
    expect(items).toEqual([{ kind: 'user', clientId: 0, text: 'hi', attachments: [{ kind: 'file', name: 'attachment', bytes: 0 }] }]);
  });

  it('an attachment-only message (no text part text) is kept, with no text', () => {
    const items = itemsFromMessages([{ role: 'user', content: [{ type: 'text', text: '' }, { type: 'watt_file', name: 'only.md', bytes: 4, text: 'data' }] }]);
    expect(items).toEqual([{ kind: 'user', clientId: 0, text: '', attachments: [{ kind: 'file', name: 'only.md', bytes: 4 }] }]);
  });

  it('an array with no text and no known attachment is skipped, as an empty message is', () => {
    expect(itemsFromMessages([{ role: 'user', content: [{ type: 'text', text: '' }, { type: 'other' }] }])).toEqual([]);
    expect(itemsFromMessages([{ role: 'user', content: [] }])).toEqual([]);
  });

  it('a plain string message has no attachments key', () => {
    expect(itemsFromMessages([{ role: 'user', content: 'plain' }])).toEqual([{ kind: 'user', clientId: 0, text: 'plain' }]);
  });
});

describe('itemsFromMessages: ask_user', () => {
  const ARGS = JSON.stringify({
    questions: [{ question: 'Which capa?', header: 'Capa', options: [{ label: 'a', description: 'first' }, { label: 'b' }] }],
  });
  const RESULT = JSON.stringify({
    answers: [{ question_id: 'q1', question: 'Which capa?', header: 'Capa', status: 'answered', selected: ['a'] }],
  });
  const call = (id: string) => ({
    role: 'assistant',
    content: null,
    tool_calls: [{ id, type: 'function', function: { name: 'ask_user', arguments: ARGS } }],
  });

  it('a settled call is a resolved question card, and no tool row', () => {
    const items = itemsFromMessages([call('c1'), { role: 'tool', tool_call_id: 'c1', content: RESULT }]);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({
      kind: 'question',
      callId: 'c1',
      state: 'resolved',
      questions: [{ id: 'q1', question: 'Which capa?', header: 'Capa' }],
      result: { answers: [{ status: 'answered', selected: ['a'] }] },
      answers: null,
      error: null,
    });
    expect(items[0]).not.toMatchObject({ kind: 'tool' });
  });

  it('an interrupted call is a closed card, never a tool row and never answerable', () => {
    // No tool message, the repair close text, the fallback text, or an error text: the turn never settled the call.
    const contents = [undefined, 'skipped: not recorded', 'skipped: turn ended', NO_RESULT_TEXT, 'error: not_approved: nope'];
    for (const content of contents) {
      const messages: ChatMessage[] =
        content === undefined ? [call('c1')] : [call('c1'), { role: 'tool', tool_call_id: 'c1', content }];
      const items = itemsFromMessages(messages);
      expect(items).toHaveLength(1);
      expect(items[0]).toMatchObject({
        kind: 'question',
        callId: 'c1',
        state: 'closed',
        questions: [{ id: 'q1', question: 'Which capa?', header: 'Capa' }],
        result: { answers: [{ question_id: 'q1', question: 'Which capa?', header: 'Capa', status: 'cancelled' }] },
        answers: null,
        error: null,
        note: UNANSWERED_NOTE,
      });
      expect(items[0]).not.toMatchObject({ kind: 'tool' });
    }
  });

  it('an unreadable call falls back to a generic tool row', () => {
    // Bad arguments are a generic row, never both a card and a row.
    const badArgs = itemsFromMessages([
      { role: 'assistant', content: null, tool_calls: [{ id: 'c1', type: 'function', function: { name: 'ask_user', arguments: '{"questions": []}' } }] },
      { role: 'tool', tool_call_id: 'c1', content: RESULT },
    ]);
    expect(badArgs).toHaveLength(1);
    expect(badArgs[0]).toMatchObject({ kind: 'tool', name: 'ask_user' });
    // A present result that is not a result is a generic row too.
    const badResult = itemsFromMessages([call('c1'), { role: 'tool', tool_call_id: 'c1', content: '{"answers": []}' }]);
    expect(badResult).toHaveLength(1);
    expect(badResult[0]).toMatchObject({ kind: 'tool', name: 'ask_user' });
  });

  it('other calls are still shown next to a question card', () => {
    const items = itemsFromMessages([
      call('c1'),
      { role: 'assistant', content: null, tool_calls: [{ id: 'c2', type: 'function', function: { name: 'get_cwd', arguments: '{}' } }] },
      { role: 'tool', tool_call_id: 'c1', content: RESULT },
      { role: 'tool', tool_call_id: 'c2', content: '/' },
    ]);
    expect(items.map((i) => i.kind)).toEqual(['question', 'tool']);
    expect(items[1]).toMatchObject({ kind: 'tool', name: 'get_cwd', status: 'ok' });
  });
});

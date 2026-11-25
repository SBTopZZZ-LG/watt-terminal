import { describe, expect, it } from 'vitest';
import { CHUNK_MESSAGES, isTurnStart, keptTop, needsFill, olderStart, OLDER_THRESHOLD_PX, tailStart, wantsOlder } from './transcriptPaging';
import type { ChatMessage } from './types';

const user = (text: string): ChatMessage => ({ role: 'user', content: text });
const steer = (text: string): ChatMessage => ({ role: 'user', content: text, steer: true });
const assistant = (text: string): ChatMessage => ({ role: 'assistant', content: text });

/** `n` messages, a user message at each index for which `userAt` is true, an assistant message otherwise. */
function pattern(n: number, userAt: (i: number) => boolean): ChatMessage[] {
  return Array.from({ length: n }, (_, i) => (userAt(i) ? user(`u${i}`) : assistant(`a${i}`)));
}

/** Every chunk of a conversation, newest first, as the reducer takes them. */
function chunksNewestFirst(messages: ChatMessage[], size = CHUNK_MESSAGES): Array<[number, number]> {
  const out: Array<[number, number]> = [];
  let end = messages.length;
  while (end > 0) {
    const start = olderStart(messages, end, size);
    out.push([start, end]);
    end = start;
  }
  return out;
}

describe('isTurnStart', () => {
  it('a user message starts a turn, a steered one does not, and no other message does', () => {
    expect(isTurnStart(user('hi'))).toBe(true);
    expect(isTurnStart(steer('change of plan'))).toBe(false);
    expect(isTurnStart(assistant('hello'))).toBe(false);
    expect(isTurnStart({ role: 'tool', tool_call_id: 'c1', content: 'ok' })).toBe(false);
  });
});

describe('tailStart', () => {
  it('a conversation within one chunk has no older part', () => {
    const messages = pattern(CHUNK_MESSAGES, (i) => i % 2 === 0);
    expect(tailStart(messages)).toBe(0);
    expect(olderStart(messages, messages.length)).toBe(0);
  });

  it('the newest chunk is about CHUNK_MESSAGES messages, starting at a turn start', () => {
    const messages = pattern(200, (i) => i % 2 === 0);
    const cut = tailStart(messages);
    expect(cut).toBe(200 - CHUNK_MESSAGES);
    expect(isTurnStart(messages[cut])).toBe(true);
  });

  it('moves back to the nearest turn start when the cut would land mid-turn', () => {
    // Turn starts every 5 messages. A cut at 141 (200 - 59) lands mid-turn, so it moves back to 140.
    const messages = pattern(201, (i) => i % 5 === 0);
    expect(tailStart(messages, 60)).toBe(140);
  });

  it('a steered message is not a turn start, so the cut moves past it', () => {
    // Index 7 (length 10 - size 3) is a steered message: the cut must not stop there.
    const messages = [user('u0'), assistant('a1'), assistant('a2'), assistant('a3'), assistant('a4'), assistant('a5'), assistant('a6'), steer('s7'), assistant('a8'), assistant('a9')];
    expect(tailStart(messages, 3)).toBe(0);
  });

  it('with no turn start in range it goes back to the nearest one before, or to the start', () => {
    const messages = pattern(300, (i) => i === 3);
    expect(tailStart(messages, 60)).toBe(3);
    expect(tailStart(pattern(300, () => false), 60)).toBe(0);
    expect(tailStart([], 60)).toBe(0);
  });
});

describe('olderStart', () => {
  it('takes the chunk before the given end, starting at a turn start', () => {
    const messages = pattern(200, (i) => i % 2 === 0);
    const start = olderStart(messages, 140);
    expect(start).toBe(140 - CHUNK_MESSAGES);
    expect(isTurnStart(messages[start])).toBe(true);
  });

  it('gives 0 when nothing is left', () => {
    expect(olderStart(pattern(10, () => true), 0)).toBe(0);
  });
});

describe('chunking', () => {
  it('successive chunks cover every message exactly once, in order, each at a turn start', () => {
    const messages = pattern(517, (i) => i % 3 === 0 || i === 0);
    const chunks = chunksNewestFirst(messages);
    // Oldest first: the ranges are contiguous from 0 to the end.
    const ordered = [...chunks].reverse();
    expect(ordered[0][0]).toBe(0);
    for (let k = 0; k < ordered.length; k++) {
      expect(ordered[k][1]).toBe(k + 1 < ordered.length ? ordered[k + 1][0] : messages.length);
      expect(ordered[k][0] < ordered[k][1]).toBe(true);
      expect(ordered[k][0] === 0 || isTurnStart(messages[ordered[k][0]])).toBe(true);
    }
  });

  it('never splits a tool call from its result', () => {
    // Each turn: a user message, an assistant call, its tool result, and a closing assistant message.
    const messages: ChatMessage[] = [];
    for (let t = 0; t < 40; t++) {
      messages.push(user(`q${t}`), { role: 'assistant', content: '', tool_calls: [{ id: `c${t}`, type: 'function', function: { name: 'x', arguments: '{}' } }] });
      messages.push({ role: 'tool', tool_call_id: `c${t}`, content: 'ok' }, assistant(`a${t}`));
    }
    for (const [start, end] of chunksNewestFirst(messages)) {
      const calls = new Set<string>();
      const results = new Set<string>();
      for (const m of messages.slice(start, end)) {
        for (const c of (m.tool_calls as Array<{ id: string }> | undefined) ?? []) {
          calls.add(c.id);
        }
        if (m.role === 'tool') {
          results.add(m.tool_call_id as string);
        }
      }
      expect([...results].every((id) => calls.has(id))).toBe(true);
    }
  });

  it('a conversation within one chunk gives one chunk, with nothing older', () => {
    const messages = pattern(CHUNK_MESSAGES, (i) => i % 2 === 0);
    expect(chunksNewestFirst(messages)).toEqual([[0, CHUNK_MESSAGES]]);
  });

  it('terminates when there are no turn starts (every chunk takes the rest)', () => {
    const messages = pattern(1000, () => false);
    expect(chunksNewestFirst(messages)).toEqual([[0, 1000]]);
  });
});

describe('wantsOlder', () => {
  const near = { movedUp: true, top: OLDER_THRESHOLD_PX, hasOlder: true, pending: false };

  it('asks on an upward scroll that reaches the threshold, with more to load and none pending', () => {
    expect(wantsOlder(near)).toBe(true);
    expect(wantsOlder({ ...near, top: 0 })).toBe(true);
  });

  it('does not ask on a downward scroll, away from the top, with nothing left, or while a load is pending', () => {
    expect(wantsOlder({ ...near, movedUp: false })).toBe(false);
    expect(wantsOlder({ ...near, top: OLDER_THRESHOLD_PX + 1 })).toBe(false);
    expect(wantsOlder({ ...near, hasOlder: false })).toBe(false);
    expect(wantsOlder({ ...near, pending: true })).toBe(false);
  });
});

describe('needsFill', () => {
  it('content shorter than the view needs an older chunk; a longer one does not', () => {
    expect(needsFill({ scrollTop: 0, scrollHeight: 100, clientHeight: 200 })).toBe(true);
    expect(needsFill({ scrollTop: 0, scrollHeight: 200, clientHeight: 200 })).toBe(true);
    expect(needsFill({ scrollTop: 0, scrollHeight: 300, clientHeight: 200 })).toBe(false);
  });

  it('a view with no height (not laid out) is not measured', () => {
    expect(needsFill({ scrollTop: 0, scrollHeight: 0, clientHeight: 0 })).toBe(false);
  });
});

describe('keptTop', () => {
  it('moves the view down by the growth above it, so the same content stays in view', () => {
    expect(keptTop({ top: 100, height: 1000 }, 1500)).toBe(600);
    expect(keptTop({ top: 0, height: 1000 }, 1000)).toBe(0);
  });
});

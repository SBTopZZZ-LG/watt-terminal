import { describe, expect, it } from 'vitest';
import { CHAT_FIND_LIMIT, findChatMatches, keepMatch, stepMatch } from './chatFind';

describe('findChatMatches', () => {
  it('has no matches for an empty query', () => {
    expect(findChatMatches(['anything'], '', false)).toEqual([]);
  });

  it('finds every match in each text, with offsets, in order', () => {
    expect(findChatMatches(['cat and cat', 'no', 'a cat'], 'cat', false)).toEqual([
      { nodeIndex: 0, start: 0, end: 3 },
      { nodeIndex: 0, start: 8, end: 11 },
      { nodeIndex: 2, start: 2, end: 5 },
    ]);
  });

  it('is case-insensitive by default, and case-sensitive on request', () => {
    const texts = ['Error error ERROR'];
    expect(findChatMatches(texts, 'error', false)).toHaveLength(3);
    expect(findChatMatches(texts, 'error', true)).toEqual([{ nodeIndex: 0, start: 6, end: 11 }]);
  });

  it('folds non-ASCII case too', () => {
    expect(findChatMatches(['CAFÉ café'], 'café', false)).toHaveLength(2);
  });

  it('does not overlap matches within a text', () => {
    expect(findChatMatches(['aaaa'], 'aa', false)).toEqual([
      { nodeIndex: 0, start: 0, end: 2 },
      { nodeIndex: 0, start: 2, end: 4 },
    ]);
  });

  it('treats the query as plain text, not as a pattern', () => {
    expect(findChatMatches(['a.b axb a+b'], 'a.b', false)).toEqual([{ nodeIndex: 0, start: 0, end: 3 }]);
    expect(findChatMatches(['a+b (x)'], '(x)', false)).toEqual([{ nodeIndex: 0, start: 4, end: 7 }]);
  });

  it('does not find a phrase split across two texts', () => {
    expect(findChatMatches(['hello ', 'world'], 'hello world', false)).toEqual([]);
  });

  it('stops at the limit', () => {
    const texts = ['x x x x', 'x x'];
    expect(findChatMatches(texts, 'x', false, 3)).toHaveLength(3);
    expect(findChatMatches(texts, 'x', false)).toHaveLength(6);
    expect(CHAT_FIND_LIMIT).toBe(1000);
  });
});

describe('stepMatch', () => {
  it('moves forward and back, wrapping at both ends', () => {
    expect(stepMatch(0, 3, 'next')).toBe(1);
    expect(stepMatch(2, 3, 'next')).toBe(0);
    expect(stepMatch(0, 3, 'prev')).toBe(2);
    expect(stepMatch(2, 3, 'prev')).toBe(1);
  });

  it('starts at the first match going forward and at the last going back', () => {
    expect(stepMatch(-1, 3, 'next')).toBe(0);
    expect(stepMatch(-1, 3, 'prev')).toBe(2);
  });

  it('is -1 when there are no matches', () => {
    expect(stepMatch(-1, 0, 'next')).toBe(-1);
    expect(stepMatch(0, 0, 'prev')).toBe(-1);
  });

  it('recovers an index that is past the end', () => {
    expect(stepMatch(5, 3, 'next')).toBe(0);
  });
});

describe('keepMatch', () => {
  it('keeps the index while it exists, and clamps it to the last match', () => {
    expect(keepMatch(1, 4)).toBe(1);
    expect(keepMatch(7, 4)).toBe(3);
  });

  it('starts at the first match when there was none, and is -1 with no matches', () => {
    expect(keepMatch(-1, 4)).toBe(0);
    expect(keepMatch(2, 0)).toBe(-1);
  });
});

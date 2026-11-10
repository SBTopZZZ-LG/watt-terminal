import { describe, expect, it } from 'vitest';
import { distanceFromBottom, isAtBottom, shouldFollowAfterSend } from './stickScroll';
import type { TranscriptItem } from './transcript';

describe('distanceFromBottom', () => {
  it('is 0 at exact bottom', () => {
    expect(distanceFromBottom({ scrollTop: 700, scrollHeight: 1000, clientHeight: 300 })).toBe(0);
  });

  it('is the gap above the bottom', () => {
    expect(distanceFromBottom({ scrollTop: 600, scrollHeight: 1000, clientHeight: 300 })).toBe(100);
  });

  it('is 0 when content is shorter than the view', () => {
    expect(distanceFromBottom({ scrollTop: 0, scrollHeight: 120, clientHeight: 300 })).toBe(0);
  });

  it('is never negative for a fractional scrollTop past the bottom', () => {
    expect(distanceFromBottom({ scrollTop: 700.6, scrollHeight: 1000, clientHeight: 300 })).toBe(0);
  });
});

describe('isAtBottom', () => {
  const at = (scrollTop: number) => ({ scrollTop, scrollHeight: 1000, clientHeight: 300 });

  it('is true at exact bottom', () => {
    expect(isAtBottom(at(700))).toBe(true);
  });

  it('is true for a fractional scrollTop within the threshold (high-DPI)', () => {
    expect(isAtBottom(at(699.5))).toBe(true);
  });

  it('is true exactly at the default threshold (24px)', () => {
    expect(isAtBottom(at(676))).toBe(true);
  });

  it('is false just beyond the default threshold', () => {
    expect(isAtBottom(at(675))).toBe(false);
  });

  it('is false well above the bottom', () => {
    expect(isAtBottom(at(100))).toBe(false);
  });

  it('is true when content is shorter than the view', () => {
    expect(isAtBottom({ scrollTop: 0, scrollHeight: 120, clientHeight: 300 })).toBe(true);
  });

  it('honours an explicit threshold', () => {
    expect(isAtBottom(at(690), 5)).toBe(false);
    expect(isAtBottom(at(696), 5)).toBe(true);
  });
});

describe('shouldFollowAfterSend', () => {
  const user = (id: number): TranscriptItem => ({ id, kind: 'user', clientId: id, text: 'hi' });
  const agent = (id: number): TranscriptItem => ({ id, kind: 'agent', text: 'reply', streaming: true });

  it('is false for an empty list', () => {
    expect(shouldFollowAfterSend(undefined, undefined)).toBe(false);
  });

  it('is true when a user message is the first item', () => {
    expect(shouldFollowAfterSend(undefined, user(1))).toBe(true);
  });

  it('is true when a new user message is appended after an agent item', () => {
    expect(shouldFollowAfterSend(agent(4), user(5))).toBe(true);
  });

  it('is false when the last item is the same user message (no new item)', () => {
    expect(shouldFollowAfterSend(user(5), user(5))).toBe(false);
  });

  it('is false when an agent item is appended', () => {
    expect(shouldFollowAfterSend(user(5), agent(6))).toBe(false);
  });

  it('is false when the last item is unchanged agent text', () => {
    expect(shouldFollowAfterSend(agent(6), agent(6))).toBe(false);
  });
});

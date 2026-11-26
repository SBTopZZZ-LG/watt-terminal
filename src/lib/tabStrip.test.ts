import { describe, expect, it } from 'vitest';
import { allocTabId, isOwnedByChat, neighbourAfterClose, TAB_LIMIT, tabLimitReached } from './tabStrip';

describe('tabLimitReached', () => {
  it('allows a tab below the limit and refuses one at it', () => {
    expect(TAB_LIMIT).toBe(20);
    expect(tabLimitReached(19)).toBe(false);
    expect(tabLimitReached(20)).toBe(true);
    expect(tabLimitReached(21)).toBe(true);
  });
});

describe('allocTabId', () => {
  it('starts at the counter and returns the counter after it', () => {
    expect(allocTabId(2, new Set(['main']))).toEqual({ id: 't2', counter: 3 });
  });

  it('skips ids already taken, so a reload that kept the backend tabs does not reuse one', () => {
    expect(allocTabId(2, new Set(['main', 't2', 't3']))).toEqual({ id: 't4', counter: 5 });
  });

  it('never repeats an id across calls when the taken set does not change', () => {
    const taken = new Set(['main']);
    const first = allocTabId(2, taken);
    const second = allocTabId(first.counter, taken);
    expect(first.id).toBe('t2');
    expect(second.id).toBe('t3');
  });
});

describe('neighbourAfterClose', () => {
  it('picks the right neighbour when there is one', () => {
    expect(neighbourAfterClose(['main', 't2', 't3'], 't2')).toBe('t3');
    expect(neighbourAfterClose(['main', 't2', 't3'], 'main')).toBe('t2');
  });

  it('picks the left neighbour when the closed tab was the last', () => {
    expect(neighbourAfterClose(['main', 't2', 't3'], 't3')).toBe('t2');
  });

  it('is null when no other tab is left, or the id is unknown', () => {
    expect(neighbourAfterClose(['main'], 'main')).toBeNull();
    expect(neighbourAfterClose(['main', 't2'], 't9')).toBeNull();
  });
});

describe('isOwnedByChat', () => {
  it('is true only for the chat that owns the terminal', () => {
    expect(isOwnedByChat('main', 'main')).toBe(true);
    expect(isOwnedByChat('other', 'main')).toBe(false);
    expect(isOwnedByChat(null, 'main')).toBe(false);
  });
});

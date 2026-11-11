import { describe, expect, it } from 'vitest';
import { hasUnfinished, listMaxHeight, revealIndex, scrollTopToReveal, shouldExpand, todoShape } from './todoPanel';
import type { TodoItem } from './types';

const t = (content: string, status: TodoItem['status'] = 'pending'): TodoItem => ({ content, status });

describe('hasUnfinished and shouldExpand', () => {
  it('a list with an unfinished item is unfinished, an all-done list and an empty list are not', () => {
    expect(hasUnfinished([t('a', 'completed'), t('b')])).toBe(true);
    expect(hasUnfinished([t('a', 'completed'), t('b', 'completed')])).toBe(false);
    expect(hasUnfinished([])).toBe(false);
  });

  it('without a choice the list decides: expanded while unfinished, collapsed once all done', () => {
    expect(shouldExpand([t('a', 'in_progress')], null)).toBe(true);
    expect(shouldExpand([t('a', 'completed')], null)).toBe(false);
  });

  it('a choice the user made wins over the list', () => {
    expect(shouldExpand([t('a', 'in_progress')], false)).toBe(false);
    expect(shouldExpand([t('a', 'completed')], true)).toBe(true);
  });
});

describe('todoShape', () => {
  it('ignores statuses: a status update keeps the shape', () => {
    expect(todoShape([t('a'), t('b')])).toBe(todoShape([t('a', 'completed'), t('b', 'in_progress')]));
  });

  it('changes with the tasks, their order and their count', () => {
    expect(todoShape([t('a'), t('b')])).not.toBe(todoShape([t('a'), t('c')]));
    expect(todoShape([t('a'), t('b')])).not.toBe(todoShape([t('b'), t('a')]));
    expect(todoShape([t('a'), t('b')])).not.toBe(todoShape([t('a')]));
  });
});

describe('revealIndex', () => {
  it('is the last item whose status changed to done in an update of the same shape', () => {
    const prev = [t('a', 'completed'), t('b', 'in_progress'), t('c'), t('d')];
    const next = [t('a', 'completed'), t('b', 'completed'), t('c', 'in_progress'), t('d')];
    expect(revealIndex(prev, next)).toBe(2);
  });

  it('a change to in progress counts as much as a change to done', () => {
    const prev = [t('a', 'completed'), t('b'), t('c')];
    const next = [t('a', 'completed'), t('b', 'in_progress'), t('c')];
    expect(revealIndex(prev, next)).toBe(1);
  });

  it('a change back to pending does not count, so the first unfinished item is used', () => {
    const prev = [t('a', 'completed'), t('b', 'in_progress'), t('c', 'completed')];
    const next = [t('a', 'completed'), t('b'), t('c', 'completed')];
    expect(revealIndex(prev, next)).toBe(1);
  });

  it('with no change to done or in progress, the first unfinished item', () => {
    const prev = [t('a', 'completed'), t('b'), t('c')];
    expect(revealIndex(prev, [...prev])).toBe(1);
  });

  it('a new list (a different shape) falls back to the first unfinished item, whatever its statuses were', () => {
    const prev = [t('a', 'completed'), t('b', 'in_progress')];
    const next = [t('x', 'completed'), t('y', 'completed'), t('z', 'in_progress')];
    expect(revealIndex(prev, next)).toBe(2);
  });

  it('the first render (no previous list) uses the first unfinished item', () => {
    expect(revealIndex(null, [t('a', 'completed'), t('b'), t('c', 'in_progress')])).toBe(1);
  });

  it('the item just completed when the last one is done, and -1 when nothing changed and every item is done', () => {
    expect(revealIndex([t('a', 'completed'), t('b')], [t('a', 'completed'), t('b', 'completed')])).toBe(1);
    expect(revealIndex([t('a', 'completed'), t('b', 'completed')], [t('a', 'completed'), t('b', 'completed')])).toBe(-1);
    expect(revealIndex(null, [])).toBe(-1);
  });
});

describe('scrollTopToReveal', () => {
  it('a row above the view scrolls up to its top', () => {
    expect(scrollTopToReveal(20, 18, 100, 50)).toBe(20);
  });

  it('a row below the view scrolls down until its bottom is visible', () => {
    expect(scrollTopToReveal(200, 18, 100, 0)).toBe(118);
  });

  it('a row already in view does not move the list', () => {
    expect(scrollTopToReveal(60, 18, 100, 30)).toBe(30);
  });

  it('a row taller than the view shows its top', () => {
    expect(scrollTopToReveal(150, 300, 100, 0)).toBe(150);
  });
});

describe('listMaxHeight', () => {
  it('is 40% of the agent panel when that is smaller than 180px', () => {
    expect(listMaxHeight(300)).toBe(120);
    expect(listMaxHeight(401)).toBe(160);
  });

  it('is 180px at most', () => {
    expect(listMaxHeight(800)).toBe(180);
  });

  it('keeps the 180px cap when the panel has no height yet', () => {
    expect(listMaxHeight(0)).toBe(180);
  });
});

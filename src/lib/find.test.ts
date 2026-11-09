import { describe, expect, it } from 'vitest';
import { findLabel, HIGHLIGHT_LIMIT } from './find';

describe('findLabel', () => {
  it('is empty with no query', () => {
    expect(findLabel('', { index: -1, count: 0 })).toBe('');
    expect(findLabel('', { index: 2, count: 5 })).toBe('');
  });

  it('says so when nothing matches', () => {
    expect(findLabel('zzz', { index: -1, count: 0 })).toBe('No results');
  });

  it('shows the selected match as "n of m"', () => {
    expect(findLabel('a', { index: 0, count: 3 })).toBe('1 of 3');
    expect(findLabel('a', { index: 2, count: 3 })).toBe('3 of 3');
  });

  it('shows a plain count when no match is selected', () => {
    expect(findLabel('a', { index: -1, count: 4 })).toBe('4 matches');
  });

  it('marks a capped result set as over the highlight limit', () => {
    expect(HIGHLIGHT_LIMIT).toBe(1000);
    expect(findLabel('a', { index: -1, count: 1000 })).toBe('1000+ matches');
  });
});

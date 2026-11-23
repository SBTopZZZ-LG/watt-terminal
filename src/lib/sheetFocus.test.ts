import { describe, expect, it } from 'vitest';
import { nextStop } from './sheetFocus';

describe('nextStop', () => {
  it('moves forward one stop at a time and wraps from the last to the first', () => {
    expect(nextStop(3, 0, false)).toBe(1);
    expect(nextStop(3, 1, false)).toBe(2);
    expect(nextStop(3, 2, false)).toBe(0);
  });

  it('moves backward one stop at a time and wraps from the first to the last', () => {
    expect(nextStop(3, 2, true)).toBe(1);
    expect(nextStop(3, 1, true)).toBe(0);
    expect(nextStop(3, 0, true)).toBe(2);
  });

  it('starts from the first stop going forward, and from the last going backward, when focus is not on a stop', () => {
    expect(nextStop(3, -1, false)).toBe(0);
    expect(nextStop(3, -1, true)).toBe(2);
  });

  it('is -1 with no stops, and stays on the only stop', () => {
    expect(nextStop(0, -1, false)).toBe(-1);
    expect(nextStop(1, 0, false)).toBe(0);
    expect(nextStop(1, 0, true)).toBe(0);
  });
});

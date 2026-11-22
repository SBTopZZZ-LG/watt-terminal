import { describe, expect, it } from 'vitest';
import { shouldSelectMatch } from './chatFindDom';

describe('shouldSelectMatch', () => {
  it('selects when the input is not focused, with or without the highlight API', () => {
    expect(shouldSelectMatch({ inputFocused: false, highlightsSupported: true })).toBe(true);
    expect(shouldSelectMatch({ inputFocused: false, highlightsSupported: false })).toBe(true);
  });

  it('does not select while the input is focused and the highlights mark the match', () => {
    expect(shouldSelectMatch({ inputFocused: true, highlightsSupported: true })).toBe(false);
  });

  it('selects while the input is focused only when there are no highlights (the selection is the only mark)', () => {
    expect(shouldSelectMatch({ inputFocused: true, highlightsSupported: false })).toBe(true);
  });
});

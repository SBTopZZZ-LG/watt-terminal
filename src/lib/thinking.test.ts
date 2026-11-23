import { describe, expect, it } from 'vitest';
import { DEFAULT_THINKING_MODE, THINKING_MODE_OPTIONS, thinkingOpen, thoughtLabel } from './thinking';

describe('thinkingOpen', () => {
  it('collapsed is closed and expanded is open, streaming or not', () => {
    expect(thinkingOpen('collapsed', true, null)).toBe(false);
    expect(thinkingOpen('collapsed', false, null)).toBe(false);
    expect(thinkingOpen('expanded', true, null)).toBe(true);
    expect(thinkingOpen('expanded', false, null)).toBe(true);
  });

  it('auto is open only while the reasoning streams', () => {
    expect(thinkingOpen('auto', true, null)).toBe(true);
    expect(thinkingOpen('auto', false, null)).toBe(false);
  });

  it('a click (manual) overrides the mode either way', () => {
    expect(thinkingOpen('collapsed', true, true)).toBe(true);
    expect(thinkingOpen('expanded', false, false)).toBe(false);
    expect(thinkingOpen('auto', false, true)).toBe(true);
  });

  it('the default is auto', () => {
    expect(DEFAULT_THINKING_MODE).toBe('auto');
  });
});

describe('thoughtLabel', () => {
  it('says how long the reasoning took, in whole seconds', () => {
    expect(thoughtLabel(1000, 5000)).toBe('Thought for 4s');
    expect(thoughtLabel(0, 1000)).toBe('Thought for 1s');
  });

  it('is just Thought under a second, or without both times', () => {
    expect(thoughtLabel(0, 999)).toBe('Thought');
    expect(thoughtLabel(undefined, 5000)).toBe('Thought');
    expect(thoughtLabel(1000, undefined)).toBe('Thought');
  });
});

describe('THINKING_MODE_OPTIONS', () => {
  it('lists the three modes in the settings order, with the labels the settings show', () => {
    expect(THINKING_MODE_OPTIONS.map((o) => [o.mode, o.label])).toEqual([
      ['collapsed', 'Collapsed'],
      ['auto', 'Auto'],
      ['expanded', 'Expanded'],
    ]);
  });
});

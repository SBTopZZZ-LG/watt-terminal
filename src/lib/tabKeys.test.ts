import { describe, expect, it } from 'vitest';
import { nextTabId } from './tabKeys';

const IDS = ['models', 'approvals', 'extra'];

describe('nextTabId', () => {
  it('ArrowRight and ArrowLeft move one tab', () => {
    expect(nextTabId(IDS, 'models', 'ArrowRight')).toBe('approvals');
    expect(nextTabId(IDS, 'approvals', 'ArrowLeft')).toBe('models');
  });

  it('the arrows wrap at both ends', () => {
    expect(nextTabId(IDS, 'extra', 'ArrowRight')).toBe('models');
    expect(nextTabId(IDS, 'models', 'ArrowLeft')).toBe('extra');
  });

  it('Home and End go to the first and the last tab', () => {
    expect(nextTabId(IDS, 'approvals', 'Home')).toBe('models');
    expect(nextTabId(IDS, 'models', 'End')).toBe('extra');
  });

  it('other keys, an unknown current tab and an empty list move nothing', () => {
    expect(nextTabId(IDS, 'models', 'Enter')).toBeNull();
    expect(nextTabId(IDS, 'models', 'ArrowUp')).toBeNull();
    expect(nextTabId(IDS, 'nope', 'ArrowRight')).toBeNull();
    expect(nextTabId([], 'models', 'ArrowRight')).toBeNull();
  });

  it('a single tab stays put', () => {
    expect(nextTabId(['only'], 'only', 'ArrowRight')).toBe('only');
    expect(nextTabId(['only'], 'only', 'ArrowLeft')).toBe('only');
  });
});

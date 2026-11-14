import { describe, expect, it } from 'vitest';
import { APPROVAL_WINDOW_MS, approvalExpired, remainingLabel } from './approvalTime';

describe('approvalTime', () => {
  it('waits two minutes', () => {
    expect(APPROVAL_WINDOW_MS).toBe(120_000);
  });

  it('labels the remaining time as m:ss', () => {
    expect(remainingLabel(120_000, 0)).toBe('expires in 2:00');
    expect(remainingLabel(90_000, 0)).toBe('expires in 1:30');
    expect(remainingLabel(5_000, 0)).toBe('expires in 0:05');
    expect(remainingLabel(61_000, 0)).toBe('expires in 1:01');
  });

  it('says expired once the time is up', () => {
    expect(remainingLabel(1_000, 1_000)).toBe('expired');
    expect(remainingLabel(1_000, 2_000)).toBe('expired');
  });

  it('reports whether the countdown has run out', () => {
    expect(approvalExpired(1_000, 999)).toBe(false);
    expect(approvalExpired(1_000, 1_000)).toBe(true);
    expect(approvalExpired(1_000, 2_000)).toBe(true);
  });
});

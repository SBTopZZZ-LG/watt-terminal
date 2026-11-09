import { describe, expect, it } from 'vitest';
import { createOverflowGuard, MAX_SELF_CHECK_REFITS, overflowsHost } from './gridFit';
import type { GridMeasure } from './gridFit';

describe('overflowsHost', () => {
  it('is false when the grid ends at the host edge minus the padding', () => {
    expect(overflowsHost(900 - 12, 900, 12)).toBe(false);
  });

  it('is false when the grid stops short of the padding', () => {
    expect(overflowsHost(800, 900, 12)).toBe(false);
  });

  it('is true when the grid reaches into the right padding', () => {
    expect(overflowsHost(888.5, 900, 12)).toBe(true);
  });

  it('is true when the grid runs past the host (the clipped case, 17-35 px)', () => {
    expect(overflowsHost(900 + 17, 900, 12)).toBe(true);
    expect(overflowsHost(900 + 35, 900, 12)).toBe(true);
  });

  it('uses the padding it is given', () => {
    expect(overflowsHost(895, 900, 0)).toBe(false);
    expect(overflowsHost(895, 900, 10)).toBe(true);
  });
});

interface Fake {
  /** The measurement the guard sees. Tests change this to simulate fits that do or do not fix the grid. */
  m: GridMeasure | null;
  refits: number;
  gaveUp: GridMeasure[];
}

function fake(overflowing: boolean, fixOnRefit: number | null = null): Fake & { check: () => Promise<void> } {
  const over: GridMeasure = { screenRight: 920, hostRight: 900, padRight: 12 };
  const fine: GridMeasure = { screenRight: 800, hostRight: 900, padRight: 12 };
  const f: Fake & { check: () => Promise<void> } = {
    m: overflowing ? over : fine,
    refits: 0,
    gaveUp: [],
    check: null as unknown as () => Promise<void>,
  };
  f.check = createOverflowGuard({
    measure: () => f.m,
    refit: async () => {
      f.refits += 1;
      if (fixOnRefit !== null && f.refits === fixOnRefit) {
        f.m = fine;
      }
    },
    onGiveUp: (m) => f.gaveUp.push(m),
  });
  return f;
}

describe('createOverflowGuard', () => {
  it('does nothing when the grid fits', async () => {
    const f = fake(false);
    await f.check();
    expect(f.refits).toBe(0);
    expect(f.gaveUp).toHaveLength(0);
  });

  it('does nothing when nothing is on screen to measure', async () => {
    const f = fake(false);
    f.m = null;
    await f.check();
    expect(f.refits).toBe(0);
  });

  it('refits once and stops when one refit fixes the grid', async () => {
    const f = fake(true, 1);
    await f.check();
    expect(f.refits).toBe(1);
    expect(f.gaveUp).toHaveLength(0);
  });

  it('gives up after the cap when the grid never fits, and warns only once', async () => {
    const f = fake(true);
    await f.check();
    expect(f.refits).toBe(MAX_SELF_CHECK_REFITS);
    expect(f.gaveUp).toHaveLength(1);

    await f.check();
    await f.check();
    expect(f.refits).toBe(MAX_SELF_CHECK_REFITS);
    expect(f.gaveUp).toHaveLength(1);
  });

  it('starts a new streak after a clean check', async () => {
    const f = fake(true);
    await f.check();
    expect(f.refits).toBe(MAX_SELF_CHECK_REFITS);

    f.m = { screenRight: 800, hostRight: 900, padRight: 12 };
    await f.check();
    f.m = { screenRight: 920, hostRight: 900, padRight: 12 };
    await f.check();
    expect(f.refits).toBe(MAX_SELF_CHECK_REFITS * 2);
    expect(f.gaveUp).toHaveLength(2);
  });
});

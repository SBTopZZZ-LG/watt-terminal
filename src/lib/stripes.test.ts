import { describe, expect, it } from 'vitest';
import { badgeOnSeparator, separatorSegments, separatorTop, stripeSegments } from './stripes';
import type { StripeMark } from './stripes';

function mark(start: number, end: number, extra: Partial<StripeMark> = {}): StripeMark {
  return { start, end, exit: 'ok', agent: false, separatorRow: null, ...extra };
}

describe('stripeSegments', () => {
  it('a mark inside the viewport covers its rows, relative to the viewport top', () => {
    const [seg] = stripeSegments([mark(12, 14)], 10, 24);
    expect(seg).toEqual({ firstRow: 2, rowCount: 3, exit: 'ok', agent: false, startVisible: true, separatorRow: null });
  });

  it('a mark whose first row scrolled above the viewport stays visible on its remaining rows', () => {
    // seq 1 500 style: the command runs rows 0 to 499, the viewport shows rows 300 to 323.
    const [seg] = stripeSegments([mark(0, 499)], 300, 24);
    expect(seg).toEqual({
      firstRow: 0,
      rowCount: 24,
      exit: 'ok',
      agent: false,
      startVisible: false,
      separatorRow: null,
    });
  });

  it('a mark that starts above and ends below the viewport covers the whole viewport', () => {
    const [seg] = stripeSegments([mark(0, 1000)], 500, 24);
    expect(seg.firstRow).toBe(0);
    expect(seg.rowCount).toBe(24);
    expect(seg.startVisible).toBe(false);
  });

  it('a mark that ends below the viewport is cut at the bottom row', () => {
    const [seg] = stripeSegments([mark(20, 100)], 10, 24);
    expect(seg).toMatchObject({ firstRow: 10, rowCount: 14, startVisible: true });
  });

  it('a mark that ends in the viewport top row is a one-row stripe on that row', () => {
    const [seg] = stripeSegments([mark(0, 10)], 10, 24);
    expect(seg).toMatchObject({ firstRow: 0, rowCount: 1, startVisible: false });
  });

  it('a mark that starts on the last viewport row is a one-row stripe there', () => {
    const [seg] = stripeSegments([mark(33, 40)], 10, 24);
    expect(seg).toMatchObject({ firstRow: 23, rowCount: 1, startVisible: true });
  });

  it('a mark that just reaches into the viewport from above shows one row, and one just below shows nothing', () => {
    const [seg] = stripeSegments([mark(9, 10)], 10, 24);
    expect(seg).toMatchObject({ firstRow: 0, rowCount: 1, startVisible: false });
    expect(stripeSegments([mark(34, 34)], 10, 24)).toEqual([]);
  });

  it('a mark entirely above or entirely below the viewport gives no segment', () => {
    expect(stripeSegments([mark(0, 9)], 10, 24)).toEqual([]);
    expect(stripeSegments([mark(34, 50)], 10, 24)).toEqual([]);
  });

  it('a mark that starts at -1 (trimmed) and ends inside the viewport is not marked as starting visibly', () => {
    const [seg] = stripeSegments([mark(-1, 12)], 10, 24);
    expect(seg).toEqual({
      firstRow: 0,
      rowCount: 3,
      exit: 'ok',
      agent: false,
      startVisible: false,
      separatorRow: null,
    });
  });

  it('a mark whose end is before its start gives no segment', () => {
    expect(stripeSegments([mark(12, 11)], 10, 24)).toEqual([]);
  });

  it('keeps exit and agent, and returns one segment per visible mark in order', () => {
    const segs = stripeSegments(
      [mark(10, 10, { exit: 'running', agent: true }), mark(11, 12, { exit: 'fail' }), mark(50, 60)],
      10,
      24,
    );
    expect(segs).toEqual([
      { firstRow: 0, rowCount: 1, exit: 'running', agent: true, startVisible: true, separatorRow: null },
      { firstRow: 1, rowCount: 2, exit: 'fail', agent: false, startVisible: true, separatorRow: null },
    ]);
  });

  it('gives nothing for an empty list or a zero-row viewport', () => {
    expect(stripeSegments([], 0, 24)).toEqual([]);
    expect(stripeSegments([mark(0, 5)], 0, 0)).toEqual([]);
  });

  it('passes the separator row through, even when it is outside the viewport', () => {
    const [seg] = stripeSegments([mark(12, 14, { separatorRow: 40 })], 10, 24);
    expect(seg.separatorRow).toBe(40);
  });
});

describe('badgeOnSeparator', () => {
  const blank = () => true;
  const notBlank = () => false;

  it('a command with no separator keeps its badge on the stripe', () => {
    expect(badgeOnSeparator(null, blank)).toBe(false);
  });

  it('a separator on a blank row takes the badge', () => {
    expect(badgeOnSeparator(12, blank)).toBe(true);
  });

  it('a separator on a row that is not blank leaves the badge on the stripe', () => {
    expect(badgeOnSeparator(12, notBlank)).toBe(false);
  });

  it('checks the absolute row, so a separator out of view still takes the badge', () => {
    const checked: number[] = [];
    const isBlank = (row: number) => {
      checked.push(row);
      return true;
    };
    // The viewport is rows 10 to 33, the separator is at row 40: the answer is the same as if it were on screen.
    expect(badgeOnSeparator(40, isBlank)).toBe(true);
    expect(checked).toEqual([40]);
  });
});

describe('separatorSegments', () => {
  const at = (row: number, agent = false) => ({ row, agent });

  it('a row inside the viewport gives its row relative to the viewport top', () => {
    expect(separatorSegments([at(12)], 10, 24)).toEqual([at(2)]);
  });

  it('rows above the viewport give nothing', () => {
    expect(separatorSegments([at(0), at(9)], 10, 24)).toEqual([]);
  });

  it('rows below the viewport give nothing', () => {
    expect(separatorSegments([at(34), at(50)], 10, 24)).toEqual([]);
  });

  it('the first and last viewport rows are included', () => {
    expect(separatorSegments([at(10), at(33)], 10, 24)).toEqual([at(0), at(23)]);
  });

  it('an empty list or a zero-row viewport gives nothing', () => {
    expect(separatorSegments([], 0, 24)).toEqual([]);
    expect(separatorSegments([at(5)], 0, 0)).toEqual([]);
  });

  it('keeps the order given, and drops only the rows outside', () => {
    expect(separatorSegments([at(30), at(5), at(12), at(40), at(10)], 10, 24)).toEqual([
      at(20),
      at(2),
      at(0),
    ]);
  });

  it('keeps the agent flag of each line', () => {
    expect(separatorSegments([at(12, true), at(14, false)], 10, 24)).toEqual([at(2, true), at(4, false)]);
  });
});

describe('separatorTop', () => {
  it('centred in a blank row: the middle of the row, less half the 1px line', () => {
    expect(separatorTop(3, 20, true)).toBe(60 + 10 - 0.5);
  });

  it('not centred: the top edge of the row', () => {
    expect(separatorTop(3, 20, false)).toBe(60);
  });

  it('row 0 centred sits half a cell down, less half the line', () => {
    expect(separatorTop(0, 17, true)).toBe(8.5 - 0.5);
  });

  it('a fractional cell height keeps the centre exact', () => {
    expect(separatorTop(2, 15.5, true)).toBeCloseTo(31 + 7.75 - 0.5, 10);
  });
});

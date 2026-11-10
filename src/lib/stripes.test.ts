import { describe, expect, it } from 'vitest';
import { separatorSegments, separatorTop, stripeSegments } from './stripes';
import type { StripeMark } from './stripes';

function mark(start: number, end: number, extra: Partial<StripeMark> = {}): StripeMark {
  return { start, end, exit: 'ok', agent: false, ...extra };
}

describe('stripeSegments', () => {
  it('a mark inside the viewport covers its rows, relative to the viewport top', () => {
    const [seg] = stripeSegments([mark(12, 14)], 10, 24);
    expect(seg).toEqual({ firstRow: 2, rowCount: 3, exit: 'ok', agent: false, startVisible: true });
  });

  it('a mark whose first row scrolled above the viewport stays visible on its remaining rows', () => {
    // seq 1 500 style: the command runs rows 0 to 499, the viewport shows rows 300 to 323.
    const [seg] = stripeSegments([mark(0, 499)], 300, 24);
    expect(seg).toEqual({ firstRow: 0, rowCount: 24, exit: 'ok', agent: false, startVisible: false });
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
    expect(seg).toEqual({ firstRow: 0, rowCount: 3, exit: 'ok', agent: false, startVisible: false });
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
      { firstRow: 0, rowCount: 1, exit: 'running', agent: true, startVisible: true },
      { firstRow: 1, rowCount: 2, exit: 'fail', agent: false, startVisible: true },
    ]);
  });

  it('gives nothing for an empty list or a zero-row viewport', () => {
    expect(stripeSegments([], 0, 24)).toEqual([]);
    expect(stripeSegments([mark(0, 5)], 0, 0)).toEqual([]);
  });
});

describe('separatorSegments', () => {
  it('a row inside the viewport gives its row relative to the viewport top', () => {
    expect(separatorSegments([12], 10, 24)).toEqual([2]);
  });

  it('rows above the viewport give nothing', () => {
    expect(separatorSegments([0, 9], 10, 24)).toEqual([]);
  });

  it('rows below the viewport give nothing', () => {
    expect(separatorSegments([34, 50], 10, 24)).toEqual([]);
  });

  it('the first and last viewport rows are included', () => {
    expect(separatorSegments([10, 33], 10, 24)).toEqual([0, 23]);
  });

  it('an empty list or a zero-row viewport gives nothing', () => {
    expect(separatorSegments([], 0, 24)).toEqual([]);
    expect(separatorSegments([5], 0, 0)).toEqual([]);
  });

  it('keeps the order given, and drops only the rows outside', () => {
    expect(separatorSegments([30, 5, 12, 40, 10], 10, 24)).toEqual([20, 2, 0]);
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

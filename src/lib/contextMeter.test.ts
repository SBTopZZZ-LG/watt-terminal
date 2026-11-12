import { describe, expect, it } from 'vitest';
import { DETAIL_MAX_PX, detailMaxHeight, formatTokens, groupDigits, headline, labelOf, level, percent, rows, segments, share } from './contextMeter';
import type { ContextUsage } from './types';

const usage = (over: Partial<ContextUsage> = {}): ContextUsage => ({
  limit: 8000,
  used: 1234,
  reserved_output: 4096,
  trimmed: false,
  calibrated: false,
  categories: [
    { id: 'system', tokens: 1000 },
    { id: 'user', tokens: 0 },
    { id: 'attachments', tokens: 234 },
  ],
  ...over,
});

describe('formatTokens', () => {
  it.each([
    [0, '0'],
    [850, '850'],
    [999, '999'],
    [1000, '1k'],
    [1234, '1.2k'],
    [9999, '10k'],
    [48213, '48k'],
    [128000, '128k'],
    [1_200_000, '1.2M'],
    [999_999, '1M'],
  ])('%i reads as %s', (n, text) => {
    expect(formatTokens(n)).toBe(text);
  });

  it('counts a negative or non-finite value as zero', () => {
    expect(formatTokens(-5)).toBe('0');
    expect(formatTokens(Number.NaN)).toBe('0');
    expect(formatTokens(Number.POSITIVE_INFINITY)).toBe('0');
  });
});

describe('groupDigits', () => {
  it.each([
    [0, '0'],
    [999, '999'],
    [48213, '48,213'],
    [128000, '128,000'],
    [1_000_000, '1,000,000'],
  ])('%i groups as %s', (n, text) => {
    expect(groupDigits(n)).toBe(text);
  });
});

describe('percent and share', () => {
  it('rounds to a whole percent', () => {
    expect(percent(37, 100)).toBe('37%');
    expect(percent(2, 3)).toBe('67%');
    expect(percent(100, 100)).toBe('100%');
  });

  it('is 0% for nothing, <1% for a little, and rounds from half a percent up', () => {
    expect(percent(0, 100)).toBe('0%');
    expect(percent(1, 1000)).toBe('<1%');
    expect(percent(5, 1000)).toBe('1%');
    expect(percent(10, 0)).toBe('0%');
  });

  it('share is the unrounded percentage, and 0 for a whole that is not positive', () => {
    expect(share(1, 4)).toBe(25);
    expect(share(3, 0)).toBe(0);
  });
});

describe('level', () => {
  const at = (used: number, reserved = 0, limit: number | null = 100) => level({ limit, used, reserved_output: reserved });

  it('is unknown when no limit is known, or the limit is not positive', () => {
    expect(at(50, 0, null)).toBe('unknown');
    expect(at(50, 0, 0)).toBe('unknown');
  });

  it('is ok below 70%, warn from 70 to under 85, high from 85', () => {
    expect(at(0)).toBe('ok');
    expect(at(69)).toBe('ok');
    expect(at(70)).toBe('warn');
    expect(at(84)).toBe('warn');
    expect(at(85)).toBe('high');
    expect(at(100)).toBe('high');
  });

  it('is high when the used tokens and the reply reserve do not fit, even below 85% used', () => {
    expect(at(50, 60)).toBe('high');
    expect(at(50, 50)).toBe('ok');
  });

  it('uses exact boundaries on a large limit, too', () => {
    // 70% of 128000 is 89600, 85% is 108800.
    expect(level({ limit: 128000, used: 89599, reserved_output: 0 })).toBe('ok');
    expect(level({ limit: 128000, used: 89600, reserved_output: 0 })).toBe('warn');
    expect(level({ limit: 128000, used: 108799, reserved_output: 0 })).toBe('warn');
    expect(level({ limit: 128000, used: 108800, reserved_output: 0 })).toBe('high');
  });
});

describe('labelOf', () => {
  it('names the known categories', () => {
    expect(labelOf('system')).toBe('System instructions');
    expect(labelOf('tools')).toBe('Tool definitions');
    expect(labelOf('user')).toBe('Your messages');
    expect(labelOf('attachments')).toBe('Attachments');
    expect(labelOf('assistant')).toBe('Agent messages');
    expect(labelOf('tool_output')).toBe('Tool output');
    expect(labelOf('other')).toBe('Other');
  });

  it('shows an unknown id as its own text, with control characters taken out', () => {
    expect(labelOf('reasoning')).toBe('reasoning');
    expect(labelOf('a\u0007b')).toBe('a b');
  });

  it('does not take an inherited property name for a label', () => {
    expect(labelOf('constructor')).toBe('constructor');
    expect(labelOf('__proto__')).toBe('__proto__');
  });
});

describe('headline', () => {
  it('gives the used tokens, the limit and the percentage', () => {
    expect(headline(usage({ used: 48213, limit: 128000 }))).toBe('Context: 48,213 of 128,000 tokens (38%)');
  });

  it('gives the used tokens alone when the limit is not known', () => {
    expect(headline(usage({ used: 48213, limit: null }))).toBe('Context: 48,213 tokens (limit unknown)');
  });
});

describe('rows', () => {
  it('lists the categories with tokens in the backend order, then Free and Reserved, with percentages of the limit', () => {
    expect(rows(usage())).toEqual([
      { kind: 'category', id: 'system', label: 'System instructions', tokens: 1000, percent: '13%', slot: 0 },
      { kind: 'category', id: 'attachments', label: 'Attachments', tokens: 234, percent: '3%', slot: 1 },
      { kind: 'free', id: 'free', label: 'Free', tokens: 2670, percent: '33%', slot: null },
      { kind: 'reserved', id: 'reserved', label: 'Reserved for reply', tokens: 4096, percent: '51%', slot: null },
    ]);
  });

  it('hides the zero-token categories', () => {
    expect(rows(usage()).some((r) => r.id === 'user')).toBe(false);
  });

  it('gives no Free or Reserved rows, and percentages of the used tokens, when the limit is not known', () => {
    const r = rows(usage({ limit: null }));
    expect(r.map((x) => x.kind)).toEqual(['category', 'category']);
    expect(r.map((x) => x.percent)).toEqual(['81%', '19%']);
  });

  it('free is never negative', () => {
    const free = rows(usage({ used: 7900, reserved_output: 4096 })).find((r) => r.kind === 'free');
    expect(free?.tokens).toBe(0);
  });

  it('shows an unknown id with its id as the label', () => {
    const r = rows(usage({ categories: [{ id: 'reasoning', tokens: 5 }] }));
    expect(r[0]).toMatchObject({ id: 'reasoning', label: 'reasoning', slot: 0 });
  });
});

describe('segments', () => {
  it('takes each category\'s share of the limit, in the rows\' order', () => {
    const s = segments(usage());
    expect(s.map((x) => x.id)).toEqual(['system', 'attachments']);
    expect(s[0].share).toBeCloseTo(12.5, 6);
    expect(s[1].share).toBeCloseTo(2.925, 6);
  });

  it('scales the shares down when they would add up to more than 100 (used above the limit)', () => {
    const s = segments(usage({ limit: 8000, used: 12000, categories: [{ id: 'system', tokens: 9000 }, { id: 'user', tokens: 3000 }] }));
    expect(s.map((x) => x.share)).toEqual([expect.closeTo(75, 6), expect.closeTo(25, 6)]);
    expect(s.reduce((sum, x) => sum + x.share, 0)).toBeLessThanOrEqual(100 + 1e-9);
  });

  it('takes shares of the used tokens when the limit is not known, summing to 100', () => {
    const s = segments(usage({ limit: null, used: 4, categories: [{ id: 'system', tokens: 3 }, { id: 'user', tokens: 1 }] }));
    expect(s.map((x) => x.share)).toEqual([75, 25]);
  });
});

describe('detailMaxHeight', () => {
  it('is 60% of the agent panel, at most 240px', () => {
    expect(detailMaxHeight(300)).toBe(180);
    expect(detailMaxHeight(100)).toBe(60);
    expect(detailMaxHeight(600)).toBe(DETAIL_MAX_PX);
    expect(DETAIL_MAX_PX).toBe(240);
  });

  it('is 240px before the panel is measured', () => {
    expect(detailMaxHeight(0)).toBe(240);
    expect(detailMaxHeight(Number.NaN)).toBe(240);
    expect(detailMaxHeight(-5)).toBe(240);
  });
});

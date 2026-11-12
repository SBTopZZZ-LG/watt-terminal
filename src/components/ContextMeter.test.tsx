import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { ContextBreakdown, ContextMeter } from './ContextMeter';
import type { ContextUsage } from '../lib/types';

const usage = (over: Partial<ContextUsage> = {}): ContextUsage => ({
  limit: 8000,
  used: 1234,
  reserved_output: 4096,
  trimmed: false,
  calibrated: false,
  categories: [
    { id: 'system', tokens: 1000 },
    { id: 'attachments', tokens: 234 },
  ],
  ...over,
});
const meter = (u: ContextUsage | null) => renderToStaticMarkup(<ContextMeter usage={u} />);
const breakdown = (u: ContextUsage) => renderToStaticMarkup(<ContextBreakdown usage={u} />);

describe('ContextMeter: hidden without a value', () => {
  it('renders nothing when there is no usage', () => {
    expect(meter(null)).toBe('');
  });
});

describe('ContextMeter: collapsed', () => {
  it('is a real button, collapsed, that controls the breakdown by id', () => {
    const out = meter(usage());
    expect(out).toMatch(/<button type="button" class="context-meter is-ok"/);
    expect(out).toContain('aria-expanded="false"');
    const controls = /aria-controls="([^"]+)"/.exec(out)?.[1];
    expect(controls).toBeTruthy();
    expect(out).toContain(`id="${controls}"`);
  });

  it('the breakdown is in the DOM but hidden while collapsed', () => {
    expect(meter(usage())).toMatch(/class="context-detail" role="region"[^>]*hidden=""/);
  });

  it('names the state in the accessible name: the percentage, and the tokens of the limit', () => {
    expect(meter(usage())).toContain('aria-label="Context: 15% used, 1.2k of 8k tokens"');
  });

  it('shows the percentage, the figures (CSS shows them only in a wide panel) and a meter with its values', () => {
    const out = meter(usage());
    expect(out).toContain('<span class="context-meter-percent">15%</span>');
    expect(out).toContain('<span class="context-meter-figures">1.2k / 8k</span>');
    expect(out).toMatch(/role="meter" aria-label="Context used" aria-valuemin="0" aria-valuemax="100" aria-valuenow="15"/);
    expect(out).toContain('<span class="context-meter-fill" style="width:15.425%"></span>');
  });

  it('the level colours the meter: warn from 70%, high from 85%', () => {
    // No reserve here: with the default reserve of 4096, 5600 used would not fit 8000 and the level is high on that rule alone.
    expect(meter(usage({ used: 5600, reserved_output: 0 }))).toMatch(/class="context-meter is-warn"/);
    expect(meter(usage({ used: 6800, reserved_output: 0 }))).toMatch(/class="context-meter is-high"/);
    expect(meter(usage({ used: 5600 }))).toMatch(/class="context-meter is-high"/);
  });

  it('with no limit: no meter role, no fill, the used tokens alone, and a title saying the limit is unknown', () => {
    const out = meter(usage({ limit: null, used: 48213 }));
    expect(out).toMatch(/class="context-meter is-unknown"/);
    expect(out).not.toContain('role="meter"');
    expect(out).not.toContain('context-meter-fill');
    expect(out).not.toContain('context-meter-figures');
    expect(out).toContain('<span class="context-meter-percent">48k</span>');
    expect(out).toContain('aria-label="Context: 48k tokens used, limit unknown"');
    expect(out).toContain('title="Context limit unknown. Show the details"');
  });
});

describe('ContextBreakdown: the expanded view', () => {
  it('gives the headline, then the basis: the estimate or the provider count', () => {
    expect(breakdown(usage())).toContain('<p class="context-headline">Context: 1,234 of 8,000 tokens (15%)</p>');
    expect(breakdown(usage())).toContain('<p class="context-basis">Estimated</p>');
    expect(breakdown(usage({ calibrated: true }))).toContain("<p class=\"context-basis\">Provider&#x27;s count</p>");
  });

  it('draws the stacked bar with one segment per category, in colour slots, and a row for each', () => {
    const out = breakdown(usage());
    expect(out).toContain('class="context-seg context-seg-0" style="width:12.5%"');
    expect(out).toContain('class="context-seg context-seg-3"');
    expect(out).toContain('<span class="context-label">System instructions</span><span class="context-tokens">1,000</span><span class="context-percent">13%</span>');
    expect(out).toContain('<span class="context-label">Attachments</span><span class="context-tokens">234</span><span class="context-percent">3%</span>');
  });

  it('adds Free and Reserved for the reply when the limit is known', () => {
    const out = breakdown(usage());
    expect(out).toContain('context-row context-row-free');
    expect(out).toContain('<span class="context-label">Free</span><span class="context-tokens">2,670</span><span class="context-percent">33%</span>');
    expect(out).toContain('context-row context-row-reserved');
    expect(out).toContain('<span class="context-label">Reserved for reply</span><span class="context-tokens">4,096</span><span class="context-percent">51%</span>');
  });

  it('with no limit: no stacked bar, no Free or Reserved row, and the headline says the limit is unknown', () => {
    const out = breakdown(usage({ limit: null }));
    expect(out).not.toContain('context-stack');
    expect(out).not.toContain('context-row-free');
    expect(out).not.toContain('context-row-reserved');
    expect(out).toContain('Context: 1,234 tokens (limit unknown)');
  });

  it('says so when older content was left out, and not otherwise', () => {
    expect(breakdown(usage({ trimmed: true }))).toContain('Older content was left out of the last request.');
    expect(breakdown(usage({ trimmed: false }))).not.toContain('Older content');
  });

  it('puts the trim line directly under the headline, above the basis and the bar', () => {
    const out = breakdown(usage({ trimmed: true }));
    const headlineAt = out.indexOf('class="context-headline"');
    const trimAt = out.indexOf('class="context-trimmed"');
    expect(headlineAt).toBeGreaterThanOrEqual(0);
    expect(trimAt).toBeGreaterThan(headlineAt);
    expect(trimAt).toBeLessThan(out.indexOf('class="context-basis"'));
    expect(trimAt).toBeLessThan(out.indexOf('class="context-stack"'));
  });

  it('shows an unknown category by its id, escaped as text, never as markup', () => {
    const out = breakdown(usage({ categories: [{ id: '<img src=x onerror=1>', tokens: 10 }] }));
    expect(out).not.toContain('<img');
    expect(out).toContain('&lt;img src=x onerror=1&gt;');
  });

  it('shows a long unknown id in full (the CSS wraps and ellipsises, the text is not cut)', () => {
    const long = 'x'.repeat(300);
    const out = breakdown(usage({ categories: [{ id: long, tokens: 10 }] }));
    expect(out).toContain(`<span class="context-label">${long}</span>`);
  });

  it('a zero-token category gets no row', () => {
    expect(breakdown(usage({ categories: [{ id: 'user', tokens: 0 }, { id: 'system', tokens: 5 }] }))).not.toContain('Your messages');
  });
});

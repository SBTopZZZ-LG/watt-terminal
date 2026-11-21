import { describe, expect, it } from 'vitest';
import {
  TIP_DELAY_MS,
  TIP_GAP,
  TIP_MARGIN,
  TIP_MAX_CHARS,
  TIP_REOPEN_MS,
  capText,
  isClipped,
  placement,
  resolveTip,
  showDelay,
} from './tooltip';
import type { TipNode } from './tooltip';

/** A fake element: attributes, a class list, and a parent. Enough for resolveTip, which reads nothing else. */
function node(attrs: Record<string, string>, parent: TipNode | null = null, tagName = 'SPAN'): TipNode {
  return {
    tagName,
    getAttribute: (name: string) => (name in attrs ? attrs[name] : null),
    parentElement: parent,
  };
}

const NEVER = () => false;
const ALWAYS = () => true;

describe('placement', () => {
  const viewport = { width: 800, height: 600 };
  const tip = { width: 200, height: 60 };

  it('sits below the anchor, centred on it, with the gap', () => {
    const anchor = { left: 300, top: 100, width: 100, height: 20 };
    expect(placement(anchor, tip, viewport)).toEqual({ x: 300 + 50 - 100, y: 120 + TIP_GAP, side: 'below' });
  });

  it('flips above when there is no room below and there is room above', () => {
    const anchor = { left: 300, top: 560, width: 100, height: 20 };
    const p = placement(anchor, tip, viewport);
    expect(p.side).toBe('above');
    expect(p.y).toBe(560 - TIP_GAP - 60);
  });

  it('stays below when there is no room on either side, clamped to the viewport', () => {
    const anchor = { left: 300, top: 290, width: 100, height: 20 };
    const short = { width: 200, height: 400 };
    const p = placement(anchor, short, { width: 800, height: 600 });
    expect(p.side).toBe('below');
    expect(p.y).toBe(600 - TIP_MARGIN - 400);
  });

  it('clamps horizontally at both edges', () => {
    expect(placement({ left: 0, top: 100, width: 10, height: 10 }, tip, viewport).x).toBe(TIP_MARGIN);
    expect(placement({ left: 790, top: 100, width: 10, height: 10 }, tip, viewport).x).toBe(800 - TIP_MARGIN - 200);
  });

  it('a tip wider than the viewport sits at the left margin', () => {
    expect(placement({ left: 400, top: 100, width: 10, height: 10 }, { width: 1000, height: 40 }, viewport).x).toBe(TIP_MARGIN);
  });
});

describe('isClipped', () => {
  const fits = { scrollWidth: 100, clientWidth: 100, scrollHeight: 40, clientHeight: 40 };

  it('is false when the content fits', () => {
    expect(isClipped(fits)).toBe(false);
  });

  it('is true when it overflows sideways or downwards', () => {
    expect(isClipped({ ...fits, scrollWidth: 140 })).toBe(true);
    expect(isClipped({ ...fits, scrollHeight: 90 })).toBe(true);
  });

  it('allows 1px of rounding', () => {
    expect(isClipped({ ...fits, scrollWidth: 101 })).toBe(false);
    expect(isClipped({ ...fits, scrollHeight: 41 })).toBe(false);
    expect(isClipped({ ...fits, scrollWidth: 102 })).toBe(true);
  });
});

describe('resolveTip', () => {
  it('returns the element own data-tip', () => {
    const el = node({ 'data-tip': 'Full text' });
    expect(resolveTip(el, NEVER)).toEqual({ anchor: el, text: 'Full text', requireClip: false });
  });

  it('walks up to the nearest ancestor with data-tip', () => {
    const outer = node({ 'data-tip': 'outer' });
    const inner = node({ 'data-tip': 'inner' }, outer);
    const leaf = node({}, inner);
    expect(resolveTip(leaf, NEVER)?.text).toBe('inner');
    expect(resolveTip(node({}, outer), NEVER)?.text).toBe('outer');
  });

  it('returns null when no ancestor has data-tip', () => {
    expect(resolveTip(node({}, node({ title: 'native' })), NEVER)).toBeNull();
  });

  it('marks a data-tip-clip tip as requiring a clip', () => {
    const el = node({ 'data-tip': 'x', 'data-tip-clip': '' });
    expect(resolveTip(el, ALWAYS)?.requireClip).toBe(true);
  });

  it('skips a data-tip-clip tip that is not clipped, so the next tip up applies', () => {
    const outer = node({ 'data-tip': 'whole row' });
    const el = node({ 'data-tip': 'title', 'data-tip-clip': '' }, outer);
    expect(resolveTip(el, NEVER)?.text).toBe('whole row');
    expect(resolveTip(el, ALWAYS)?.text).toBe('title');
  });

  it('gives the clip check the node it is about', () => {
    const el = node({ 'data-tip': 'x', 'data-tip-clip': '' });
    const seen: TipNode[] = [];
    resolveTip(el, (n) => (seen.push(n), true));
    expect(seen).toEqual([el]);
  });

  it('ignores typing targets', () => {
    expect(resolveTip(node({ 'data-tip': 'x' }, null, 'INPUT'), ALWAYS)).toBeNull();
    expect(resolveTip(node({ 'data-tip': 'x' }, null, 'TEXTAREA'), ALWAYS)).toBeNull();
  });

  it('ignores everything inside the terminal', () => {
    const term = node({ class: 'xterm-screen xterm' });
    const inside = node({ 'data-tip': 'x' }, term);
    expect(resolveTip(inside, ALWAYS)).toBeNull();
    expect(resolveTip(node({}, node({ class: 'xterm-screen xterm' })), ALWAYS)).toBeNull();
  });

  it('treats an empty or blank data-tip as no tip', () => {
    expect(resolveTip(node({ 'data-tip': '' }), ALWAYS)).toBeNull();
    expect(resolveTip(node({ 'data-tip': '   ' }), ALWAYS)).toBeNull();
  });

  it('caps the text', () => {
    const long = 'a'.repeat(TIP_MAX_CHARS + 50);
    expect(resolveTip(node({ 'data-tip': long }), ALWAYS)?.text).toBe(`${'a'.repeat(TIP_MAX_CHARS - 1)}…`);
  });
});

describe('capText', () => {
  it('keeps a text at or under the cap as it is', () => {
    expect(capText('short')).toBe('short');
    expect(capText('abc', 3)).toBe('abc');
  });

  it('cuts by characters, not UTF-16 units, and marks the cut', () => {
    expect(capText('abcd', 3)).toBe('ab…');
    expect(capText('😀😀😀😀', 3)).toBe('😀😀…');
  });
});

describe('showDelay', () => {
  it('waits the normal delay when no tip hid recently', () => {
    expect(showDelay(1000, null)).toBe(TIP_DELAY_MS);
    expect(showDelay(1000 + TIP_REOPEN_MS + 1, 1000)).toBe(TIP_DELAY_MS);
  });

  it('shows at once when the last tip hid within the reopen window', () => {
    expect(showDelay(1000, 1000 - TIP_REOPEN_MS)).toBe(0);
    expect(showDelay(1010, 1000)).toBe(0);
  });
});

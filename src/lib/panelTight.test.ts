import { describe, expect, it } from 'vitest';
import { settleTight, TIGHT_ATTR } from './panelTight';

/** A stand-in for the agent panel's element: the attributes, classes and sizes settleTight reads and writes. */
function panel({ sized = true, scrollH = 320, clientH = 320, tight = false } = {}) {
  const attrs = new Set<string>(tight ? [TIGHT_ATTR] : []);
  const classes = new Set<string>(sized ? ['agent-panel', 'is-sized'] : ['agent-panel']);
  return {
    scrollHeight: scrollH,
    clientHeight: clientH,
    classList: { contains: (c: string) => classes.has(c) },
    hasAttribute: (a: string) => attrs.has(a),
    setAttribute: (a: string) => attrs.add(a),
    removeAttribute: (a: string) => attrs.delete(a),
    attrs,
  };
}
const asEl = (p: object) => p as unknown as HTMLElement;

describe('settleTight: the agent panel is tight when a sized panel overflows its box', () => {
  it('sets the flag when a sized panel overflows by more than 1.5px', () => {
    const p = panel({ scrollH: 360, clientH: 320 });
    settleTight(asEl(p));
    expect(p.hasAttribute(TIGHT_ATTR)).toBe(true);
  });

  it('clears the flag when the panel has room again', () => {
    const p = panel({ scrollH: 320, clientH: 320, tight: true });
    settleTight(asEl(p));
    expect(p.hasAttribute(TIGHT_ATTR)).toBe(false);
  });

  it('ignores sub-pixel overflow (within the 1.5px threshold)', () => {
    const p = panel({ scrollH: 321.2, clientH: 320 });
    settleTight(asEl(p));
    expect(p.hasAttribute(TIGHT_ATTR)).toBe(false);
  });

  it('an auto panel is never tight, even when it overflows, and a stale flag is cleared', () => {
    const p = panel({ sized: false, scrollH: 400, clientH: 320, tight: true });
    settleTight(asEl(p));
    expect(p.hasAttribute(TIGHT_ATTR)).toBe(false);
  });

  it('no panel: nothing happens', () => {
    expect(() => settleTight(null)).not.toThrow();
  });
});

/**
 * A panel and a kept body together. The body's scrollTop is clamped to a max that depends on the flag (maxTight when set,
 * maxLoose when not). Reading a size or the scrollTop lays out, and a layout clamps the stored position for good, as the browser does.
 * `sets` records every scrollTop write.
 */
function scene({ sized = true, overflow = true, tight = true, top, maxTight, maxLoose }: {
  sized?: boolean;
  overflow?: boolean;
  tight?: boolean;
  top: number;
  maxTight: number;
  maxLoose: number;
}) {
  const attrs = new Set<string>(tight ? [TIGHT_ATTR] : []);
  const classes = new Set<string>(sized ? ['agent-panel', 'is-sized'] : ['agent-panel']);
  let stored = top;
  const sets: number[] = [];
  const max = () => (attrs.has(TIGHT_ATTR) ? maxTight : maxLoose);
  const layout = () => {
    stored = Math.min(stored, max());
  };
  const agent = {
    classList: { contains: (c: string) => classes.has(c) },
    hasAttribute: (a: string) => attrs.has(a),
    setAttribute: (a: string) => attrs.add(a),
    removeAttribute: (a: string) => attrs.delete(a),
    get scrollHeight() {
      layout();
      return overflow ? 330 : 320;
    },
    get clientHeight() {
      layout();
      return 320;
    },
  };
  const keep = {
    get scrollTop() {
      layout();
      return stored;
    },
    set scrollTop(v: number) {
      sets.push(v);
      stored = Math.min(v, max());
    },
  };
  return { agent, keep, sets, attrs };
}

describe('settleTight with a kept element: the scroll position survives the measure', () => {
  it('restores a position the measure clamped, when the panel ends tight', () => {
    // At the bottom (962) with the flag on; the flag-off measure clamps it to 919; the flag ends on, so 962 is allowed again.
    const s = scene({ overflow: true, top: 962, maxTight: 962, maxLoose: 919 });
    settleTight(asEl(s.agent), asEl(s.keep));
    expect(s.attrs.has(TIGHT_ATTR)).toBe(true);
    expect(s.keep.scrollTop).toBe(962);
  });

  it('never restores above the final max when the panel ends not tight', () => {
    const s = scene({ overflow: false, top: 962, maxTight: 962, maxLoose: 919 });
    settleTight(asEl(s.agent), asEl(s.keep));
    expect(s.attrs.has(TIGHT_ATTR)).toBe(false);
    expect(s.keep.scrollTop).toBe(919);
    expect(s.keep.scrollTop).toBeLessThanOrEqual(919);
    expect(s.sets).toEqual([962]);
  });

  it('restores on the not-sized path too, since the flag was removed there as well', () => {
    // A stale flag on an auto panel: the restore is written (962) and clamped to the final max (919), which is all the layout allows.
    const s = scene({ sized: false, top: 962, maxTight: 962, maxLoose: 919 });
    settleTight(asEl(s.agent), asEl(s.keep));
    expect(s.attrs.has(TIGHT_ATTR)).toBe(false);
    expect(s.sets).toEqual([962]);
    expect(s.keep.scrollTop).toBe(919);
  });

  it('a missing panel does nothing, and the kept element is not touched', () => {
    const s = scene({ top: 962, maxTight: 962, maxLoose: 919 });
    expect(() => settleTight(null, asEl(s.keep))).not.toThrow();
    expect(s.sets).toEqual([]);
  });

  it('without a kept element the body is not read or written', () => {
    const s = scene({ overflow: true, top: 962, maxTight: 962, maxLoose: 919 });
    settleTight(asEl(s.agent));
    expect(s.attrs.has(TIGHT_ATTR)).toBe(true);
    expect(s.sets).toEqual([]);
  });

  it('does not write when the position is unchanged', () => {
    const s = scene({ overflow: true, top: 500, maxTight: 962, maxLoose: 919 });
    settleTight(asEl(s.agent), asEl(s.keep));
    expect(s.sets).toEqual([]);
    expect(s.keep.scrollTop).toBe(500);
  });
});

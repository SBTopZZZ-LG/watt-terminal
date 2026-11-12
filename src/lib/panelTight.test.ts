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
const asEl = (p: ReturnType<typeof panel>) => p as unknown as HTMLElement;

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

import { describe, expect, it } from 'vitest';
import {
  DEFAULT_SIZES,
  DIVIDER_PX,
  PANEL_KEY,
  PANEL_LEGACY_KEY,
  PANEL_MIN,
  STORED_SIZE_MAX,
  TERMINAL_MIN,
  chatBodyShown,
  clampSize,
  defaultLayout,
  keyboardSize,
  layoutFromUnknown,
  loadPanelLayout,
  panelBounds,
  panelReducer,
  panelView,
  parsePanelLayout,
  resolvePanelSize,
  nextSizeFromDrag,
  sameLayout,
  savePanelLayout,
} from './panelLayout';
import type { Container, Dock, PanelLayout } from './panelLayout';
import type { SizeStorage } from './zoom';

/** A Map-backed storage. `throws` makes every call throw, like a denied localStorage. */
function fakeStorage(initial: Record<string, string> = {}, throws = false): SizeStorage & { data: Map<string, string> } {
  const data = new Map(Object.entries(initial));
  return {
    data,
    getItem(key) {
      if (throws) {
        throw new Error('denied');
      }
      return data.get(key) ?? null;
    },
    setItem(key, value) {
      if (throws) {
        throw new Error('quota');
      }
      data.set(key, value);
    },
  };
}

/** A workspace big enough that no clamp applies unless a test asks for it. */
const BIG: Container = { width: 1600, height: 1000 };

describe('defaults', () => {
  it('starts at the bottom dock, open, with 380px side panels and auto top and bottom', () => {
    expect(defaultLayout()).toEqual({
      dock: 'bottom',
      sizes: { left: 380, right: 380, top: null, bottom: null },
      open: true,
    });
    expect(DEFAULT_SIZES).toEqual({ left: 380, right: 380, top: null, bottom: null });
  });

  it('hands out a fresh object each time, so callers cannot change the shared default', () => {
    const a = defaultLayout();
    a.sizes.left = 999;
    expect(defaultLayout().sizes.left).toBe(380);
  });

  it('keeps the minimums the spec asks for', () => {
    expect(PANEL_MIN).toEqual({ side: 240, vertical: 140 });
    expect(TERMINAL_MIN).toEqual({ side: 320, vertical: 120 });
    expect(DIVIDER_PX).toBe(6);
  });
});

describe('panelBounds and clampSize', () => {
  it('gives the side docks a width range: 240 up to the workspace width less 320 and the divider', () => {
    expect(panelBounds('left', BIG)).toEqual({ min: 240, max: 1600 - 320 - 6 });
    expect(panelBounds('right', BIG)).toEqual({ min: 240, max: 1600 - 320 - 6 });
  });

  it('gives the vertical docks a height range: 140 up to the workspace height less 120 and the divider', () => {
    expect(panelBounds('top', BIG)).toEqual({ min: 140, max: 1000 - 120 - 6 });
    expect(panelBounds('bottom', BIG)).toEqual({ min: 140, max: 1000 - 120 - 6 });
  });

  it('uses the width for side docks and the height for top and bottom', () => {
    const wide: Container = { width: 2000, height: 300 };
    expect(panelBounds('left', wide).max).toBe(2000 - 326);
    expect(panelBounds('bottom', wide).max).toBe(300 - 126);
  });

  it('keeps a size inside the range, for each dock', () => {
    const docks: Dock[] = ['left', 'right', 'top', 'bottom'];
    for (const dock of docks) {
      const { min, max } = panelBounds(dock, BIG);
      expect(clampSize(dock, min - 50, BIG)).toBe(min);
      expect(clampSize(dock, max + 50, BIG)).toBe(max);
      expect(clampSize(dock, (min + max) / 2, BIG)).toBe((min + max) / 2);
    }
  });

  it('keeps the terminal at or above its minimum when the panel is at its largest', () => {
    const cases: Array<[Dock, Container]> = [
      ['left', { width: 900, height: 600 }],
      ['right', { width: 1280, height: 800 }],
      ['top', { width: 900, height: 600 }],
      ['bottom', { width: 1280, height: 500 }],
    ];
    for (const [dock, container] of cases) {
      const panel = clampSize(dock, Infinity, container);
      const axis = dock === 'left' || dock === 'right' ? container.width : container.height;
      const termMin = dock === 'left' || dock === 'right' ? TERMINAL_MIN.side : TERMINAL_MIN.vertical;
      expect(axis - panel - DIVIDER_PX).toBeGreaterThanOrEqual(termMin);
    }
  });

  it('gives the panel its minimum when the workspace is too small for both minimums', () => {
    const tiny: Container = { width: 500, height: 200 };
    expect(panelBounds('left', tiny)).toEqual({ min: 240, max: 240 });
    expect(clampSize('left', 400, tiny)).toBe(240);
    expect(panelBounds('bottom', tiny)).toEqual({ min: 140, max: 140 });
    expect(clampSize('bottom', 500, tiny)).toBe(140);
  });

  it('treats a zero or non-finite workspace as empty', () => {
    expect(clampSize('left', 400, { width: 0, height: 0 })).toBe(240);
    expect(clampSize('left', 400, { width: NaN, height: NaN })).toBe(240);
  });

  it('gives the minimum for NaN, and the bounds for the infinities', () => {
    expect(clampSize('right', NaN, BIG)).toBe(240);
    expect(clampSize('right', -Infinity, BIG)).toBe(240);
    expect(clampSize('right', Infinity, BIG)).toBe(1600 - 326);
  });
});

describe('resolvePanelSize', () => {
  it('gives the side default when the stored size is null', () => {
    expect(resolvePanelSize('left', null, BIG)).toBe(380);
    expect(resolvePanelSize('right', null, BIG)).toBe(380);
  });

  it('gives auto (null) for top and bottom when the stored size is null', () => {
    expect(resolvePanelSize('top', null, BIG)).toBeNull();
    expect(resolvePanelSize('bottom', null, BIG)).toBeNull();
  });

  it('clamps a stored size that does not fit the workspace', () => {
    expect(resolvePanelSize('left', 900, { width: 800, height: 600 })).toBe(800 - 326);
    expect(resolvePanelSize('bottom', 50, BIG)).toBe(140);
  });

  it('keeps a stored size that fits', () => {
    expect(resolvePanelSize('left', 300, BIG)).toBe(300);
    expect(resolvePanelSize('top', 220, BIG)).toBe(220);
  });
});

describe('nextSizeFromDrag', () => {
  it('grows a left panel as the pointer moves right', () => {
    expect(nextSizeFromDrag('left', 380, 100, 150, BIG)).toBe(430);
    expect(nextSizeFromDrag('left', 380, 100, 60, BIG)).toBe(340);
  });

  it('grows a right panel as the pointer moves left, toward the terminal', () => {
    expect(nextSizeFromDrag('right', 380, 500, 450, BIG)).toBe(430);
    expect(nextSizeFromDrag('right', 380, 500, 560, BIG)).toBe(320);
  });

  it('grows a top panel as the pointer moves down', () => {
    expect(nextSizeFromDrag('top', 200, 300, 340, BIG)).toBe(240);
    expect(nextSizeFromDrag('top', 200, 300, 250, BIG)).toBe(150);
  });

  it('grows a bottom panel as the pointer moves up, toward the terminal', () => {
    expect(nextSizeFromDrag('bottom', 200, 600, 560, BIG)).toBe(240);
    expect(nextSizeFromDrag('bottom', 200, 600, 660, BIG)).toBe(140);
  });

  it('ignores the other axis: a left panel does not respond to vertical movement', () => {
    expect(nextSizeFromDrag('left', 380, 100, 100, BIG)).toBe(380);
  });

  it('stops at the clamp in both directions', () => {
    expect(nextSizeFromDrag('left', 380, 100, 5000, BIG)).toBe(1600 - 326);
    expect(nextSizeFromDrag('left', 380, 100, -5000, BIG)).toBe(240);
  });
});

describe('keyboardSize', () => {
  it('steps the left panel by 16 with Right (grow) and Left (shrink)', () => {
    expect(keyboardSize('left', 380, 'ArrowRight', false, BIG)).toBe(396);
    expect(keyboardSize('left', 380, 'ArrowLeft', false, BIG)).toBe(364);
  });

  it('steps the right panel by 16, growing with Left', () => {
    expect(keyboardSize('right', 380, 'ArrowLeft', false, BIG)).toBe(396);
    expect(keyboardSize('right', 380, 'ArrowRight', false, BIG)).toBe(364);
  });

  it('steps the top panel by 16, growing with Down, and the bottom panel growing with Up', () => {
    expect(keyboardSize('top', 200, 'ArrowDown', false, BIG)).toBe(216);
    expect(keyboardSize('top', 200, 'ArrowUp', false, BIG)).toBe(184);
    expect(keyboardSize('bottom', 200, 'ArrowUp', false, BIG)).toBe(216);
    expect(keyboardSize('bottom', 200, 'ArrowDown', false, BIG)).toBe(184);
  });

  it('steps by 64 with Shift', () => {
    expect(keyboardSize('left', 380, 'ArrowRight', true, BIG)).toBe(444);
    expect(keyboardSize('left', 380, 'ArrowLeft', true, BIG)).toBe(316);
  });

  it('returns null for keys that do not act on this dock', () => {
    expect(keyboardSize('left', 380, 'ArrowUp', false, BIG)).toBeNull();
    expect(keyboardSize('bottom', 200, 'ArrowLeft', false, BIG)).toBeNull();
    expect(keyboardSize('left', 380, 'a', false, BIG)).toBeNull();
  });

  it('gives the minimum for Home and the maximum for End', () => {
    expect(keyboardSize('left', 380, 'Home', false, BIG)).toBe(240);
    expect(keyboardSize('left', 380, 'End', false, BIG)).toBe(1600 - 326);
    expect(keyboardSize('bottom', 200, 'Home', false, BIG)).toBe(140);
    expect(keyboardSize('bottom', 200, 'End', false, BIG)).toBe(1000 - 126);
  });

  it('stops at the clamp', () => {
    expect(keyboardSize('left', 240, 'ArrowLeft', true, BIG)).toBe(240);
    expect(keyboardSize('left', 1600 - 326, 'ArrowRight', false, BIG)).toBe(1600 - 326);
  });
});

describe('parsePanelLayout', () => {
  it('gives the default for missing, empty and non-JSON values', () => {
    expect(parsePanelLayout(null)).toEqual(defaultLayout());
    expect(parsePanelLayout(undefined)).toEqual(defaultLayout());
    expect(parsePanelLayout('')).toEqual(defaultLayout());
    expect(parsePanelLayout('{not json')).toEqual(defaultLayout());
  });

  it('gives the default for JSON that is not an object', () => {
    expect(parsePanelLayout('null')).toEqual(defaultLayout());
    expect(parsePanelLayout('42')).toEqual(defaultLayout());
    expect(parsePanelLayout('"left"')).toEqual(defaultLayout());
    expect(parsePanelLayout('[1,2]')).toEqual(defaultLayout());
  });

  it('falls back to bottom for an unknown dock and keeps the rest', () => {
    const layout = parsePanelLayout(JSON.stringify({ dock: 'center', sizes: { left: 300 }, open: false }));
    expect(layout.dock).toBe('bottom');
    expect(layout.sizes.left).toBe(300);
    expect(layout.open).toBe(false);
  });

  it('falls back to bottom when the dock is not a string', () => {
    expect(parsePanelLayout(JSON.stringify({ dock: 3 })).dock).toBe('bottom');
    expect(parsePanelLayout(JSON.stringify({ dock: ['left'] })).dock).toBe('bottom');
    expect(parsePanelLayout(JSON.stringify({ dock: 'constructor' })).dock).toBe('bottom');
  });

  it('gives null for sizes that are not finite numbers', () => {
    const layout = parsePanelLayout(JSON.stringify({ sizes: { left: 'wide', right: 'NaN', top: null, bottom: true } }));
    expect(layout.sizes).toEqual({ left: null, right: null, top: null, bottom: null });
  });

  it('gives null for NaN and Infinity written as JSON strings or missing', () => {
    expect(parsePanelLayout('{"sizes":{"left":NaN}}')).toEqual(defaultLayout());
    expect(parsePanelLayout(JSON.stringify({ sizes: { left: Infinity } })).sizes.left).toBeNull();
  });

  it('gives null for negative and zero sizes', () => {
    const layout = parsePanelLayout(JSON.stringify({ sizes: { left: -300, right: 0, top: -1, bottom: -0.5 } }));
    expect(layout.sizes).toEqual({ left: null, right: null, top: null, bottom: null });
  });

  it('gives null for huge sizes, and keeps the largest allowed one', () => {
    const huge = parsePanelLayout(JSON.stringify({ sizes: { left: 1e9, right: STORED_SIZE_MAX + 1 } }));
    expect(huge.sizes.left).toBeNull();
    expect(huge.sizes.right).toBeNull();
    const edge = parsePanelLayout(JSON.stringify({ sizes: { left: STORED_SIZE_MAX } }));
    expect(edge.sizes.left).toBe(STORED_SIZE_MAX);
  });

  it('keeps every size the Rust side holds: above 0 and at most 20000', () => {
    expect(STORED_SIZE_MAX).toBe(20000);
    const kept = parsePanelLayout(JSON.stringify({ sizes: { left: 15000, right: 20000, top: 0.5, bottom: 1 } }));
    expect(kept.sizes).toEqual({ left: 15000, right: 20000, top: 0.5, bottom: 1 });
    const dropped = parsePanelLayout(JSON.stringify({ sizes: { left: 20001, right: 0, top: 20000.5 } }));
    expect(dropped.sizes).toEqual({ left: null, right: null, top: null, bottom: null });
  });

  it('gives null for a sizes value that is not an object', () => {
    expect(parsePanelLayout(JSON.stringify({ dock: 'left', sizes: 380 }))).toEqual({
      dock: 'left',
      sizes: { left: null, right: null, top: null, bottom: null },
      open: true,
    });
  });

  it('treats a non-boolean open as true', () => {
    expect(parsePanelLayout(JSON.stringify({ open: 'no' })).open).toBe(true);
    expect(parsePanelLayout(JSON.stringify({ open: 0 })).open).toBe(true);
    expect(parsePanelLayout(JSON.stringify({ open: false })).open).toBe(false);
  });

  it('accepts a valid layout as written', () => {
    const stored: PanelLayout = {
      dock: 'right',
      sizes: { left: 300, right: 420, top: 220, bottom: null },
      open: false,
    };
    expect(parsePanelLayout(JSON.stringify(stored))).toEqual(stored);
  });
});

describe('loadPanelLayout and savePanelLayout', () => {
  it('round-trips through injected storage', () => {
    const storage = fakeStorage();
    const layout: PanelLayout = { dock: 'top', sizes: { left: 410, right: 380, top: 250, bottom: null }, open: true };
    savePanelLayout(layout, storage);
    expect(loadPanelLayout(storage)).toEqual(layout);
  });

  it('writes the JSON under the storage key, with dock, sizes and open', () => {
    const storage = fakeStorage();
    savePanelLayout(defaultLayout(), storage);
    const written = JSON.parse(storage.data.get(PANEL_KEY) ?? 'null');
    expect(written).toEqual({ dock: 'bottom', sizes: { left: 380, right: 380, top: null, bottom: null }, open: true });
    expect(PANEL_KEY).toBe('watt.panel');
  });

  it('loads the default when nothing is saved', () => {
    expect(loadPanelLayout(fakeStorage())).toEqual(defaultLayout());
  });

  it('loads the default when there is no storage', () => {
    expect(loadPanelLayout(null)).toEqual(defaultLayout());
  });

  it('loads the default when storage throws', () => {
    expect(loadPanelLayout(fakeStorage({ [PANEL_KEY]: '{"dock":"left"}' }, true))).toEqual(defaultLayout());
  });

  it('reads the layout from the old agent-term key when the new key is absent, and copies it', () => {
    const old = { dock: 'left', sizes: { left: 300, right: 380, top: null, bottom: null }, open: false };
    const storage = fakeStorage({ [PANEL_LEGACY_KEY]: JSON.stringify(old) });
    expect(loadPanelLayout(storage)).toEqual(old);
    expect(JSON.parse(storage.data.get(PANEL_KEY) ?? 'null')).toEqual(old);
    expect(storage.data.get(PANEL_LEGACY_KEY)).toBe(JSON.stringify(old));
  });

  it('prefers the new key when both keys are set', () => {
    const storage = fakeStorage({
      [PANEL_KEY]: '{"dock":"top"}',
      [PANEL_LEGACY_KEY]: '{"dock":"left"}',
    });
    expect(loadPanelLayout(storage).dock).toBe('top');
  });

  it('does nothing without storage, and does not throw when storage throws', () => {
    expect(() => savePanelLayout(defaultLayout(), null)).not.toThrow();
    expect(() => savePanelLayout(defaultLayout(), fakeStorage({}, true))).not.toThrow();
  });
});

describe('panelReducer', () => {
  const base = defaultLayout();

  it('changes the dock and keeps each dock its own size', () => {
    const left = panelReducer(base, { type: 'dock', dock: 'left' });
    expect(left.dock).toBe('left');
    const sized = panelReducer(left, { type: 'size', dock: 'left', size: 420 });
    const back = panelReducer(sized, { type: 'dock', dock: 'bottom' });
    expect(back.sizes.left).toBe(420);
    expect(back.dock).toBe('bottom');
  });

  it('returns the same object when the dock does not change', () => {
    expect(panelReducer(base, { type: 'dock', dock: 'bottom' })).toBe(base);
  });

  it('sets the size for one dock and leaves the others', () => {
    const next = panelReducer(base, { type: 'size', dock: 'top', size: 210 });
    expect(next.sizes).toEqual({ left: 380, right: 380, top: 210, bottom: null });
    expect(base.sizes.top).toBeNull();
  });

  it('ignores a size that is not finite', () => {
    expect(panelReducer(base, { type: 'size', dock: 'left', size: NaN })).toBe(base);
    expect(panelReducer(base, { type: 'size', dock: 'left', size: Infinity })).toBe(base);
  });

  it('resets one dock to its default, which is auto for top and bottom', () => {
    const sized = panelReducer(base, { type: 'size', dock: 'top', size: 210 });
    const reset = panelReducer(sized, { type: 'reset_size', dock: 'top' });
    expect(reset.sizes.top).toBeNull();
    const side = panelReducer(panelReducer(base, { type: 'size', dock: 'right', size: 500 }), {
      type: 'reset_size',
      dock: 'right',
    });
    expect(side.sizes.right).toBe(380);
  });

  it('opens and closes, and toggles', () => {
    const closed = panelReducer(base, { type: 'open', open: false });
    expect(closed.open).toBe(false);
    expect(panelReducer(closed, { type: 'open', open: false })).toBe(closed);
    expect(panelReducer(closed, { type: 'toggle' }).open).toBe(true);
    expect(panelReducer(base, { type: 'toggle' }).open).toBe(false);
  });

  it('does not change the state it was given', () => {
    const before = JSON.stringify(base);
    panelReducer(base, { type: 'size', dock: 'left', size: 300 });
    panelReducer(base, { type: 'toggle' });
    expect(JSON.stringify(base)).toBe(before);
  });
});

describe('panelView', () => {
  it('draws the bottom dock as it is today: auto height, a divider, no rail', () => {
    expect(panelView(defaultLayout(), BIG)).toEqual({
      side: false,
      rail: false,
      divider: true,
      sized: false,
      px: null,
      panelClass: 'agent-panel dock-bottom',
      style: {},
    });
  });

  it('draws a sized top dock with a fixed height', () => {
    const layout = { ...defaultLayout(), dock: 'top' as const, sizes: { ...DEFAULT_SIZES, top: 220 } };
    const view = panelView(layout, BIG);
    expect(view.panelClass).toBe('agent-panel dock-top is-sized');
    expect(view.style).toEqual({ height: 220 });
    expect(view.sized).toBe(true);
    expect(view.divider).toBe(true);
  });

  it('draws a side dock at its width, and sized', () => {
    const view = panelView({ ...defaultLayout(), dock: 'left' }, BIG);
    expect(view.side).toBe(true);
    expect(view.panelClass).toBe('agent-panel dock-left is-sized');
    expect(view.style).toEqual({ width: 380 });
    expect(view.px).toBe(380);
  });

  it('draws the right dock with its width', () => {
    const view = panelView({ ...defaultLayout(), dock: 'right', sizes: { ...DEFAULT_SIZES, right: 300 } }, BIG);
    expect(view.panelClass).toBe('agent-panel dock-right is-sized');
    expect(view.style).toEqual({ width: 300 });
  });

  it('draws a closed side dock as the rail: no size, no divider', () => {
    for (const dock of ['left', 'right'] as const) {
      const view = panelView({ ...defaultLayout(), dock, open: false }, BIG);
      expect(view.rail).toBe(true);
      expect(view.divider).toBe(false);
      expect(view.sized).toBe(false);
      expect(view.style).toEqual({});
      expect(view.panelClass).toBe(`agent-panel dock-${dock} is-rail`);
    }
  });

  it('draws a closed top or bottom panel without a fixed size or a divider, and not as the rail', () => {
    for (const dock of ['top', 'bottom'] as const) {
      const layout = { ...defaultLayout(), dock, open: false, sizes: { ...DEFAULT_SIZES, [dock]: 220 } };
      const view = panelView(layout, BIG);
      expect(view.rail).toBe(false);
      expect(view.divider).toBe(false);
      expect(view.sized).toBe(false);
      expect(view.style).toEqual({});
      expect(view.panelClass).toBe(`agent-panel dock-${dock}`);
    }
  });

  it('clamps the drawn size to the workspace', () => {
    const view = panelView({ ...defaultLayout(), dock: 'left', sizes: { ...DEFAULT_SIZES, left: 900 } }, { width: 800, height: 600 });
    expect(view.style).toEqual({ width: 800 - 326 });
  });
});

describe('panelReducer: replace', () => {
  const start = defaultLayout();
  const remote: PanelLayout = { dock: 'left', sizes: { left: 300, right: 420, top: null, bottom: 200 }, open: false };

  it('takes a valid layout whole', () => {
    expect(panelReducer(start, { type: 'replace', layout: remote })).toEqual(remote);
  });

  it('a layout with a bad dock is ignored', () => {
    const bad = { ...remote, dock: 'middle' } as unknown as PanelLayout;
    expect(panelReducer(start, { type: 'replace', layout: bad })).toBe(start);
  });

  it('a layout with a size out of range is ignored, whole', () => {
    const bad = { ...remote, sizes: { ...remote.sizes, left: 0 } };
    expect(panelReducer(start, { type: 'replace', layout: bad })).toBe(start);
  });

  it('a layout whose open is not a boolean is ignored', () => {
    const bad = { ...remote, open: 'yes' } as unknown as PanelLayout;
    expect(panelReducer(start, { type: 'replace', layout: bad })).toBe(start);
  });
});

describe('layoutFromUnknown', () => {
  const valid = { dock: 'top', sizes: { left: null, right: 380, top: 240, bottom: null }, open: true };

  it('accepts a valid layout and returns a copy', () => {
    const got = layoutFromUnknown(valid);
    expect(got).toEqual(valid);
    expect(got?.sizes).not.toBe(valid.sizes);
  });

  it('rejects anything that is not an object', () => {
    for (const value of [null, undefined, 'bottom', 3, [], true]) {
      expect(layoutFromUnknown(value)).toBeNull();
    }
  });

  it('rejects a missing or unknown dock, and a non-boolean open', () => {
    expect(layoutFromUnknown({ ...valid, dock: undefined })).toBeNull();
    expect(layoutFromUnknown({ ...valid, dock: 'Top' })).toBeNull();
    expect(layoutFromUnknown({ ...valid, open: 1 })).toBeNull();
  });

  it('rejects sizes that are missing a dock, not an object, or not finite and in range', () => {
    expect(layoutFromUnknown({ ...valid, sizes: null })).toBeNull();
    expect(layoutFromUnknown({ ...valid, sizes: { left: 1, right: 1, top: 1 } })).toBeNull();
    expect(layoutFromUnknown({ ...valid, sizes: { ...valid.sizes, right: Number.NaN } })).toBeNull();
    expect(layoutFromUnknown({ ...valid, sizes: { ...valid.sizes, right: '380' } })).toBeNull();
    expect(layoutFromUnknown({ ...valid, sizes: { ...valid.sizes, right: STORED_SIZE_MAX + 1 } })).toBeNull();
    expect(layoutFromUnknown({ ...valid, sizes: { ...valid.sizes, right: 0 } })).toBeNull();
    expect(layoutFromUnknown({ ...valid, sizes: { ...valid.sizes, right: -1 } })).toBeNull();
  });

  it('accepts sizes in the Rust range: 15000 and 20000 are in, 20001 and 0 are out, 0.5 is in', () => {
    const sized = (right: number) => layoutFromUnknown({ ...valid, sizes: { ...valid.sizes, right } });
    expect(sized(15000)?.sizes.right).toBe(15000);
    expect(sized(20000)?.sizes.right).toBe(20000);
    expect(sized(0.5)?.sizes.right).toBe(0.5);
    expect(sized(20001)).toBeNull();
    expect(sized(0)).toBeNull();
  });
});

describe('sameLayout', () => {
  it('compares dock, open, and every size', () => {
    const a = defaultLayout();
    expect(sameLayout(a, { ...a, sizes: { ...a.sizes } })).toBe(true);
    expect(sameLayout(a, { ...a, dock: 'left' })).toBe(false);
    expect(sameLayout(a, { ...a, open: false })).toBe(false);
    expect(sameLayout(a, { ...a, sizes: { ...a.sizes, top: 200 } })).toBe(false);
  });
});

describe('chatBodyShown', () => {
  it('shows the chat body only when the panel is open and the view is the chat', () => {
    expect(chatBodyShown(true, 'chat')).toBe(true);
    expect(chatBodyShown(false, 'chat')).toBe(false);
    expect(chatBodyShown(true, 'history')).toBe(false);
    expect(chatBodyShown(false, 'history')).toBe(false);
  });
});

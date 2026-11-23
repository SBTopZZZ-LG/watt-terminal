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
import type { Container, PanelLayout } from './panelLayout';
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
  it('starts on the right dock, open, with 380px panels', () => {
    expect(defaultLayout()).toEqual({
      dock: 'right',
      sizes: { left: 380, right: 380 },
      open: true,
    });
    expect(DEFAULT_SIZES).toEqual({ left: 380, right: 380 });
  });

  it('hands out a fresh object each time, so callers cannot change the shared default', () => {
    const a = defaultLayout();
    a.sizes.left = 999;
    expect(defaultLayout().sizes.left).toBe(380);
  });

  it('keeps the minimums the spec asks for', () => {
    expect(PANEL_MIN).toBe(240);
    expect(TERMINAL_MIN).toBe(320);
    expect(DIVIDER_PX).toBe(6);
  });
});

describe('panelBounds and clampSize', () => {
  it('gives a width range: 240 up to the workspace width less 320 and the divider', () => {
    expect(panelBounds(BIG)).toEqual({ min: 240, max: 1600 - 320 - 6 });
  });

  it('ignores the height', () => {
    expect(panelBounds({ width: 1600, height: 50 })).toEqual({ min: 240, max: 1600 - 326 });
  });

  it('keeps a size inside the range', () => {
    const { min, max } = panelBounds(BIG);
    expect(clampSize(min - 50, BIG)).toBe(min);
    expect(clampSize(max + 50, BIG)).toBe(max);
    expect(clampSize((min + max) / 2, BIG)).toBe((min + max) / 2);
  });

  it('keeps the terminal at or above its minimum when the panel is at its largest', () => {
    const containers: Container[] = [
      { width: 900, height: 600 },
      { width: 1280, height: 800 },
    ];
    for (const container of containers) {
      const panel = clampSize(Infinity, container);
      expect(container.width - panel - DIVIDER_PX).toBeGreaterThanOrEqual(TERMINAL_MIN);
    }
  });

  it('gives the panel its minimum when the workspace is too small for both minimums', () => {
    const tiny: Container = { width: 500, height: 200 };
    expect(panelBounds(tiny)).toEqual({ min: 240, max: 240 });
    expect(clampSize(400, tiny)).toBe(240);
  });

  it('treats a zero or non-finite workspace as empty', () => {
    expect(clampSize(400, { width: 0, height: 0 })).toBe(240);
    expect(clampSize(400, { width: NaN, height: NaN })).toBe(240);
  });

  it('gives the minimum for NaN, and the bounds for the infinities', () => {
    expect(clampSize(NaN, BIG)).toBe(240);
    expect(clampSize(-Infinity, BIG)).toBe(240);
    expect(clampSize(Infinity, BIG)).toBe(1600 - 326);
  });
});

describe('resolvePanelSize', () => {
  it('gives the 380px default when the stored size is null, for both docks', () => {
    expect(resolvePanelSize('left', null, BIG)).toBe(380);
    expect(resolvePanelSize('right', null, BIG)).toBe(380);
  });

  it('clamps a stored size that does not fit the workspace', () => {
    expect(resolvePanelSize('left', 900, { width: 800, height: 600 })).toBe(800 - 326);
    expect(resolvePanelSize('right', 50, BIG)).toBe(240);
  });

  it('keeps a stored size that fits', () => {
    expect(resolvePanelSize('left', 300, BIG)).toBe(300);
    expect(resolvePanelSize('right', 420, BIG)).toBe(420);
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

  it('steps by 64 with Shift', () => {
    expect(keyboardSize('left', 380, 'ArrowRight', true, BIG)).toBe(444);
    expect(keyboardSize('left', 380, 'ArrowLeft', true, BIG)).toBe(316);
  });

  it('returns null for keys that do not act on this dock', () => {
    expect(keyboardSize('left', 380, 'ArrowUp', false, BIG)).toBeNull();
    expect(keyboardSize('right', 380, 'ArrowDown', false, BIG)).toBeNull();
    expect(keyboardSize('left', 380, 'a', false, BIG)).toBeNull();
  });

  it('gives the minimum for Home and the maximum for End', () => {
    expect(keyboardSize('left', 380, 'Home', false, BIG)).toBe(240);
    expect(keyboardSize('right', 380, 'End', false, BIG)).toBe(1600 - 326);
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

  it('falls back to right for an unknown dock and keeps the rest', () => {
    const layout = parsePanelLayout(JSON.stringify({ dock: 'center', sizes: { left: 300 }, open: false }));
    expect(layout.dock).toBe('right');
    expect(layout.sizes.left).toBe(300);
    expect(layout.open).toBe(false);
  });

  it('falls back to right when the dock is not a string', () => {
    expect(parsePanelLayout(JSON.stringify({ dock: 3 })).dock).toBe('right');
    expect(parsePanelLayout(JSON.stringify({ dock: ['left'] })).dock).toBe('right');
    expect(parsePanelLayout(JSON.stringify({ dock: 'constructor' })).dock).toBe('right');
  });

  it('falls back to right for a stored top or bottom dock, and drops nothing else', () => {
    for (const dock of ['top', 'bottom']) {
      const layout = parsePanelLayout(JSON.stringify({ dock, sizes: { left: 300, top: 220, bottom: 200 }, open: true }));
      expect(layout).toEqual({ dock: 'right', sizes: { left: 300, right: null }, open: true });
    }
  });

  it('gives null for sizes that are not finite numbers', () => {
    const layout = parsePanelLayout(JSON.stringify({ sizes: { left: 'wide', right: 'NaN' } }));
    expect(layout.sizes).toEqual({ left: null, right: null });
  });

  it('gives null for NaN and Infinity written as JSON strings or missing', () => {
    expect(parsePanelLayout('{"sizes":{"left":NaN}}')).toEqual(defaultLayout());
    expect(parsePanelLayout(JSON.stringify({ sizes: { left: Infinity } })).sizes.left).toBeNull();
  });

  it('gives null for negative and zero sizes', () => {
    const layout = parsePanelLayout(JSON.stringify({ sizes: { left: -300, right: 0 } }));
    expect(layout.sizes).toEqual({ left: null, right: null });
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
    const kept = parsePanelLayout(JSON.stringify({ sizes: { left: 15000, right: 20000 } }));
    expect(kept.sizes).toEqual({ left: 15000, right: 20000 });
    const dropped = parsePanelLayout(JSON.stringify({ sizes: { left: 20001, right: 0 } }));
    expect(dropped.sizes).toEqual({ left: null, right: null });
  });

  it('gives null for a sizes value that is not an object', () => {
    expect(parsePanelLayout(JSON.stringify({ dock: 'left', sizes: 380 }))).toEqual({
      dock: 'left',
      sizes: { left: null, right: null },
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
      sizes: { left: 300, right: 420 },
      open: false,
    };
    expect(parsePanelLayout(JSON.stringify(stored))).toEqual(stored);
  });
});

describe('loadPanelLayout and savePanelLayout', () => {
  it('round-trips through injected storage', () => {
    const storage = fakeStorage();
    const layout: PanelLayout = { dock: 'left', sizes: { left: 410, right: 380 }, open: true };
    savePanelLayout(layout, storage);
    expect(loadPanelLayout(storage)).toEqual(layout);
  });

  it('writes the JSON under the storage key, with dock, sizes and open', () => {
    const storage = fakeStorage();
    savePanelLayout(defaultLayout(), storage);
    const written = JSON.parse(storage.data.get(PANEL_KEY) ?? 'null');
    expect(written).toEqual({ dock: 'right', sizes: { left: 380, right: 380 }, open: true });
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
    const old = { dock: 'left', sizes: { left: 300, right: 380 }, open: false };
    const storage = fakeStorage({ [PANEL_LEGACY_KEY]: JSON.stringify(old) });
    expect(loadPanelLayout(storage)).toEqual(old);
    expect(JSON.parse(storage.data.get(PANEL_KEY) ?? 'null')).toEqual(old);
    expect(storage.data.get(PANEL_LEGACY_KEY)).toBe(JSON.stringify(old));
  });

  it('prefers the new key when both keys are set', () => {
    const storage = fakeStorage({
      [PANEL_KEY]: '{"dock":"left"}',
      [PANEL_LEGACY_KEY]: '{"dock":"right"}',
    });
    expect(loadPanelLayout(storage).dock).toBe('left');
  });

  it('loads a stored bottom dock as the default dock, with no throw', () => {
    const storage = fakeStorage({ [PANEL_KEY]: '{"dock":"bottom","sizes":{"left":null,"right":null,"bottom":200},"open":true}' });
    expect(loadPanelLayout(storage)).toEqual({ dock: 'right', sizes: { left: null, right: null }, open: true });
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
    const back = panelReducer(sized, { type: 'dock', dock: 'right' });
    expect(back.sizes.left).toBe(420);
    expect(back.dock).toBe('right');
  });

  it('returns the same object when the dock does not change', () => {
    expect(panelReducer(base, { type: 'dock', dock: 'right' })).toBe(base);
  });

  it('sets the size for one dock and leaves the other', () => {
    const next = panelReducer(base, { type: 'size', dock: 'left', size: 210 });
    expect(next.sizes).toEqual({ left: 210, right: 380 });
    expect(base.sizes.left).toBe(380);
  });

  it('ignores a size that is not finite', () => {
    expect(panelReducer(base, { type: 'size', dock: 'left', size: NaN })).toBe(base);
    expect(panelReducer(base, { type: 'size', dock: 'left', size: Infinity })).toBe(base);
  });

  it('resets one dock to its default', () => {
    const sized = panelReducer(base, { type: 'size', dock: 'right', size: 500 });
    const reset = panelReducer(sized, { type: 'reset_size', dock: 'right' });
    expect(reset.sizes.right).toBe(380);
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
  it('draws the default right dock: a sized width, a divider, no rail', () => {
    expect(panelView(defaultLayout(), BIG)).toEqual({
      rail: false,
      divider: true,
      sized: true,
      px: 380,
      panelClass: 'agent-panel dock-right is-sized',
      style: { width: 380 },
    });
  });

  it('draws a side dock at its width, and sized', () => {
    const view = panelView({ ...defaultLayout(), dock: 'left' }, BIG);
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
      expect(view.px).toBeNull();
      expect(view.style).toEqual({});
      expect(view.panelClass).toBe(`agent-panel dock-${dock} is-rail`);
    }
  });

  it('clamps the drawn size to the workspace', () => {
    const view = panelView({ ...defaultLayout(), dock: 'left', sizes: { ...DEFAULT_SIZES, left: 900 } }, { width: 800, height: 600 });
    expect(view.style).toEqual({ width: 800 - 326 });
  });
});

describe('panelReducer: replace', () => {
  const start = defaultLayout();
  const remote: PanelLayout = { dock: 'left', sizes: { left: 300, right: 420 }, open: false };

  it('takes a valid layout whole', () => {
    expect(panelReducer(start, { type: 'replace', layout: remote })).toEqual(remote);
  });

  it('a layout with a bad dock is ignored', () => {
    const bad = { ...remote, dock: 'middle' } as unknown as PanelLayout;
    expect(panelReducer(start, { type: 'replace', layout: bad })).toBe(start);
  });

  it('a layout with a stored top or bottom dock is ignored', () => {
    for (const dock of ['top', 'bottom']) {
      const bad = { ...remote, dock } as unknown as PanelLayout;
      expect(panelReducer(start, { type: 'replace', layout: bad })).toBe(start);
    }
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
  const valid = { dock: 'left', sizes: { left: null, right: 380 }, open: true };

  it('accepts a valid layout and returns a copy', () => {
    const got = layoutFromUnknown(valid);
    expect(got).toEqual(valid);
    expect(got?.sizes).not.toBe(valid.sizes);
  });

  it('rejects anything that is not an object', () => {
    for (const value of [null, undefined, 'right', 3, [], true]) {
      expect(layoutFromUnknown(value)).toBeNull();
    }
  });

  it('rejects a missing or unknown dock, and a non-boolean open', () => {
    expect(layoutFromUnknown({ ...valid, dock: undefined })).toBeNull();
    expect(layoutFromUnknown({ ...valid, dock: 'Left' })).toBeNull();
    expect(layoutFromUnknown({ ...valid, open: 1 })).toBeNull();
  });

  it('rejects a top or bottom dock, which are gone', () => {
    expect(layoutFromUnknown({ ...valid, dock: 'top' })).toBeNull();
    expect(layoutFromUnknown({ ...valid, dock: 'bottom' })).toBeNull();
  });

  it('rejects sizes that are missing a dock, not an object, or not finite and in range', () => {
    expect(layoutFromUnknown({ ...valid, sizes: null })).toBeNull();
    expect(layoutFromUnknown({ ...valid, sizes: { left: 1 } })).toBeNull();
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
    expect(sameLayout(a, { ...a, sizes: { ...a.sizes, left: 200 } })).toBe(false);
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

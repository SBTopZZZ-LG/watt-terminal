// The agent panel's placement and size: the dock, the size for each dock, and whether the panel is open. Pure, so the
// layout rules are tested without the DOM. The clamp and the drag math take the workspace size (the box the terminal and
// the panel share), which the components measure.
//
// Dock geometry: left and right are side docks, the panel is a column beside the terminal and its size is a width.
// Top and bottom are the vertical docks, the panel is a row above or below the terminal and its size is a height.

import { readWithLegacyKey } from './zoom';
import type { SizeStorage } from './zoom';

export type Dock = 'left' | 'right' | 'top' | 'bottom';

export const DOCKS: readonly Dock[] = ['left', 'right', 'top', 'bottom'];

/** The workspace size in CSS px: the box the terminal and the agent panel share. */
export interface Container {
  width: number;
  height: number;
}

/** Px on the panel's axis, per dock. Null is auto: the panel grows with its content (top and bottom only). */
export type Sizes = Record<Dock, number | null>;

export interface PanelLayout {
  dock: Dock;
  sizes: Sizes;
  /**
   * Side docks: false collapses the panel to the rail. Top and bottom: false hides the transcript body, as before.
   */
  open: boolean;
}

/** The localStorage key. */
export const PANEL_KEY = 'watt.panel';
/** The key before the rename to Watt. Read only while PANEL_KEY is absent (see loadPanelLayout). */
export const PANEL_LEGACY_KEY = 'agent-term.panel';

/** The panel's minimum size on its axis. */
export const PANEL_MIN = { side: 240, vertical: 140 } as const;

/** The terminal's minimum size on the axis the panel shares with it. */
export const TERMINAL_MIN = { side: 320, vertical: 120 } as const;

/** The divider's thickness, in px. The CSS (.panel-divider) uses the same value. */
export const DIVIDER_PX = 6;

/** Arrow keys on the divider move it by this much, or by KEY_STEP_LARGE with Shift. */
export const KEY_STEP = 16;
export const KEY_STEP_LARGE = 64;

/**
 * A stored size must be above STORED_SIZE_MIN and at most STORED_SIZE_MAX, or it is dropped on load (null). The range mirrors
 * the Rust side, MAX_PANEL_SIZE in src-tauri/src/core/persist/ui_state.rs (`n > 0.0 && n <= MAX_PANEL_SIZE`), so a size the
 * UI state file holds is never dropped here. The minimum is exclusive.
 */
export const STORED_SIZE_MIN = 0;
export const STORED_SIZE_MAX = 20000;

export const DEFAULT_SIZES: Sizes = { left: 380, right: 380, top: null, bottom: null };

export function defaultLayout(): PanelLayout {
  return { dock: 'bottom', sizes: { ...DEFAULT_SIZES }, open: true };
}

export function isSideDock(dock: Dock): boolean {
  return dock === 'left' || dock === 'right';
}

/**
 * Whether the chat body is on screen: the panel is open and the view is the chat, not the history. The transcript's body and
 * banners follow it, and so does the todo panel, which sits under the transcript. Closed, or in the history view, all are hidden.
 */
export function chatBodyShown(open: boolean, view: 'chat' | 'history'): boolean {
  return open && view === 'chat';
}

function isDock(value: unknown): value is Dock {
  return typeof value === 'string' && (DOCKS as readonly string[]).includes(value);
}

/** The keys that move the divider toward the terminal (grow the panel) and away from it (shrink it), per dock. */
const GROW_KEY: Record<Dock, string> = { left: 'ArrowRight', right: 'ArrowLeft', top: 'ArrowDown', bottom: 'ArrowUp' };
const SHRINK_KEY: Record<Dock, string> = { left: 'ArrowLeft', right: 'ArrowRight', top: 'ArrowUp', bottom: 'ArrowDown' };

/**
 * The smallest and largest panel size on its axis for this workspace. The largest leaves the terminal its minimum plus the
 * divider. When the workspace is too small for both minimums, the largest is the smallest: the panel keeps its minimum and
 * the terminal gets what is left, below its own minimum.
 */
export function panelBounds(dock: Dock, container: Container): { min: number; max: number } {
  const side = isSideDock(dock);
  const min = side ? PANEL_MIN.side : PANEL_MIN.vertical;
  const axis = side ? container.width : container.height;
  const room = (Number.isFinite(axis) ? axis : 0) - (side ? TERMINAL_MIN.side : TERMINAL_MIN.vertical) - DIVIDER_PX;
  return { min, max: Math.max(min, room) };
}

/** Keeps a panel size inside panelBounds. NaN gives the minimum; infinities give the bounds. */
export function clampSize(dock: Dock, size: number, container: Container): number {
  const { min, max } = panelBounds(dock, container);
  const wanted = Number.isNaN(size) ? min : size;
  return Math.min(max, Math.max(min, wanted));
}

/**
 * The size to draw. A stored null takes the dock's default: 380 for the side docks, auto (null) for top and bottom.
 * Any size that comes back is clamped to the workspace.
 */
export function resolvePanelSize(dock: Dock, stored: number | null, container: Container): number | null {
  const size = stored ?? DEFAULT_SIZES[dock];
  return size === null ? null : clampSize(dock, size, container);
}

/**
 * The size after a pointer drag. The pointer moved from `startPointer` to `pointer` along the panel's axis (x for side docks,
 * y for top and bottom). Right and bottom docks have the panel past the divider, so it grows as the pointer moves toward the
 * terminal (left, up). Left and top docks grow as it moves right, down.
 */
export function nextSizeFromDrag(
  dock: Dock,
  startSize: number,
  startPointer: number,
  pointer: number,
  container: Container,
): number {
  const toward = dock === 'right' || dock === 'bottom' ? -1 : 1;
  return clampSize(dock, startSize + toward * (pointer - startPointer), container);
}

/**
 * The size after a key on the divider, or null when the key does not act on this dock. The grow and shrink arrows are the
 * ones that move the divider toward and away from the terminal. Shift takes a larger step. Home gives the minimum and End
 * the maximum.
 */
export function keyboardSize(
  dock: Dock,
  current: number,
  key: string,
  shift: boolean,
  container: Container,
): number | null {
  if (key === 'Home') {
    return clampSize(dock, -Infinity, container);
  }
  if (key === 'End') {
    return clampSize(dock, Infinity, container);
  }
  const step = shift ? KEY_STEP_LARGE : KEY_STEP;
  if (key === GROW_KEY[dock]) {
    return clampSize(dock, current + step, container);
  }
  if (key === SHRINK_KEY[dock]) {
    return clampSize(dock, current - step, container);
  }
  return null;
}

function isStoredSize(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value) && value > STORED_SIZE_MIN && value <= STORED_SIZE_MAX;
}

function storedSize(value: unknown): number | null {
  return isStoredSize(value) ? value : null;
}

/**
 * A layout from untrusted data (the UI state file, a remote value). Every field must be valid, or the result is null:
 * a dock that is not one of the four, a non-boolean open, a sizes object without all four docks, or a size that is not
 * null and not a finite number in range. Unlike parsePanelLayout, nothing is defaulted field by field here.
 */
export function layoutFromUnknown(value: unknown): PanelLayout | null {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    return null;
  }
  const record = value as Record<string, unknown>;
  if (!isDock(record.dock) || typeof record.open !== 'boolean') {
    return null;
  }
  const rawSizes = record.sizes;
  if (typeof rawSizes !== 'object' || rawSizes === null || Array.isArray(rawSizes)) {
    return null;
  }
  const sizesRecord = rawSizes as Record<string, unknown>;
  const sizes = { ...DEFAULT_SIZES };
  for (const dock of DOCKS) {
    const size = sizesRecord[dock];
    if (size !== null && !isStoredSize(size)) {
      return null;
    }
    sizes[dock] = size;
  }
  return { dock: record.dock, sizes, open: record.open };
}

/** Whether two layouts draw the same. */
export function sameLayout(a: PanelLayout, b: PanelLayout): boolean {
  return a.dock === b.dock && a.open === b.open && DOCKS.every((dock) => a.sizes[dock] === b.sizes[dock]);
}

/**
 * Reads the saved layout. Anything unusable falls back field by field: no JSON or a non-object gives the default layout,
 * an unknown dock gives 'bottom', a size that is not a finite number in range gives null, a non-boolean open gives true.
 */
export function parsePanelLayout(raw: string | null | undefined): PanelLayout {
  const fallback = defaultLayout();
  if (typeof raw !== 'string') {
    return fallback;
  }
  let data: unknown;
  try {
    data = JSON.parse(raw);
  } catch {
    return fallback;
  }
  if (typeof data !== 'object' || data === null || Array.isArray(data)) {
    return fallback;
  }
  const record = data as Record<string, unknown>;
  const rawSizes: Record<string, unknown> =
    typeof record.sizes === 'object' && record.sizes !== null ? (record.sizes as Record<string, unknown>) : {};
  return {
    dock: isDock(record.dock) ? record.dock : fallback.dock,
    sizes: {
      left: storedSize(rawSizes.left),
      right: storedSize(rawSizes.right),
      top: storedSize(rawSizes.top),
      bottom: storedSize(rawSizes.bottom),
    },
    open: typeof record.open === 'boolean' ? record.open : fallback.open,
  };
}

/** The saved layout, or the default. A missing storage or one that throws gives the default. */
export function loadPanelLayout(storage: SizeStorage | null): PanelLayout {
  if (storage === null) {
    return defaultLayout();
  }
  try {
    return parsePanelLayout(readWithLegacyKey(storage, PANEL_KEY, PANEL_LEGACY_KEY));
  } catch {
    return defaultLayout();
  }
}

/** Saves the layout. A missing storage is skipped, and a storage that throws is ignored: the layout still applies. */
export function savePanelLayout(layout: PanelLayout, storage: SizeStorage | null): void {
  if (storage === null) {
    return;
  }
  try {
    storage.setItem(PANEL_KEY, JSON.stringify(layout));
  } catch {
    // Not persisted. The layout still applies until the next launch.
  }
}

export type PanelAction =
  | { type: 'dock'; dock: Dock }
  | { type: 'size'; dock: Dock; size: number }
  | { type: 'reset_size'; dock: Dock }
  | { type: 'open'; open: boolean }
  | { type: 'toggle' }
  /** Takes a whole layout, e.g. the one saved in the UI state file. An invalid layout is ignored. */
  | { type: 'replace'; layout: PanelLayout };

/** The layout after one action. The size is taken as given: the caller clamps it to the workspace. */
export function panelReducer(state: PanelLayout, action: PanelAction): PanelLayout {
  switch (action.type) {
    case 'replace':
      return layoutFromUnknown(action.layout) ?? state;
    case 'dock':
      return action.dock === state.dock ? state : { ...state, dock: action.dock };
    case 'size':
      if (!Number.isFinite(action.size)) {
        return state;
      }
      return { ...state, sizes: { ...state.sizes, [action.dock]: action.size } };
    case 'reset_size':
      return { ...state, sizes: { ...state.sizes, [action.dock]: DEFAULT_SIZES[action.dock] } };
    case 'open':
      return action.open === state.open ? state : { ...state, open: action.open };
    case 'toggle':
      return { ...state, open: !state.open };
  }
}

/** How the layout is drawn. The components read this and do not decide anything themselves. */
export interface PanelView {
  side: boolean;
  /** Side docks, closed: the panel is the 32px rail. The transcript and bar stay mounted and hidden. */
  rail: boolean;
  /**
   * The divider between the panel and the terminal. Hidden on the rail, and on a closed top or bottom panel (there is no
   * body to size).
   */
  divider: boolean;
  /** The panel has a fixed px size on its axis. Side docks always do (except the rail); top and bottom only when open and set. */
  sized: boolean;
  /** The px size on the panel's axis, or null when the panel is auto. */
  px: number | null;
  panelClass: string;
  /** Inline size: width for side docks, height for top and bottom. Empty when auto or on the rail. */
  style: { width?: number; height?: number };
}

export function panelView(layout: PanelLayout, container: Container): PanelView {
  const { dock, open } = layout;
  const side = isSideDock(dock);
  const rail = side && !open;
  const px = rail || (!side && !open) ? null : resolvePanelSize(dock, layout.sizes[dock], container);
  const sized = px !== null;
  const classes = ['agent-panel', `dock-${dock}`];
  if (sized) {
    classes.push('is-sized');
  }
  if (rail) {
    classes.push('is-rail');
  }
  return {
    side,
    rail,
    divider: !rail && (side || open),
    sized,
    px,
    panelClass: classes.join(' '),
    style: px === null ? {} : side ? { width: px } : { height: px },
  };
}

// The tooltip's rules, without the DOM: where a tip goes, whether an element's text is clipped, which element's tip
// applies, how long a tip may be, and when it shows. The component that uses them is src/components/Tooltip.tsx.

/** Gap between the anchor and the tip, in px. */
export const TIP_GAP = 8;
/** Clear space kept between the tip and the viewport edges, in px. */
export const TIP_MARGIN = 8;
/** How long the pointer (or keyboard focus) rests on an anchor before its tip shows, in ms. */
export const TIP_DELAY_MS = 350;
/** A tip shown again within this many ms of the last one hiding appears at once, so moving between tips feels direct. */
export const TIP_REOPEN_MS = 300;
/** A tip keeps at most this many characters; the cut is marked with an ellipsis. */
export const TIP_MAX_CHARS = 2000;
/** Overflow of up to this many px is rounding, not clipping. */
export const CLIP_TOLERANCE_PX = 1;

export interface Rect {
  left: number;
  top: number;
  width: number;
  height: number;
}

export interface Size {
  width: number;
  height: number;
}

export type Side = 'above' | 'below';

export interface Placement {
  /** Left edge of the tip, in viewport px (the tip is position: fixed). */
  x: number;
  /** Top edge of the tip, in viewport px. */
  y: number;
  side: Side;
}

function clamp(value: number, low: number, high: number): number {
  return Math.max(low, Math.min(value, high));
}

/**
 * Where the tip goes: centred under the anchor, or above it when there is no room below and there is room above. Kept
 * inside the viewport: horizontally by clamping, vertically by clamping too (a tip taller than the room is cut at the edge).
 */
export function placement(anchor: Rect, tip: Size, viewport: Size): Placement {
  const below = anchor.top + anchor.height + TIP_GAP;
  const above = anchor.top - TIP_GAP - tip.height;
  const fitsBelow = below + tip.height <= viewport.height - TIP_MARGIN;
  const fitsAbove = above >= TIP_MARGIN;
  const side: Side = !fitsBelow && fitsAbove ? 'above' : 'below';
  const x = anchor.left + anchor.width / 2 - tip.width / 2;
  return {
    x: clamp(x, TIP_MARGIN, viewport.width - TIP_MARGIN - tip.width),
    y: clamp(side === 'above' ? above : below, TIP_MARGIN, viewport.height - TIP_MARGIN - tip.height),
    side,
  };
}

/** The metrics of an element that tell whether its content is cut off. */
export interface ClipMetrics {
  scrollWidth: number;
  clientWidth: number;
  scrollHeight: number;
  clientHeight: number;
}

/** True when the content is wider or taller than the box (beyond rounding), so something is cut off. */
export function isClipped(m: ClipMetrics): boolean {
  return m.scrollWidth - m.clientWidth > CLIP_TOLERANCE_PX || m.scrollHeight - m.clientHeight > CLIP_TOLERANCE_PX;
}

/** The part of an element the resolver reads. A DOM element has all of it. */
export interface TipNode {
  tagName?: string;
  getAttribute(name: string): string | null;
  parentElement: TipNode | null;
}

export interface ResolvedTip<T> {
  /** The element that carries data-tip. It gets aria-describedby while its tip shows. */
  anchor: T;
  text: string;
  /** The tip was opted in with data-tip-clip: it only shows while the element is really clipped. */
  requireClip: boolean;
}

function hasClass(node: TipNode, name: string): boolean {
  return (node.getAttribute('class') ?? '').split(/\s+/).includes(name);
}

/**
 * The tip for a hovered or focused element: the nearest ancestor (or the element itself) with data-tip. A data-tip-clip
 * tip is skipped unless `clipped` says its element is cut off, so the next tip up applies instead. Nothing shows for
 * typing targets (input, textarea) or anything inside the terminal (.xterm), and nothing for an empty tip.
 */
export function resolveTip<T extends TipNode>(el: T, clipped: (node: T) => boolean): ResolvedTip<T> | null {
  if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {
    return null;
  }
  for (let node: TipNode | null = el; node !== null; node = node.parentElement) {
    if (hasClass(node, 'xterm')) {
      return null;
    }
  }
  for (let node: TipNode | null = el; node !== null; node = node.parentElement) {
    const raw = node.getAttribute('data-tip');
    if (raw === null) {
      continue;
    }
    if (raw.trim() === '') {
      return null;
    }
    const requireClip = node.getAttribute('data-tip-clip') !== null;
    if (requireClip && !clipped(node as T)) {
      continue;
    }
    return { anchor: node as T, text: capText(raw), requireClip };
  }
  return null;
}

/** Caps a tip's text at TIP_MAX_CHARS characters (code points), marking the cut with an ellipsis. */
export function capText(text: string, max: number = TIP_MAX_CHARS): string {
  const chars = [...text];
  if (chars.length <= max) {
    return text;
  }
  return `${chars.slice(0, max - 1).join('')}…`;
}

/** The delay before a tip shows: none when the last tip hid less than TIP_REOPEN_MS ago, else TIP_DELAY_MS. */
export function showDelay(now: number, lastHideAt: number | null): number {
  return lastHideAt !== null && now - lastHideAt <= TIP_REOPEN_MS ? 0 : TIP_DELAY_MS;
}

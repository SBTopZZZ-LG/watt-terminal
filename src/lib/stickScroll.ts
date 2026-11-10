// Pure geometry for the transcript's stick-to-bottom scrolling: whether the view is at the bottom, and whether a send
// should resume following. No DOM, so it is tested directly.

import type { TranscriptItem } from './transcript';

/** The scroll geometry of an element, as the DOM reports it. */
export interface ScrollMetrics {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
}

/** How far the view is above the bottom of the content, in px. Content shorter than the view counts as 0. */
export function distanceFromBottom(m: ScrollMetrics): number {
  return Math.max(0, m.scrollHeight - m.clientHeight - m.scrollTop);
}

/** True when the view is within `threshold` px of the bottom. The threshold absorbs sub-pixel and fractional scrollTop. */
export function isAtBottom(m: ScrollMetrics, threshold = 24): boolean {
  return distanceFromBottom(m) <= threshold;
}

/** True when the last item is a user message that was not the last item before: the user just sent something. */
export function shouldFollowAfterSend(prevLast: TranscriptItem | undefined, nextLast: TranscriptItem | undefined): boolean {
  return nextLast !== undefined && nextLast.kind === 'user' && nextLast.id !== prevLast?.id;
}

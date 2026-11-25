// Paging of a saved conversation: the transcript shows its newest messages, and older chunks load as the view scrolls up.
// Pure, so the cuts and the scroll decisions are tested without the DOM. The cuts are in message space (the raw messages),
// at turn starts, so a tool call is never split from its result.

import type { ChatMessage } from './types';
import type { ScrollMetrics } from './stickScroll';

/** How many messages a chunk aims for. A cut moves back to a turn start, so a chunk can be longer. Must be at least 1. */
export const CHUNK_MESSAGES = 60;

/** An upward scroll that reaches this many px from the top asks for the next older chunk. */
export const OLDER_THRESHOLD_PX = 500;

/** A turn starts at a user message. A steered message (sent into a running turn) is in the middle of one, so it is not. */
export function isTurnStart(message: ChatMessage): boolean {
  return message.role === 'user' && message.steer !== true;
}

/** `at`, or the nearest turn start before it. Index 0 always counts, so this always finds one. */
function turnStartAtOrBefore(messages: ChatMessage[], at: number): number {
  for (let i = at; i > 0; i--) {
    if (isTurnStart(messages[i])) {
      return i;
    }
  }
  return 0;
}

/** The index the newest chunk starts at: about `size` messages from the end, moved back to a turn start. 0 means all of them. */
export function tailStart(messages: ChatMessage[], size = CHUNK_MESSAGES): number {
  return turnStartAtOrBefore(messages, Math.max(0, messages.length - size));
}

/** The index the chunk that ends just before `end` starts at: about `size` messages back, moved back to a turn start. 0 means the rest. */
export function olderStart(messages: ChatMessage[], end: number, size = CHUNK_MESSAGES): number {
  return end <= 0 ? 0 : turnStartAtOrBefore(messages, Math.max(0, end - size));
}

/** Whether an upward scroll asks for the next older chunk: the view came up to the threshold, more is there, and none is on its way. */
export function wantsOlder(s: { movedUp: boolean; top: number; hasOlder: boolean; pending: boolean }): boolean {
  return s.movedUp && s.top <= OLDER_THRESHOLD_PX && s.hasOlder && !s.pending;
}

/** Whether the content does not fill the view, so there is nothing to scroll. A view with no height (not laid out yet) is not measured. */
export function needsFill(m: ScrollMetrics): boolean {
  return m.clientHeight > 0 && m.scrollHeight <= m.clientHeight;
}

/** The scrollTop that keeps the same content in view once content was added above it: the old top plus the growth. */
export function keptTop(before: { top: number; height: number }, height: number): number {
  return before.top + (height - before.height);
}

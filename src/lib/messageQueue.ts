// Messages typed while the agent is busy. They wait in a queue and are sent after the turn ends: all at once as one
// message, or one per turn (the queue_mode setting). Pure, so the rules are tested without React or the backend. The app
// owns the queue state and does the sending (see App.tsx); this module only decides what goes where.

import type { AttachmentInput, QueueMode } from './types';

/** A message waiting for the agent to finish its turn. `id` is unique within the app run. */
export interface QueuedMessage {
  id: number;
  text: string;
  attachments: AttachmentInput[];
}

/** How often the drain asks the backend whether its turn slot is free (the `done` event comes before the slot is released). */
export const QUEUE_SLOT_POLL_MS = 100;
/** How long the drain waits for the slot before it sends anyway (a refusal then puts the messages back in the queue). */
export const QUEUE_SLOT_WAIT_MS = 3000;

/** Adds a message at the end of the queue. */
export function enqueue(queue: QueuedMessage[], id: number, text: string, attachments: AttachmentInput[]): QueuedMessage[] {
  return [...queue, { id, text, attachments }];
}

/** Takes a message out of the queue. `removed` is null when no message has that id. */
export function removeQueued(queue: QueuedMessage[], id: number): { queue: QueuedMessage[]; removed: QueuedMessage | null } {
  const removed = queue.find((q) => q.id === id) ?? null;
  return removed === null ? { queue, removed } : { queue: queue.filter((q) => q.id !== id), removed };
}

/**
 * What the next send takes from the queue. `all`: one message holding every queued text (joined by a blank line; empty
 * texts are left out) and every attachment, in order. `one`: the first message alone. `rest` is what stays queued.
 * `send` is null when the queue is empty.
 */
export function drainBatch(
  queue: QueuedMessage[],
  mode: QueueMode,
): { send: { text: string; attachments: AttachmentInput[] } | null; rest: QueuedMessage[] } {
  if (queue.length === 0) {
    return { send: null, rest: [] };
  }
  if (mode === 'one') {
    const [first, ...rest] = queue;
    return { send: { text: first.text, attachments: first.attachments }, rest };
  }
  const text = queue
    .map((q) => q.text)
    .filter((t) => t !== '')
    .join('\n\n');
  return { send: { text, attachments: queue.flatMap((q) => q.attachments) }, rest: [] };
}

/**
 * The text the input gets back when a queued message is removed. The removed text goes in front of whatever the input
 * holds (on its own line), or is the whole text when the input is empty.
 */
export function restoreText(removedText: string, currentDraft: string): string {
  return currentDraft.trim() === '' ? removedText : `${removedText}\n${currentDraft}`;
}

/**
 * Whether a turn has just ended with messages waiting: the agent (or a side command) was active and no longer is, and
 * the queue is not empty. `active` is busy or a side command running; the drain waits for both to end.
 */
export function shouldDrain(prevActive: boolean, active: boolean, queueLength: number): boolean {
  return prevActive && !active && queueLength > 0;
}

/** The label above the queue rows, by mode. */
export function queueHeader(mode: QueueMode): string {
  return mode === 'one' ? 'Queued · sent one at a time when the agent finishes' : 'Queued · sent together when the agent finishes';
}

/** How many attachments a queued message carries, as shown on its row. */
export function queuedAttachmentNote(count: number): string {
  return count === 1 ? '1 attachment' : `${count} attachments`;
}

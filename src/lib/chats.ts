// The agent panel's chats as pure rules: what a chat keeps while another one is shown, how an event for a chat that is not
// shown reduces into its record, the badges the strip shows, the names, and the chat a close moves to. The App holds the
// shown chat's state and swaps it with a record when the user switches (see App.tsx switchChat).

import { agentRequestAfter } from './integrate';
import type { AgentRequest } from './integrate';
import { removeInjected, unsteer } from './messageQueue';
import type { QueuedMessage } from './messageQueue';
import { isSessionEvent } from './tabState';
import { neighbourAfterClose } from './tabStrip';
import { EMPTY_TRANSCRIPT, firstPendingApproval, isBusy, transcriptReducer } from './transcript';
import type { Transcript } from './transcript';
import type { WireEvent } from './types';

/**
 * What one chat keeps while another one is shown: its transcript, its conversation, its queued messages, a question or an
 * integration request still waiting, and the approve-all flags (the backend holds them per chat; kept so they show at once).
 * Not kept, on purpose: the suggestions (dropped on a switch; saved ones come back through the conversation), the draft (keyed
 * per chat, see App), and the queue's ids (they are unique across chats already).
 */
export interface ChatRecord {
  transcript: Transcript;
  conversationId: string | null;
  queue: QueuedMessage[];
  agentReq: AgentRequest | null;
  autoApprove: boolean;
  autoSuspended: boolean;
}

/** A chat with no conversation yet: an empty transcript, nothing queued, nothing waiting. */
export function emptyRecord(): ChatRecord {
  return {
    transcript: EMPTY_TRANSCRIPT,
    conversationId: null,
    queue: [],
    agentReq: null,
    autoApprove: false,
    autoSuspended: false,
  };
}

/**
 * The record of a chat that is not shown, after one of its events. The same reduction the shown chat gets (App's handleEvent):
 * agent events go through transcriptReducer; a steer moves the queue; the request for the shell and the end of a turn update
 * the request. `terminal` is the title of the terminal an approval names (the caller reads the registry). Session events are
 * the terminal's own, not the chat's, so they leave the record as it is.
 */
export function reduceBackgroundEvent(record: ChatRecord, ev: WireEvent, at: number, terminal: string | null = null): ChatRecord {
  if (isSessionEvent(ev)) {
    return record;
  }
  switch (ev.type) {
    case 'integration_request':
      return { ...record, agentReq: agentRequestAfter(record.agentReq, ev) };
    case 'done':
      return {
        ...record,
        agentReq: agentRequestAfter(record.agentReq, ev),
        transcript: transcriptReducer(record.transcript, { type: 'agent_event', event: ev, at }),
      };
    case 'steer_injected':
      // The turn took these: their rows leave the queue, and the transcript shows them at that step (as the shown chat does).
      return {
        ...record,
        queue: removeInjected(record.queue, ev.ids),
        transcript: transcriptReducer(record.transcript, { type: 'agent_event', event: ev, at }),
      };
    case 'steer_returned':
      return { ...record, queue: unsteer(record.queue, ev.ids) };
    case 'approval_request':
      return {
        ...record,
        transcript: transcriptReducer(record.transcript, { type: 'agent_event', event: ev, at, terminal: terminal ?? undefined }),
      };
    default:
      return { ...record, transcript: transcriptReducer(record.transcript, { type: 'agent_event', event: ev, at }) };
  }
}

/** What the strip shows for a chat: busy (a turn runs), approval (a tool waits for the user), question (the agent asks). */
export interface ChatBadges {
  busy: boolean;
  approval: boolean;
  question: boolean;
}

/** The badges of a chat, from its transcript. The same reads as the App's busy, approval and question flags. */
export function chatBadges(record: Pick<ChatRecord, 'transcript'>): ChatBadges {
  return {
    busy: isBusy(record.transcript),
    approval: firstPendingApproval(record.transcript) !== null,
    question: record.transcript.items.some((i) => i.kind === 'question' && i.state === 'pending'),
  };
}

/**
 * "Chat N": N is the chat's place in `order`, the ids of every chat made in this page, in creation order (`main` first). A
 * number does not move when another chat closes, so a chat keeps its name for as long as the page lives.
 */
export function chatLabel(order: readonly string[], id: string): string {
  const at = order.indexOf(id);
  return at === -1 ? 'Chat' : `Chat ${at + 1}`;
}

/** The chat shown after `closedId` closes: its right neighbour in `ids` (the strip's order), else its left one. Null when none is left. */
export function nextActiveAfterClose(ids: readonly string[], closedId: string): string | null {
  return neighbourAfterClose(ids, closedId);
}

/** What stops a switch of the shown chat now (its text is kept by the caller), or null when it may switch. */
export interface ChatSwitchState {
  /** A side command runs in the shown chat (`/btw`, `/compact`, ...): its answer would land in the chat switched to. */
  sideBusy: boolean;
  /** An open or a new conversation is in flight (the App's `switching` ref). */
  switching: boolean;
  /** A send, a queued message, or a command is in flight in the shown chat: its result would land in the chat switched to. */
  pending: boolean;
}

export const CHAT_SWITCH_SIDE_BUSY = 'A side command is still running. Wait for it to finish, or stop it first.';
export const CHAT_SWITCH_SWITCHING = 'A conversation is already being opened or cleared. Try again in a moment.';
export const CHAT_SWITCH_PENDING = 'A message is still being sent. Try the chat again in a moment.';

export function chatSwitchRefusal(s: ChatSwitchState): string | null {
  if (s.sideBusy) {
    return CHAT_SWITCH_SIDE_BUSY;
  }
  if (s.switching) {
    return CHAT_SWITCH_SWITCHING;
  }
  if (s.pending) {
    return CHAT_SWITCH_PENDING;
  }
  return null;
}

/** Why a chat cannot close now (the last one never closes; a busy one is refused by the backend too), or null when it can. */
export function chatCloseRefusal(label: string, s: { count: number; busy: boolean }): string | null {
  if (s.count <= 1) {
    return 'The last chat cannot be closed.';
  }
  if (s.busy) {
    return `${label} is busy. Stop its turn first, then close it.`;
  }
  return null;
}

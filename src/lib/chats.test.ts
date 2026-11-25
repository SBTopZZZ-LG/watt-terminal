import { describe, expect, it } from 'vitest';
import {
  CHAT_SWITCH_PENDING,
  CHAT_SWITCH_SIDE_BUSY,
  CHAT_SWITCH_SWITCHING,
  chatBadges,
  chatCloseRefusal,
  chatLabel,
  chatSwitchRefusal,
  emptyRecord,
  nextActiveAfterClose,
  reduceBackgroundEvent,
} from './chats';
import type { ChatRecord } from './chats';
import { transcriptReducer } from './transcript';
import type { WireEvent } from './types';
import type { QueuedMessage } from './messageQueue';

/** A record whose turn is running: one message sent, not yet done. */
function busyRecord(): ChatRecord {
  const transcript = transcriptReducer(emptyRecord().transcript, { type: 'user_sent', clientId: 1, text: 'go' });
  return { ...emptyRecord(), transcript };
}

function queued(id: number, text: string): QueuedMessage {
  return { id, text, attachments: [] };
}

describe('emptyRecord', () => {
  it('is a chat with no conversation, nothing queued, nothing waiting', () => {
    const r = emptyRecord();
    expect(r.conversationId).toBeNull();
    expect(r.queue).toEqual([]);
    expect(r.agentReq).toBeNull();
    expect(r.transcript.items).toEqual([]);
    expect(chatBadges(r)).toEqual({ busy: false, approval: false, question: false });
  });
});

describe('reduceBackgroundEvent: transcript', () => {
  it('puts an agent event into the chat\'s own transcript', () => {
    const r = reduceBackgroundEvent(emptyRecord(), { type: 'text', text: 'hello' } as WireEvent, 1);
    expect(r.transcript.items.map((i) => i.kind)).toEqual(['agent']);
  });

  it('ends the turn on done, and the badge goes idle', () => {
    const running = busyRecord();
    expect(chatBadges(running).busy).toBe(true);
    const finished = reduceBackgroundEvent(running, { type: 'done' }, 2);
    expect(chatBadges(finished).busy).toBe(false);
  });

  it('shows a pending approval as the approval badge', () => {
    const ev = { type: 'approval_request', call_id: 'a1', name: 'run_command', text: 'ls' } as WireEvent;
    expect(chatBadges(reduceBackgroundEvent(busyRecord(), ev, 2)).approval).toBe(true);
  });

  it('shows a waiting question as the question badge', () => {
    const ev = { type: 'question_request', call_id: 'q1', questions: [] } as WireEvent;
    expect(chatBadges(reduceBackgroundEvent(busyRecord(), ev, 2)).question).toBe(true);
  });

  it('leaves a session event (the terminal\'s own) alone', () => {
    const r = emptyRecord();
    expect(reduceBackgroundEvent(r, { type: 'levels', levels: [] } as unknown as WireEvent, 1)).toBe(r);
  });
});

describe('reduceBackgroundEvent: queue and request', () => {
  it('takes the steered rows out of the queue when the turn injects them', () => {
    const r = { ...busyRecord(), queue: [queued(1, 'a'), queued(2, 'b')] };
    const next = reduceBackgroundEvent(r, { type: 'steer_injected', ids: [1], text: 'a', attachments: [] }, 2);
    expect(next.queue.map((q) => q.id)).toEqual([2]);
  });

  it('makes a steer the turn did not take a plain queued message again', () => {
    const steered: QueuedMessage = { ...queued(1, 'a'), steered: true };
    const next = reduceBackgroundEvent({ ...emptyRecord(), queue: [steered] }, { type: 'steer_returned', ids: [1] }, 2);
    expect(next.queue).toHaveLength(1);
    expect(next.queue[0].steered).toBeFalsy();
  });

  it('sets the shell request from integration_request asking, and closes it on done', () => {
    const asked = reduceBackgroundEvent(emptyRecord(), { type: 'integration_request', call_id: 'c1', phase: 'asking', label: 'bash' }, 1);
    expect(asked.agentReq).toEqual({ kind: 'asking', callId: 'c1', label: 'bash' });
    const ended = reduceBackgroundEvent(asked, { type: 'done' }, 2);
    expect(ended.agentReq).toBeNull();
  });
});

describe('chatLabel', () => {
  const order = ['main', 'c2', 'c3'];

  it('numbers chats by creation order, main first', () => {
    expect(chatLabel(order, 'main')).toBe('Chat 1');
    expect(chatLabel(order, 'c3')).toBe('Chat 3');
  });

  it('falls back to a plain name for an unknown id', () => {
    expect(chatLabel(order, 'nope')).toBe('Chat');
  });
});

describe('nextActiveAfterClose', () => {
  it('moves to the right neighbour, else the left one', () => {
    expect(nextActiveAfterClose(['main', 'c2', 'c3'], 'c2')).toBe('c3');
    expect(nextActiveAfterClose(['main', 'c2', 'c3'], 'c3')).toBe('c2');
  });

  it('is null when no other chat is left, or the id is unknown', () => {
    expect(nextActiveAfterClose(['main'], 'main')).toBeNull();
    expect(nextActiveAfterClose(['main', 'c2'], 'zz')).toBeNull();
  });
});

describe('chatSwitchRefusal', () => {
  const calm = { sideBusy: false, switching: false, pending: false };

  it('allows a switch when nothing is in flight (a running turn does not block it)', () => {
    expect(chatSwitchRefusal(calm)).toBeNull();
  });

  it('refuses while a side command, a conversation switch, or a send is in flight', () => {
    expect(chatSwitchRefusal({ ...calm, sideBusy: true })).toBe(CHAT_SWITCH_SIDE_BUSY);
    expect(chatSwitchRefusal({ ...calm, switching: true })).toBe(CHAT_SWITCH_SWITCHING);
    expect(chatSwitchRefusal({ ...calm, pending: true })).toBe(CHAT_SWITCH_PENDING);
  });
});

describe('chatCloseRefusal', () => {
  it('never closes the last chat', () => {
    expect(chatCloseRefusal('Chat 1', { count: 1, busy: false })).toBe('The last chat cannot be closed.');
  });

  it('refuses a busy chat, naming it', () => {
    expect(chatCloseRefusal('Chat 2', { count: 2, busy: true })).toBe('Chat 2 is busy. Stop its turn first, then close it.');
  });

  it('allows an idle chat when others exist', () => {
    expect(chatCloseRefusal('Chat 2', { count: 2, busy: false })).toBeNull();
  });
});

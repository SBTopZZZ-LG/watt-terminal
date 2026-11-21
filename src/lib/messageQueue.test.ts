import { describe, expect, it } from 'vitest';
import { drainBatch, enqueue, queueHeader, queuedAttachmentNote, removeQueued, restoreText, shouldDrain } from './messageQueue';
import type { QueuedMessage } from './messageQueue';
import type { AttachmentInput } from './types';

const file = (name: string): AttachmentInput => ({ name, mime: 'text/plain', kind: 'text', data_b64: 'aGk=' });

const q = (id: number, text: string, attachments: AttachmentInput[] = []): QueuedMessage => ({ id, text, attachments });

describe('enqueue and removeQueued', () => {
  it('adds at the end, keeping the order', () => {
    const one = enqueue([], 1, 'first', []);
    const two = enqueue(one, 2, 'second', [file('a.txt')]);
    expect(two).toEqual([q(1, 'first'), q(2, 'second', [file('a.txt')])]);
    expect(one).toEqual([q(1, 'first')]);
  });

  it('removes the message with the id and returns it', () => {
    const queue = [q(1, 'a'), q(2, 'b'), q(3, 'c')];
    expect(removeQueued(queue, 2)).toEqual({ queue: [q(1, 'a'), q(3, 'c')], removed: q(2, 'b') });
  });

  it('an unknown id removes nothing', () => {
    const queue = [q(1, 'a')];
    expect(removeQueued(queue, 9)).toEqual({ queue, removed: null });
    expect(removeQueued([], 1)).toEqual({ queue: [], removed: null });
  });
});

describe('drainBatch', () => {
  it('an empty queue sends nothing', () => {
    expect(drainBatch([], 'all')).toEqual({ send: null, rest: [] });
    expect(drainBatch([], 'one')).toEqual({ send: null, rest: [] });
  });

  it('all: one message joining the texts with a blank line, and nothing stays queued', () => {
    const queue = [q(1, 'first'), q(2, 'second'), q(3, 'third')];
    expect(drainBatch(queue, 'all')).toEqual({ send: { text: 'first\n\nsecond\n\nthird', attachments: [] }, rest: [] });
  });

  it('all: attachments are concatenated in queue order', () => {
    const queue = [q(1, 'a', [file('1')]), q(2, 'b', [file('2'), file('3')]), q(3, 'c')];
    expect(drainBatch(queue, 'all').send).toEqual({ text: 'a\n\nb\n\nc', attachments: [file('1'), file('2'), file('3')] });
  });

  it('all: a message with only attachments adds no blank text', () => {
    const queue = [q(1, '', [file('1')]), q(2, 'hello')];
    expect(drainBatch(queue, 'all').send).toEqual({ text: 'hello', attachments: [file('1')] });
    expect(drainBatch([q(1, '', [file('1')])], 'all').send).toEqual({ text: '', attachments: [file('1')] });
  });

  it('one: only the first message, and the rest stay queued in order', () => {
    const queue = [q(1, 'first', [file('x')]), q(2, 'second'), q(3, 'third')];
    expect(drainBatch(queue, 'one')).toEqual({
      send: { text: 'first', attachments: [file('x')] },
      rest: [q(2, 'second'), q(3, 'third')],
    });
  });

  it('one: a single message leaves nothing queued', () => {
    expect(drainBatch([q(1, 'only')], 'one')).toEqual({ send: { text: 'only', attachments: [] }, rest: [] });
  });
});

describe('restoreText', () => {
  it('an empty input gets the removed text alone', () => {
    expect(restoreText('queued text', '')).toBe('queued text');
    expect(restoreText('queued text', '   ')).toBe('queued text');
  });

  it('a non-empty input keeps its text after the removed text, on a new line', () => {
    expect(restoreText('queued text', 'draft')).toBe('queued text\ndraft');
  });

  it('an attachments-only message restores as an empty text when the input is empty', () => {
    expect(restoreText('', '')).toBe('');
  });
});

describe('shouldDrain', () => {
  it('fires on the active-to-idle edge with messages queued', () => {
    expect(shouldDrain(true, false, 1)).toBe(true);
  });

  it('does not fire with an empty queue', () => {
    expect(shouldDrain(true, false, 0)).toBe(false);
  });

  it('does not fire while the agent or a side command is still active (the caller passes busy or sideBusy)', () => {
    expect(shouldDrain(true, true, 2)).toBe(false);
  });

  it('does not fire on the idle-to-active edge, or while idle', () => {
    expect(shouldDrain(false, true, 2)).toBe(false);
    expect(shouldDrain(false, false, 2)).toBe(false);
  });
});

describe('labels', () => {
  it('the header names the mode', () => {
    expect(queueHeader('all')).toBe('Queued · sent together when the agent finishes');
    expect(queueHeader('one')).toBe('Queued · sent one at a time when the agent finishes');
  });

  it('the attachment note is singular for one', () => {
    expect(queuedAttachmentNote(1)).toBe('1 attachment');
    expect(queuedAttachmentNote(3)).toBe('3 attachments');
  });
});

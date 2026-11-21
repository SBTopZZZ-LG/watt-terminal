import { describe, expect, it } from 'vitest';
import { ATTENTION_BODY_MAX, clip, decideAttention } from './attention';
import type { AttentionContext, AttentionEvent } from './attention';

const AWAY: AttentionContext = { enabled: true, focused: false, queueLength: 0 };
const IDLE: AttentionEvent = { type: 'turn_idle', lastText: 'All done.' };
const QUESTION: AttentionEvent = { type: 'question', text: 'Which branch?' };
const APPROVAL: AttentionEvent = { type: 'approval', text: 'rm -rf build' };

describe('decideAttention: the setting and focus', () => {
  it('is silent for every event while the setting is off', () => {
    const off = { ...AWAY, enabled: false };
    expect(decideAttention(IDLE, off)).toBeNull();
    expect(decideAttention(QUESTION, off)).toBeNull();
    expect(decideAttention(APPROVAL, off)).toBeNull();
  });

  it('is silent for every event while the window has focus', () => {
    const focused = { ...AWAY, focused: true };
    expect(decideAttention(IDLE, focused)).toBeNull();
    expect(decideAttention(QUESTION, focused)).toBeNull();
    expect(decideAttention(APPROVAL, focused)).toBeNull();
  });
});

describe('decideAttention: a finished turn', () => {
  it('is informational, titled, and carries the last reply', () => {
    expect(decideAttention(IDLE, AWAY)).toEqual({
      kind: 'idle',
      title: 'Watt · done',
      body: 'All done.',
      attention: 'informational',
    });
  });

  it('is silent when messages are queued, since the queue is about to send', () => {
    expect(decideAttention(IDLE, { ...AWAY, queueLength: 1 })).toBeNull();
  });

  it('says the agent has finished when there is no reply text', () => {
    expect(decideAttention({ type: 'turn_idle', lastText: null }, AWAY)?.body).toBe('The agent has finished.');
    expect(decideAttention({ type: 'turn_idle', lastText: '  \n\t ' }, AWAY)?.body).toBe('The agent has finished.');
  });
});

describe('decideAttention: a question or an approval', () => {
  it('a question is critical and titled as a question, with the question text', () => {
    expect(decideAttention(QUESTION, AWAY)).toEqual({
      kind: 'question',
      title: 'Watt · the agent has a question',
      body: 'Which branch?',
      attention: 'critical',
    });
  });

  it('an approval is critical and titled as an approval, with the command', () => {
    expect(decideAttention(APPROVAL, AWAY)).toEqual({
      kind: 'approval',
      title: 'Watt · approval needed',
      body: 'rm -rf build',
      attention: 'critical',
    });
  });

  it('a question or an approval ignores the queue', () => {
    const queued = { ...AWAY, queueLength: 3 };
    expect(decideAttention(QUESTION, queued)?.kind).toBe('question');
    expect(decideAttention(APPROVAL, queued)?.kind).toBe('approval');
  });

  it('falls back to a short line when the text is empty', () => {
    expect(decideAttention({ type: 'question', text: '' }, AWAY)?.body).toBe('Open Watt to answer.');
    expect(decideAttention({ type: 'approval', text: '\n' }, AWAY)?.body).toBe('Open Watt to review the command.');
  });
});

describe('clip', () => {
  it('collapses whitespace and newlines to single spaces', () => {
    expect(clip('  first line\n\n\tsecond   line \r\n')).toBe('first line second line');
  });

  it('keeps a text of exactly the limit whole', () => {
    const exact = 'a'.repeat(ATTENTION_BODY_MAX);
    expect(clip(exact)).toBe(exact);
  });

  it('cuts a longer text to the limit with an ellipsis, without a space before it', () => {
    const long = `${'word '.repeat(60)}end`;
    const out = clip(long);
    expect(Array.from(out)).toHaveLength(ATTENTION_BODY_MAX);
    expect(out.endsWith('…')).toBe(true);
    expect(out.endsWith(' …')).toBe(false);
  });

  it('counts code points, so an emoji is never split', () => {
    const text = '😀'.repeat(ATTENTION_BODY_MAX + 5);
    const out = clip(text);
    expect(Array.from(out)).toHaveLength(ATTENTION_BODY_MAX);
    expect(out.slice(0, -1)).toBe('😀'.repeat(ATTENTION_BODY_MAX - 1));
  });
});

import { describe, expect, it } from 'vitest';
import { History, messageFor } from './barInput';

describe('messageFor', () => {
  it('empty and whitespace-only text sends nothing', () => {
    expect(messageFor('')).toBeNull();
    expect(messageFor('   ')).toBeNull();
  });

  it('the message is the text, trimmed', () => {
    expect(messageFor('  what was the last command ')).toBe('what was the last command');
  });

  it('a leading ! is plain text for the agent, not a shell command', () => {
    expect(messageFor('!ls')).toBe('!ls');
    expect(messageFor('!')).toBe('!');
  });

  it('a ! that is not first is part of the message', () => {
    expect(messageFor('why did it fail!')).toBe('why did it fail!');
  });
});

describe('History', () => {
  it('walks back through sent lines and restores the draft going forward', () => {
    const h = new History();
    h.record('what was the last command');
    h.record('why did it fail');

    expect(h.back('half typed')).toBe('why did it fail');
    expect(h.back('why did it fail')).toBe('what was the last command');
    expect(h.back('what was the last command')).toBeNull();
    expect(h.forward()).toBe('why did it fail');
    expect(h.forward()).toBe('half typed');
    expect(h.forward()).toBeNull();
  });

  it('does not record blank lines or repeat the last entry', () => {
    const h = new History();
    h.record('   ');
    h.record('where am I');
    h.record('where am I');
    expect(h.back('')).toBe('where am I');
    expect(h.back('where am I')).toBeNull();
  });
});

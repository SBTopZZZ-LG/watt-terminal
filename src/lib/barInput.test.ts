import { describe, expect, it } from 'vitest';
import { History, historyKeyTakesLine, messageFor, sendButtonFor } from './barInput';

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

describe('historyKeyTakesLine', () => {
  it('single-line text always takes Up and Down, whatever the caret', () => {
    expect(historyKeyTakesLine('up', 'why', 0)).toBe(true);
    expect(historyKeyTakesLine('up', 'why', 3)).toBe(true);
    expect(historyKeyTakesLine('down', 'why', 0)).toBe(true);
    expect(historyKeyTakesLine('down', 'why', 3)).toBe(true);
    expect(historyKeyTakesLine('up', '', 0)).toBe(true);
  });

  it('multi-line text: Up takes history only on the first line', () => {
    const text = 'first\nsecond';
    expect(historyKeyTakesLine('up', text, 0)).toBe(true);
    expect(historyKeyTakesLine('up', text, 5)).toBe(true); // end of the first line
    expect(historyKeyTakesLine('up', text, 6)).toBe(false); // start of the second line
    expect(historyKeyTakesLine('up', text, text.length)).toBe(false);
  });

  it('multi-line text: Down takes history only on the last line', () => {
    const text = 'first\nsecond';
    expect(historyKeyTakesLine('down', text, 0)).toBe(false);
    expect(historyKeyTakesLine('down', text, 5)).toBe(false); // end of the first line, the newline is after the caret
    expect(historyKeyTakesLine('down', text, 6)).toBe(true);
    expect(historyKeyTakesLine('down', text, text.length)).toBe(true);
  });
});

describe('sendButtonFor', () => {
  it('idle with nothing to send: a disabled Send', () => {
    expect(sendButtonFor(false, '', false)).toEqual({ stop: false, disabled: true });
    expect(sendButtonFor(false, '   ', false)).toEqual({ stop: false, disabled: true });
  });

  it('idle with text: an enabled Send', () => {
    expect(sendButtonFor(false, 'why', false)).toEqual({ stop: false, disabled: false });
  });

  it('busy: Stop is enabled even with no text', () => {
    expect(sendButtonFor(true, '', false)).toEqual({ stop: true, disabled: false });
    expect(sendButtonFor(true, 'typing ahead', false)).toEqual({ stop: true, disabled: false });
  });

  it('locked disables the button in either state', () => {
    expect(sendButtonFor(false, 'why', true)).toEqual({ stop: false, disabled: true });
    expect(sendButtonFor(true, '', true)).toEqual({ stop: true, disabled: true });
  });
});

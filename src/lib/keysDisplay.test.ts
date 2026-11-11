import { describe, expect, it } from 'vitest';
import { capDisplay, displayText, sendKeysDisplay } from './keysDisplay';

describe('sendKeysDisplay', () => {
  it('shows the text, then each key in the short form the approval card uses', () => {
    expect(sendKeysDisplay({ text: 'echo hi', keys: ['Enter'] })).toBe('echo hi⏎');
    expect(sendKeysDisplay({ text: ':wq', keys: ['Enter'] })).toBe(':wq⏎');
    expect(sendKeysDisplay({ keys: ['Ctrl-C'] })).toBe('^C');
    expect(sendKeysDisplay({ keys: ['Up', 'Esc'] })).toBe('<Up><Esc>');
  });

  it('matches key names without regard to case, as the backend does', () => {
    expect(sendKeysDisplay({ keys: ['enter', 'ctrl-d', 'SPACE'] })).toBe('⏎^D ');
  });

  it('is empty for no arguments, and skips anything that is not a string', () => {
    expect(sendKeysDisplay(null)).toBe('');
    expect(sendKeysDisplay('echo')).toBe('');
    expect(sendKeysDisplay({ text: 5, keys: ['Enter', 7] })).toBe('⏎');
  });
});

describe('displayText', () => {
  it('keeps the Enter mark and escapes every other non-ASCII character', () => {
    expect(displayText('echo hi⏎')).toBe('echo hi⏎');
    expect(displayText('a‮b⏎')).toBe('a\\u{202E}b⏎');
  });

  it('shows a control character as an escape, so it cannot hide', () => {
    expect(displayText('x\tY')).toBe('x\\u{9}Y');
  });

  it('leaves printable ASCII alone', () => {
    expect(displayText('ls -la | wc')).toBe('ls -la | wc');
  });
});

describe('capDisplay', () => {
  it('keeps a short text as displayText shows it', () => {
    expect(capDisplay('echo⏎', 10)).toBe('echo⏎');
  });

  it('cuts a long text to the limit, with a literal ellipsis, counting characters not bytes', () => {
    expect(capDisplay('abcdefghij', 5)).toBe('abcd…');
    expect(capDisplay('é'.repeat(6), 5)).toBe('\\u{E9}\\u{E9}\\u{E9}\\u{E9}…');
  });

  it('escapes the kept part and never splits an escape', () => {
    expect(capDisplay('a‮bcd', 3)).toBe('a\\u{202E}…');
  });
});

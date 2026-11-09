import { describe, expect, it } from 'vitest';
import { clearScreenDecision, decideSubmit, History, isClearScreenKey, modeLabel, parseBarInput } from './barInput';

describe('parseBarInput', () => {
  it('empty and whitespace-only text is empty', () => {
    expect(parseBarInput('')).toEqual({ kind: 'empty' });
    expect(parseBarInput('   ')).toEqual({ kind: 'empty' });
  });

  it('a leading ! is a shell command, with the ! and one optional space removed', () => {
    expect(parseBarInput('!ls')).toEqual({ kind: 'shell', command: 'ls' });
    expect(parseBarInput('! ls -la')).toEqual({ kind: 'shell', command: 'ls -la' });
  });

  it('only one space after the ! is removed', () => {
    expect(parseBarInput('!  ls')).toEqual({ kind: 'shell', command: ' ls' });
  });

  it('!! is passed on as the shell command "!", with no special meaning', () => {
    expect(parseBarInput('!!')).toEqual({ kind: 'shell', command: '!' });
  });

  it('a ! with nothing to run after it is rejected', () => {
    expect(parseBarInput('!')).toEqual({ kind: 'shell_invalid', reason: 'Nothing to run after !' });
    expect(parseBarInput('! ')).toEqual({ kind: 'shell_invalid', reason: 'Nothing to run after !' });
  });

  it('anything else is an agent message, trimmed', () => {
    expect(parseBarInput('  what was the last command ')).toEqual({
      kind: 'agent',
      text: 'what was the last command',
    });
  });

  it('a ! that is not first is part of an agent message', () => {
    expect(parseBarInput('why did it fail!')).toEqual({ kind: 'agent', text: 'why did it fail!' });
  });

  it('the mode label follows the leading !', () => {
    expect(modeLabel('!ls')).toBe('shell');
    expect(modeLabel('ls')).toBe('agent');
    expect(modeLabel('')).toBe('agent');
  });
});

describe('decideSubmit', () => {
  it('shell commands run only from the full bar (the prompt)', () => {
    expect(decideSubmit('full', null, '!ls')).toEqual({ kind: 'shell', command: 'ls' });
  });

  it('agent-only, collapsed and off bars refuse shell commands with a reason', () => {
    expect(decideSubmit('agent_only', 'A command is running.', '!ls')).toEqual({
      kind: 'reject',
      reason: 'Shell commands need an idle prompt.',
    });
    expect(decideSubmit('collapsed', null, '!ls')).toEqual({
      kind: 'reject',
      reason: 'Shell commands need an idle prompt.',
    });
    expect(decideSubmit('off', 'Starting the shell.', '!ls')).toEqual({ kind: 'reject', reason: 'Starting the shell.' });
  });

  it('agent messages go through the full and agent-only bars', () => {
    expect(decideSubmit('full', null, 'hi')).toEqual({ kind: 'agent', text: 'hi' });
    expect(decideSubmit('agent_only', 'hint', 'hi')).toEqual({ kind: 'agent', text: 'hi' });
  });

  it('agent messages are refused by an off bar, with its hint', () => {
    expect(decideSubmit('off', 'Waiting for the command to start...', 'hi')).toEqual({
      kind: 'reject',
      reason: 'Waiting for the command to start...',
    });
  });

  it('empty text does nothing', () => {
    expect(decideSubmit('full', null, '   ')).toEqual({ kind: 'none' });
  });

  it('a lone ! is rejected even at the prompt', () => {
    expect(decideSubmit('full', null, '!')).toEqual({ kind: 'reject', reason: 'Nothing to run after !' });
  });
});

describe('History', () => {
  it('walks back through sent lines and restores the draft going forward', () => {
    const h = new History();
    h.record('!ls');
    h.record('what happened');

    expect(h.back('half typed')).toBe('what happened');
    expect(h.back('what happened')).toBe('!ls');
    expect(h.back('!ls')).toBeNull();
    expect(h.forward()).toBe('what happened');
    expect(h.forward()).toBe('half typed');
    expect(h.forward()).toBeNull();
  });

  it('does not record blank lines or repeat the last entry', () => {
    const h = new History();
    h.record('   ');
    h.record('!ls');
    h.record('!ls');
    expect(h.back('')).toBe('!ls');
    expect(h.back('!ls')).toBeNull();
  });
});

describe('isClearScreenKey', () => {
  const none = { ctrl: false, shift: false, alt: false, meta: false };
  const ctrl = { ...none, ctrl: true };

  it('Ctrl+L and Ctrl+l are the clear-screen key', () => {
    expect(isClearScreenKey('l', ctrl)).toBe(true);
    expect(isClearScreenKey('L', ctrl)).toBe(true);
  });

  it('any other modifier, or no Ctrl, is not the clear-screen key', () => {
    expect(isClearScreenKey('L', { ...ctrl, shift: true })).toBe(false);
    expect(isClearScreenKey('l', { ...ctrl, alt: true })).toBe(false);
    expect(isClearScreenKey('l', { ...ctrl, meta: true })).toBe(false);
    expect(isClearScreenKey('l', none)).toBe(false);
  });

  it('other keys are not the clear-screen key', () => {
    expect(isClearScreenKey('k', ctrl)).toBe(false);
    expect(isClearScreenKey('Enter', ctrl)).toBe(false);
    expect(isClearScreenKey('c', ctrl)).toBe(false);
  });
});

describe('clearScreenDecision', () => {
  it('sends Ctrl+L only from the idle prompt, and not while a line is being sent', () => {
    expect(clearScreenDecision('full', false)).toBe('send');
    expect(clearScreenDecision('full', true)).toBe('ignore');
  });

  it('ignores Ctrl+L in every other bar mode (running, alt screen, booting, off, ...)', () => {
    for (const bar of ['agent_only', 'collapsed', 'off'] as const) {
      expect(clearScreenDecision(bar, false), bar).toBe('ignore');
    }
  });
});

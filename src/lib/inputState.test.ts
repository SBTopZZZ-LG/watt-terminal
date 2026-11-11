import { describe, expect, it } from 'vitest';
import {
  barKeyIntent,
  effectiveState,
  focusAfter,
  inputReducer,
  INITIAL_INPUT,
  isFocusBarHotkey,
  routeFor,
} from './inputState';
import type { InputModel } from './inputState';
import type { Foreground, InputState } from './types';

const ALL_STATES: InputState[] = [
  'booting',
  'prompt',
  'submitted',
  'running',
  'continuation',
  'alt',
  'raw',
  'exited',
];

function model(rust: InputState, alt = false): InputModel {
  return { rust, alt, foreground: null };
}

const SSH: Foreground = { kind: 'ssh', command: 'ssh ubuntu@lidia' };

describe('inputReducer and effectiveState', () => {
  it('starts booting with no alt screen', () => {
    expect(INITIAL_INPUT).toEqual({ rust: 'booting', alt: false, foreground: null });
    expect(effectiveState(INITIAL_INPUT)).toBe('booting');
  });

  it('takes the state Rust reports', () => {
    let m = INITIAL_INPUT;
    m = inputReducer(m, { type: 'rust_state', state: 'prompt' });
    expect(effectiveState(m)).toBe('prompt');
    m = inputReducer(m, { type: 'rust_state', state: 'running' });
    expect(effectiveState(m)).toBe('running');
  });

  it('returns the same object when nothing changed', () => {
    const m = model('prompt');
    expect(inputReducer(m, { type: 'rust_state', state: 'prompt' })).toBe(m);
    expect(inputReducer(m, { type: 'rust_state', state: 'prompt', foreground: null })).toBe(m);
    expect(inputReducer(m, { type: 'alt', active: false })).toBe(m);
  });

  it('takes the foreground hint with the running state, and a state event without one clears it', () => {
    let m = model('prompt');
    m = inputReducer(m, { type: 'rust_state', state: 'running', foreground: SSH });
    expect(m.foreground).toEqual(SSH);
    expect(routeFor(effectiveState(m), m.foreground).hint).toBe(
      'ssh ubuntu@lidia is running without shell integration. The agent can read the screen but cannot run commands here.',
    );
    // The same state and the same hint: nothing changes, so the same object comes back.
    expect(inputReducer(m, { type: 'rust_state', state: 'running', foreground: { ...SSH } })).toBe(m);
    // Back at the prompt the state event carries no hint, and the hint goes with it.
    m = inputReducer(m, { type: 'rust_state', state: 'prompt' });
    expect(m.foreground).toBeNull();
    expect(effectiveState(m)).toBe('prompt');
  });

  it('a changed command with the same state is a change of the hint', () => {
    const m = inputReducer(model('running'), { type: 'rust_state', state: 'running', foreground: SSH });
    const next = inputReducer(m, { type: 'rust_state', state: 'running', foreground: { kind: 'shell', command: 'zsh -f -i' } });
    expect(next).not.toBe(m);
    expect(next.foreground).toEqual({ kind: 'shell', command: 'zsh -f -i' });
  });

  it('the alt screen keeps the hint in the model, and the alt route does not show it', () => {
    const alt = inputReducer(inputReducer(model('running'), { type: 'rust_state', state: 'running', foreground: SSH }), {
      type: 'alt',
      active: true,
    });
    expect(effectiveState(alt)).toBe('alt');
    expect(routeFor(effectiveState(alt), alt.foreground).hint).not.toContain('without shell integration');
  });

  it('the alt screen overrides every Rust state, immediately, and leaving it restores the Rust state', () => {
    for (const s of ALL_STATES.filter((s) => s !== 'exited')) {
      const entered = inputReducer(model(s), { type: 'alt', active: true });
      expect(effectiveState(entered), `alt over ${s}`).toBe('alt');
      const left = inputReducer(entered, { type: 'alt', active: false });
      expect(effectiveState(left), `after alt, over ${s}`).toBe(s);
    }
  });

  it('exited wins over the alt screen', () => {
    expect(effectiveState(model('exited', true))).toBe('exited');
  });
});

describe('routeFor: the input routing table', () => {
  it.each([
    ['booting', 'pty', 'chat'],
    ['prompt', 'pty', 'chat'],
    ['submitted', 'pty', 'chat'],
    ['continuation', 'pty', 'chat'],
    ['running', 'pty', 'chat'],
    ['alt', 'pty', 'collapsed'],
    ['raw', 'pty', 'chat'],
    ['exited', 'drop', 'exited'],
  ] as const)('%s: keys %s, bar %s', (state, keys, bar) => {
    const route = routeFor(state);
    expect(route.keys).toBe(keys);
    expect(route.bar).toBe(bar);
  });

  it('sends keys to the PTY in every state except exited', () => {
    for (const s of ALL_STATES) {
      expect(routeFor(s).keys, s).toBe(s === 'exited' ? 'drop' : 'pty');
    }
  });

  it('the bar is the agent chat in every state except the alt screen and an exited shell', () => {
    for (const s of ALL_STATES) {
      const expected = s === 'alt' ? 'collapsed' : s === 'exited' ? 'exited' : 'chat';
      expect(routeFor(s).bar, s).toBe(expected);
    }
  });

  it('gives a hint for every state except the prompt, where nothing needs explaining', () => {
    for (const s of ALL_STATES) {
      expect(routeFor(s).hint === null, s).toBe(s === 'prompt');
    }
  });

  it('a program without shell integration changes the running hint, and nothing else', () => {
    const generic = 'A command is running. The agent cannot run another until it finishes.';
    expect(routeFor('running').hint).toBe(generic);
    expect(routeFor('running', null).hint).toBe(generic);
    expect(routeFor('running', { kind: 'other', command: 'sleep 2' }).hint).toBe(generic);
    expect(routeFor('running', SSH).hint).toContain('is running without shell integration');
    // Only the running state has the hint: the others are the same with or without one.
    for (const s of ALL_STATES.filter((s) => s !== 'running')) {
      expect(routeFor(s, SSH), s).toEqual(routeFor(s));
    }
    // Still inline, so the bar keeps its height, and the keys still go to the PTY.
    expect(routeFor('running', SSH).hintInline).toBe(true);
    expect(routeFor('running', SSH).keys).toBe('pty');
  });

  it('shows the hint inline (as the placeholder) only while a command is starting or running', () => {
    // These two flip on every command, so they must not add a row: the bar would grow and the terminal would resize.
    for (const s of ALL_STATES) {
      expect(routeFor(s).hintInline, s).toBe(s === 'submitted' || s === 'running');
    }
  });
});

describe('routeFor: unresponsive', () => {
  it('keys still go to the shell (so Ctrl-C reaches it), and the bar says the shell is not answering', () => {
    const route = routeFor('unresponsive', null);
    expect(route.keys).toBe('pty');
    expect(route.bar).toBe('chat');
    expect(route.hint).toBe('The shell is not answering. Ctrl-C to interrupt, or wait.');
    expect(route.hintInline).toBe(false);
  });
});

describe('focusAfter', () => {
  it('never moves focus for a state change between live states', () => {
    // The bar keeps focus while commands start, run, finish and while programs take the keyboard.
    const live = ALL_STATES.filter((s) => s !== 'exited');
    for (const prev of live) {
      for (const next of live) {
        expect(focusAfter(prev, next), `${prev} -> ${next}`).toBeNull();
      }
    }
  });

  it('a command the agent started does not move focus, so the bar keeps it while the command runs', () => {
    expect(focusAfter('submitted', 'running')).toBeNull();
    expect(focusAfter('running', 'prompt')).toBeNull();
    expect(focusAfter('alt', 'prompt')).toBeNull();
  });

  it('moves focus to the bar when the shell exits, so its restart button can take Enter', () => {
    expect(focusAfter('running', 'exited')).toBe('bar');
    expect(focusAfter('prompt', 'exited')).toBe('bar');
    expect(focusAfter('alt', 'exited')).toBe('bar');
  });

  it('moves focus to the terminal when a new shell starts after an exit', () => {
    expect(focusAfter('exited', 'booting')).toBe('xterm');
    expect(focusAfter('exited', 'prompt')).toBe('xterm');
  });

  it('does nothing when the state stays the same', () => {
    expect(focusAfter('prompt', 'prompt')).toBeNull();
    expect(focusAfter('exited', 'exited')).toBeNull();
  });
});

describe('barKeyIntent', () => {
  const none = { inputEmpty: true, hasPendingApproval: false };

  it('Ctrl-C is an interrupt in every state, with or without text or approvals', () => {
    for (const s of ALL_STATES) {
      // The key mapping does not depend on the state, so the state is taken through effectiveState to show it.
      const eff = effectiveState(model(s));
      for (const ctx of [none, { inputEmpty: false, hasPendingApproval: true }]) {
        expect(barKeyIntent('c', { ctrl: true, shift: false }, ctx), eff).toBe('interrupt');
      }
    }
  });

  it('Ctrl-Shift-C is not an interrupt', () => {
    expect(barKeyIntent('C', { ctrl: true, shift: true }, none)).toBe('none');
  });

  it('Enter approves the first pending approval only when the input is empty', () => {
    expect(barKeyIntent('Enter', { ctrl: false, shift: false }, { inputEmpty: true, hasPendingApproval: true })).toBe(
      'approve_first',
    );
    expect(barKeyIntent('Enter', { ctrl: false, shift: false }, { inputEmpty: false, hasPendingApproval: true })).toBe(
      'submit',
    );
    expect(barKeyIntent('Enter', { ctrl: false, shift: false }, none)).toBe('submit');
  });

  it('Shift-Enter is not a submit or an approval: it is none, so the textarea inserts a newline', () => {
    expect(barKeyIntent('Enter', { ctrl: false, shift: true }, none)).toBe('none');
    expect(barKeyIntent('Enter', { ctrl: false, shift: true }, { inputEmpty: false, hasPendingApproval: false })).toBe(
      'none',
    );
    expect(barKeyIntent('Enter', { ctrl: false, shift: true }, { inputEmpty: true, hasPendingApproval: true })).toBe(
      'none',
    );
  });

  it('Escape denies a pending approval, otherwise it closes the bar (focus back to the terminal)', () => {
    expect(barKeyIntent('Escape', { ctrl: false, shift: false }, { inputEmpty: true, hasPendingApproval: true })).toBe(
      'deny_first',
    );
    expect(barKeyIntent('Escape', { ctrl: false, shift: false }, none)).toBe('close');
  });

  it('arrows walk history', () => {
    expect(barKeyIntent('ArrowUp', { ctrl: false, shift: false }, none)).toBe('history_prev');
    expect(barKeyIntent('ArrowDown', { ctrl: false, shift: false }, none)).toBe('history_next');
  });
});

describe('isFocusBarHotkey', () => {
  const press = (key: string, mods: { ctrl?: boolean; meta?: boolean; shift?: boolean }) => ({
    key,
    ctrlKey: mods.ctrl ?? false,
    metaKey: mods.meta ?? false,
    shiftKey: mods.shift ?? false,
  });

  it('accepts Ctrl+Shift+A and Cmd+Shift+A, in either letter case', () => {
    expect(isFocusBarHotkey(press('a', { ctrl: true, shift: true }))).toBe(true);
    expect(isFocusBarHotkey(press('A', { ctrl: true, shift: true }))).toBe(true);
    expect(isFocusBarHotkey(press('a', { meta: true, shift: true }))).toBe(true);
    expect(isFocusBarHotkey(press('A', { meta: true, shift: true }))).toBe(true);
  });

  it('needs Shift and the letter A', () => {
    expect(isFocusBarHotkey(press('a', { ctrl: true }))).toBe(false);
    expect(isFocusBarHotkey(press('a', { meta: true }))).toBe(false);
    expect(isFocusBarHotkey(press('b', { ctrl: true, shift: true }))).toBe(false);
    expect(isFocusBarHotkey(press('a', { shift: true }))).toBe(false);
  });

  it('leaves Escape to the bar: Escape is not a focus hotkey, and the bar still closes on it', () => {
    expect(isFocusBarHotkey(press('Escape', { ctrl: true, shift: true }))).toBe(false);
    expect(barKeyIntent('Escape', { ctrl: false, shift: false }, { inputEmpty: true, hasPendingApproval: false })).toBe(
      'close',
    );
  });
});

import { describe, expect, it } from 'vitest';
import {
  barKeyIntent,
  effectiveState,
  focusAfter,
  inputReducer,
  INITIAL_INPUT,
  routeFor,
} from './inputState';
import type { InputModel } from './inputState';
import type { InputState } from './types';

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
  return { rust, alt };
}

describe('inputReducer and effectiveState', () => {
  it('starts booting with no alt screen', () => {
    expect(INITIAL_INPUT).toEqual({ rust: 'booting', alt: false });
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
    expect(inputReducer(m, { type: 'alt', active: false })).toBe(m);
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

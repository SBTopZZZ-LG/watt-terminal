import { describe, expect, it } from 'vitest';
import { clearActions } from './clear';

describe('clearActions', () => {
  it('does nothing on the alternate buffer, whatever the state', () => {
    for (const state of ['prompt', 'running', 'exited', 'booting', 'raw', 'continuation', 'submitted', 'alt'] as const) {
      expect(clearActions('alternate', state)).toEqual({ local3J: false, ctrlL: false });
    }
  });

  it('does nothing in the alt state, even on the normal buffer', () => {
    expect(clearActions('normal', 'alt')).toEqual({ local3J: false, ctrlL: false });
  });

  it('clears locally and redraws at the prompt', () => {
    expect(clearActions('normal', 'prompt')).toEqual({ local3J: true, ctrlL: true });
  });

  it('clears locally only while a command runs', () => {
    expect(clearActions('normal', 'running')).toEqual({ local3J: true, ctrlL: false });
  });

  it('clears locally only after the shell exited', () => {
    expect(clearActions('normal', 'exited')).toEqual({ local3J: true, ctrlL: false });
  });

  it('clears locally only while booting', () => {
    expect(clearActions('normal', 'booting')).toEqual({ local3J: true, ctrlL: false });
  });

  it('never sends Ctrl-L in submitted, continuation or raw states', () => {
    for (const state of ['submitted', 'continuation', 'raw'] as const) {
      expect(clearActions('normal', state)).toEqual({ local3J: true, ctrlL: false });
    }
  });
});

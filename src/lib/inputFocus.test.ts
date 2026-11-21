import { describe, expect, it } from 'vitest';
import { shouldRestoreInputFocus } from './inputFocus';
import type { FocusRestoreContext } from './inputFocus';

const FREE: FocusRestoreContext = { inputWasFocused: true, focusLost: true, dialogOpen: false, questionPending: false };

describe('shouldRestoreInputFocus', () => {
  it('restores when the input had focus, focus was lost, and no blocker is up', () => {
    expect(shouldRestoreInputFocus(FREE)).toBe(true);
  });

  it('does nothing when the input never had focus (the terminal keeps it)', () => {
    expect(shouldRestoreInputFocus({ ...FREE, inputWasFocused: false })).toBe(false);
  });

  it('does nothing while something else holds focus on purpose', () => {
    expect(shouldRestoreInputFocus({ ...FREE, focusLost: false })).toBe(false);
  });

  it('does nothing while a modal dialog is open', () => {
    expect(shouldRestoreInputFocus({ ...FREE, dialogOpen: true })).toBe(false);
  });

  it('does nothing while a question waits for the user', () => {
    expect(shouldRestoreInputFocus({ ...FREE, questionPending: true })).toBe(false);
  });
});

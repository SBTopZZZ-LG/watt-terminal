import { describe, expect, it } from 'vitest';
import type { InputState } from './types';
import { altPanelStep, type AltPanelEvent, type AltPanelState, INITIAL_ALT_PANEL, layoutToSave } from './altPanel';
import { defaultLayout } from './panelLayout';

/** A UI state event. `open` is the panel's open state at that moment. */
const at = (state: InputState, open: boolean): AltPanelEvent => ({ type: 'state', state, open });
const enter = (open: boolean) => at('alt', open);
const leave = (open: boolean) => at('running', open);
const USER: AltPanelEvent = { type: 'user' };

/** Runs the events in order from `start`, returning the final state and the open value each step gave. */
function run(events: AltPanelEvent[], start: AltPanelState = INITIAL_ALT_PANEL) {
  let state = start;
  const opens: (boolean | null)[] = [];
  for (const event of events) {
    const step = altPanelStep(state, event);
    state = step.state;
    opens.push(step.open);
  }
  return { state, opens };
}

describe('altPanelStep', () => {
  it('closes an open panel when the alternate screen starts, and remembers it', () => {
    const { state, opens } = run([enter(true)]);
    expect(opens).toEqual([false]);
    expect(state).toEqual({ fullScreen: true, hidden: true, userControl: false });
  });

  it('leaves a panel that is already closed alone, and does not remember it', () => {
    const { state, opens } = run([enter(false), leave(false)]);
    expect(opens).toEqual([null, null]);
    expect(state.hidden).toBe(false);
  });

  it('reopens the panel when the alternate screen ends, if it closed it', () => {
    const { state, opens } = run([enter(true), leave(false)]);
    expect(opens).toEqual([false, true]);
    expect(state).toEqual({ fullScreen: false, hidden: false, userControl: false });
  });

  it('does nothing when the alternate screen repeats its own state', () => {
    const { state, opens } = run([enter(true), enter(false)]);
    expect(opens).toEqual([false, null]);
    expect(state.hidden).toBe(true);
  });

  it('restores the panel when the shell exits during the full-screen run', () => {
    const { opens } = run([enter(true), at('exited', false)]);
    expect(opens).toEqual([false, true]);
  });

  it('handles entering twice quickly: hide, restore, hide again', () => {
    const { opens } = run([enter(true), at('prompt', false), enter(true), at('prompt', false)]);
    expect(opens).toEqual([false, true, false, true]);
  });

  it('keeps a panel the user showed open at the end, and does not re-hide it on a flap', () => {
    // Entered with the panel open: it is hidden. The user shows it again, the program flaps out and back in.
    const { state, opens } = run([enter(true), USER, leave(true), enter(true)]);
    expect(opens).toEqual([false, null, null, null]);
    expect(state).toEqual({ fullScreen: true, hidden: false, userControl: true });
  });

  it('does not reopen a panel the user showed and then hid again during the run', () => {
    const { state, opens } = run([enter(true), USER, leave(false)]);
    expect(opens).toEqual([false, null, null]);
    expect(state.hidden).toBe(false);
  });

  it('hides again on a later run once the command has ended', () => {
    const { opens } = run([enter(true), USER, leave(true), at('prompt', true), enter(true), leave(false)]);
    expect(opens).toEqual([false, null, null, null, false, true]);
  });

  it('re-hides on each flap when the user has not taken control', () => {
    const { opens } = run([enter(true), leave(true), enter(true), leave(true), enter(true)]);
    expect(opens).toEqual([false, true, false, true, false]);
  });

  it('ignores a user action outside a full-screen run', () => {
    const { state, opens } = run([USER]);
    expect(opens).toEqual([null]);
    expect(state).toEqual(INITIAL_ALT_PANEL);
  });

  it('ignores ordinary command states that are not the alternate screen', () => {
    const { state, opens } = run([at('prompt', true), at('running', true), at('submitted', true)]);
    expect(opens).toEqual([null, null, null]);
    expect(state).toEqual(INITIAL_ALT_PANEL);
  });

  it('does not restore on a user action that comes after the screen has ended', () => {
    const { opens } = run([enter(true), leave(false), USER, leave(false)]);
    expect(opens).toEqual([false, true, null, null]);
  });
});

describe('layoutToSave', () => {
  it('saves open while the panel is hidden for a full-screen program', () => {
    const closed = { ...defaultLayout(), open: false };
    const hidden: AltPanelState = { fullScreen: true, hidden: true, userControl: false };
    expect(layoutToSave(closed, hidden)).toEqual({ ...closed, open: true });
  });

  it('saves the layout as it is otherwise, with the same object', () => {
    const layout = defaultLayout();
    expect(layoutToSave(layout, INITIAL_ALT_PANEL)).toBe(layout);
    const closed = { ...layout, open: false };
    expect(layoutToSave(closed, { fullScreen: true, hidden: false, userControl: true })).toBe(closed);
  });

  it('keeps the dock and the sizes when it saves open', () => {
    const layout = { dock: 'left' as const, sizes: { left: 500, right: null }, open: false };
    const saved = layoutToSave(layout, { fullScreen: true, hidden: true, userControl: false });
    expect(saved).toEqual({ dock: 'left', sizes: { left: 500, right: null }, open: true });
  });
});

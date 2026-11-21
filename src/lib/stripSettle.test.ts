import { describe, expect, it } from 'vitest';
import { holdsStrip, sameStripView, settleStrip, stepStrip, STRIP_SETTLE_MS } from './stripSettle';
import type { StripSettle, StripView } from './stripSettle';
import type { Foreground, InputState } from './types';

const PROMPT: StripView = { state: 'prompt', foreground: null };
const SUBMITTED: StripView = { state: 'submitted', foreground: null };
const RUNNING: StripView = { state: 'running', foreground: null };
const SSH: Foreground = { kind: 'ssh', command: 'ssh ubuntu@lidia' };
const RUNNING_SSH: StripView = { state: 'running', foreground: SSH };

const AT_PROMPT: StripSettle = { shown: PROMPT, heldSince: null };

describe('STRIP_SETTLE_MS', () => {
  it('is 400 ms', () => {
    expect(STRIP_SETTLE_MS).toBe(400);
  });
});

describe('sameStripView', () => {
  it('compares the state and the foreground hint by kind and command', () => {
    expect(sameStripView(RUNNING, { state: 'running', foreground: null })).toBe(true);
    expect(sameStripView(RUNNING_SSH, { state: 'running', foreground: { ...SSH } })).toBe(true);
    expect(sameStripView(RUNNING, RUNNING_SSH)).toBe(false);
    expect(sameStripView(RUNNING, SUBMITTED)).toBe(false);
    expect(sameStripView(RUNNING_SSH, { state: 'running', foreground: { kind: 'ssh', command: 'ssh other' } })).toBe(false);
  });
});

describe('holdsStrip: only leaving the prompt for a command is held', () => {
  it('prompt -> submitted and prompt -> running are held', () => {
    expect(holdsStrip(PROMPT, SUBMITTED)).toBe(true);
    expect(holdsStrip(PROMPT, RUNNING_SSH)).toBe(true);
  });

  it('the other states show at once, from the prompt or not', () => {
    const others: InputState[] = ['booting', 'continuation', 'unresponsive', 'alt', 'raw', 'exited'];
    for (const state of others) {
      expect(holdsStrip(PROMPT, { state, foreground: null })).toBe(false);
    }
    expect(holdsStrip(PROMPT, PROMPT)).toBe(false);
  });

  it('a change between non-prompt states is never held', () => {
    expect(holdsStrip(SUBMITTED, RUNNING)).toBe(false);
    expect(holdsStrip(RUNNING, RUNNING_SSH)).toBe(false);
    expect(holdsStrip(RUNNING, PROMPT)).toBe(false);
    expect(holdsStrip({ state: 'raw', foreground: null }, RUNNING)).toBe(false);
  });
});

describe('settleStrip', () => {
  it('prompt -> running: the prompt is shown until 400 ms, then the command', () => {
    expect(settleStrip(PROMPT, RUNNING, 0)).toBe(PROMPT);
    expect(settleStrip(PROMPT, RUNNING, 399)).toBe(PROMPT);
    expect(settleStrip(PROMPT, RUNNING, 400)).toBe(RUNNING);
    expect(settleStrip(PROMPT, RUNNING, 5000)).toBe(RUNNING);
  });

  it('the foreground chip and the Integrate offer come with the command, after the same 400 ms', () => {
    expect(settleStrip(PROMPT, RUNNING_SSH, 399)).toEqual(PROMPT);
    expect(settleStrip(PROMPT, RUNNING_SSH, 400)).toEqual(RUNNING_SSH);
  });

  it('returning to the prompt is immediate, from any state', () => {
    expect(settleStrip(RUNNING_SSH, PROMPT, 0)).toBe(PROMPT);
    expect(settleStrip(SUBMITTED, PROMPT, 0)).toBe(PROMPT);
    expect(settleStrip(PROMPT, PROMPT, 0)).toBe(PROMPT);
  });

  it('a change between non-prompt states is immediate', () => {
    expect(settleStrip(RUNNING, { state: 'running', foreground: SSH }, 0)).toEqual(RUNNING_SSH);
    expect(settleStrip(RUNNING_SSH, { state: 'running', foreground: null }, 0)).toEqual(RUNNING);
    expect(settleStrip(SUBMITTED, RUNNING, 0)).toBe(RUNNING);
  });

  it('states the user must see at once are never held', () => {
    const unresponsive: StripView = { state: 'unresponsive', foreground: null };
    expect(settleStrip(PROMPT, unresponsive, 0)).toBe(unresponsive);
    expect(settleStrip(PROMPT, { state: 'exited', foreground: null }, 0).state).toBe('exited');
    expect(settleStrip(PROMPT, { state: 'raw', foreground: null }, 0).state).toBe('raw');
    expect(settleStrip(PROMPT, { state: 'continuation', foreground: null }, 0).state).toBe('continuation');
  });
});

/** Drives stepStrip through a timeline of [now, incoming] pairs and returns what the strip shows after each. */
function run(events: Array<[number, StripView]>, start: StripSettle = AT_PROMPT): StripView[] {
  let settle = start;
  return events.map(([now, incoming]) => {
    settle = stepStrip(settle, incoming, now).next;
    return settle.shown;
  });
}

describe('stepStrip: quick commands never show', () => {
  it('prompt -> submitted -> running -> prompt inside 400 ms: the prompt stays on screen throughout', () => {
    const shown = run([
      [0, SUBMITTED],
      [60, RUNNING],
      [200, PROMPT],
    ]);
    expect(shown).toEqual([PROMPT, PROMPT, PROMPT]);
  });

  it('a second command right after the first is still held', () => {
    const shown = run([
      [0, SUBMITTED],
      [100, PROMPT],
      [150, SUBMITTED],
      [300, RUNNING],
      [330, PROMPT],
    ]);
    expect(shown.every((s) => s.state === 'prompt')).toBe(true);
  });

  it('reports the recheck time while holding, and none once back at the prompt', () => {
    const a = stepStrip(AT_PROMPT, SUBMITTED, 0);
    expect(a.recheckMs).toBe(400);
    const b = stepStrip(a.next, RUNNING, 100);
    expect(b.recheckMs).toBe(300);
    const c = stepStrip(b.next, PROMPT, 200);
    expect(c.recheckMs).toBeNull();
    expect(c.next).toEqual({ shown: PROMPT, heldSince: null });
  });
});

describe('stepStrip: long or interactive commands show after the settle time', () => {
  it('submitted at 0, still running at 400: the command is shown', () => {
    let settle = stepStrip(AT_PROMPT, SUBMITTED, 0).next;
    expect(settle.shown).toEqual(PROMPT);
    settle = stepStrip(settle, RUNNING, 400).next;
    expect(settle.shown).toEqual(RUNNING);
  });

  it('the settle clock starts when the prompt is left, not when the state changes again', () => {
    // submitted at 0, running at 350: shown at 400, not at 750.
    const shown = run([
      [0, SUBMITTED],
      [350, RUNNING],
      [400, RUNNING],
    ]);
    expect(shown).toEqual([PROMPT, PROMPT, RUNNING]);
  });

  it('an ssh started quickly shows the foreground chip and the Integrate offer at 400 ms', () => {
    const shown = run([
      [0, SUBMITTED],
      [50, RUNNING_SSH],
      [399, RUNNING_SSH],
      [400, RUNNING_SSH],
    ]);
    expect(shown).toEqual([PROMPT, PROMPT, PROMPT, RUNNING_SSH]);
  });

  it('the recheck delay counts down from the start of the hold', () => {
    const a = stepStrip(AT_PROMPT, SUBMITTED, 1000);
    const b = stepStrip(a.next, RUNNING_SSH, 1250);
    expect(a.recheckMs).toBe(400);
    expect(b.recheckMs).toBe(150);
  });

  it('a long command that then returns to the prompt is shown as running, then the prompt at once', () => {
    const shown = run([
      [0, SUBMITTED],
      [400, RUNNING],
      [3000, PROMPT],
    ]);
    expect(shown).toEqual([PROMPT, RUNNING, PROMPT]);
  });

  it('once the command is shown, its change to another command is immediate', () => {
    const start: StripSettle = { shown: RUNNING, heldSince: null };
    const shown = run(
      [
        [0, RUNNING_SSH],
        [10, RUNNING],
      ],
      start,
    );
    expect(shown).toEqual([RUNNING_SSH, RUNNING]);
  });
});

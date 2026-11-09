import { describe, expect, it } from 'vitest';
import { createFitter, MAX_FIT_PASSES } from './fitter';
import type { FitDeps, Size } from './fitter';

interface Fake {
  deps: FitDeps;
  /** Sizes the container proposes. Tests change this to simulate a resize during an IPC. */
  proposal: { size: Size | null };
  /** xterm's current size. */
  term: { size: Size };
  ptyCalls: Size[];
  termCalls: Size[];
  errors: unknown[];
  /** How many resizePty calls are in flight right now, and the most there were at once. */
  inFlight: number;
  maxInFlight: number;
  /** Settles the next pending resizePty call. */
  settle: Array<(err?: unknown) => void>;
  connected: boolean;
}

function fake(initial: Size): Fake {
  const f: Fake = {
    deps: null as unknown as FitDeps,
    proposal: { size: initial },
    term: { size: { rows: 24, cols: 80 } },
    ptyCalls: [],
    termCalls: [],
    errors: [],
    inFlight: 0,
    maxInFlight: 0,
    settle: [],
    connected: true,
  };
  f.deps = {
    connected: () => f.connected,
    proposeSize: () => f.proposal.size,
    currentSize: () => f.term.size,
    resizePty: (size) => {
      f.ptyCalls.push(size);
      f.inFlight += 1;
      f.maxInFlight = Math.max(f.maxInFlight, f.inFlight);
      return new Promise<void>((resolve, reject) => {
        f.settle.push((err) => {
          f.inFlight -= 1;
          if (err === undefined) {
            resolve();
          } else {
            reject(err);
          }
        });
      });
    },
    resizeTerminal: (size) => {
      f.termCalls.push(size);
      f.term.size = size;
    },
    onError: (e) => f.errors.push(e),
  };
  return f;
}

/** Lets pending microtasks run, so queued fits get to their next await. */
async function tick(): Promise<void> {
  for (let i = 0; i < 5; i++) {
    await Promise.resolve();
  }
}

const SIZE_A: Size = { rows: 30, cols: 100 };
const SIZE_B: Size = { rows: 40, cols: 120 };

describe('createFitter', () => {
  it('resizes the PTY, then xterm, when the container differs from xterm', async () => {
    const f = fake(SIZE_A);
    const fit = createFitter(f.deps);
    const done = fit();
    await tick();
    f.settle[0]();
    await done;
    expect(f.ptyCalls).toEqual([SIZE_A]);
    expect(f.termCalls).toEqual([SIZE_A]);
    expect(f.term.size).toEqual(SIZE_A);
  });

  it('does nothing when xterm already matches the container', async () => {
    const f = fake({ rows: 24, cols: 80 });
    await createFitter(f.deps)();
    expect(f.ptyCalls).toEqual([]);
    expect(f.termCalls).toEqual([]);
  });

  it('does nothing when the size cannot be proposed', async () => {
    const f = fake(SIZE_A);
    f.proposal.size = null;
    await createFitter(f.deps)();
    expect(f.ptyCalls).toEqual([]);
  });

  it('does nothing before the session is connected', async () => {
    const f = fake(SIZE_A);
    f.connected = false;
    await createFitter(f.deps)();
    expect(f.ptyCalls).toEqual([]);
    expect(f.termCalls).toEqual([]);
  });

  it('never has two resizes in flight: a second call waits for the first', async () => {
    const f = fake(SIZE_A);
    const fit = createFitter(f.deps);
    const first = fit();
    const second = fit();
    await tick();
    expect(f.ptyCalls).toEqual([SIZE_A]);
    f.settle[0]();
    await tick();
    // The first call's loop found nothing more to do, so the second call runs now, against the new xterm size.
    expect(f.ptyCalls).toEqual([SIZE_A]);
    await first;
    await second;
    expect(f.maxInFlight).toBe(1);
  });

  it('a call made during the IPC does not stop at a stale size: the container change is applied', async () => {
    // The original bug: xterm's size updates only after the IPC, so a second call during it compared against the old
    // size and returned. Here the container changes to B while A is still in flight, and xterm must end at B.
    const f = fake(SIZE_A);
    const fit = createFitter(f.deps);
    const first = fit();
    await tick();
    f.proposal.size = SIZE_B;
    const second = fit();
    f.settle[0]();
    await tick();
    f.settle[1]();
    await first;
    await second;
    expect(f.termCalls).toEqual([SIZE_A, SIZE_B]);
    expect(f.term.size).toEqual(SIZE_B);
  });

  it('re-proposes after each completed resize and runs again while the size still differs', async () => {
    const f = fake(SIZE_A);
    const done = createFitter(f.deps)();
    await tick();
    // The container changes while the A resize is in flight; the loop must pick it up after A completes.
    f.proposal.size = SIZE_B;
    f.settle[0]();
    await tick();
    f.settle[1]();
    await done;
    expect(f.ptyCalls).toEqual([SIZE_A, SIZE_B]);
    expect(f.term.size).toEqual(SIZE_B);
  });

  it('caps the passes at MAX_FIT_PASSES when the container keeps changing', async () => {
    const f = fake(SIZE_A);
    let n = 0;
    f.deps = {
      ...f.deps,
      proposeSize: () => ({ rows: 10 + n, cols: 10 + n }),
      resizePty: async (size) => {
        f.ptyCalls.push(size);
        n += 1;
      },
    };
    await createFitter(f.deps)();
    expect(f.ptyCalls.length).toBe(MAX_FIT_PASSES);
    expect(MAX_FIT_PASSES).toBe(5);
  });

  it('when the PTY resize fails, xterm is not resized, the error is reported, and the call ends', async () => {
    const f = fake(SIZE_A);
    const done = createFitter(f.deps)();
    await tick();
    const boom = new Error('pty gone');
    f.settle[0](boom);
    await done;
    expect(f.termCalls).toEqual([]);
    expect(f.term.size).toEqual({ rows: 24, cols: 80 });
    expect(f.errors).toEqual([boom]);
    expect(f.ptyCalls.length).toBe(1);
  });

  it('a later call retries after a failure', async () => {
    const f = fake(SIZE_A);
    const fit = createFitter(f.deps);
    const failed = fit();
    await tick();
    f.settle[0](new Error('transient'));
    await failed;
    const retried = fit();
    await tick();
    f.settle[1]();
    await retried;
    expect(f.ptyCalls).toEqual([SIZE_A, SIZE_A]);
    expect(f.term.size).toEqual(SIZE_A);
  });

  it('a call queued behind a failed one still runs', async () => {
    const f = fake(SIZE_A);
    const fit = createFitter(f.deps);
    const failed = fit();
    const queued = fit();
    await tick();
    f.settle[0](new Error('transient'));
    await failed;
    await tick();
    f.settle[1]();
    await queued;
    expect(f.term.size).toEqual(SIZE_A);
  });
});

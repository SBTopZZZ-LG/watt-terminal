import { afterEach, describe, expect, it, vi } from 'vitest';
import { Coalescer, COALESCE_BYTES, COALESCE_MS } from './coalescer';

function chunk(n: number, fill: number): ArrayBuffer {
  return new Uint8Array(n).fill(fill).buffer;
}

function concat(parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let off = 0;
  for (const p of parts) {
    out.set(p, off);
    off += p.length;
  }
  return out;
}

describe('Coalescer', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('flushes as soon as 64 KiB is pending', () => {
    vi.useFakeTimers();
    const batches: Uint8Array[] = [];
    const c = new Coalescer({ onBatch: (b) => batches.push(b) });

    c.push(chunk(COALESCE_BYTES - 1, 1));
    expect(batches).toHaveLength(0);
    c.push(chunk(1, 2));

    expect(batches).toHaveLength(1);
    expect(batches[0].length).toBe(COALESCE_BYTES);
    expect(batches[0][COALESCE_BYTES - 1]).toBe(2);
  });

  it('flushes 16 ms after the first pending chunk when the size is not reached', () => {
    vi.useFakeTimers();
    const batches: Uint8Array[] = [];
    const c = new Coalescer({ onBatch: (b) => batches.push(b) });

    c.push(chunk(10, 7));
    vi.advanceTimersByTime(COALESCE_MS - 1);
    expect(batches).toHaveLength(0);
    vi.advanceTimersByTime(1);

    expect(batches).toHaveLength(1);
    expect(batches[0].length).toBe(10);
  });

  it('the timer is armed by the first chunk, so later chunks do not push the flush back', () => {
    vi.useFakeTimers();
    const batches: Uint8Array[] = [];
    const c = new Coalescer({ onBatch: (b) => batches.push(b) });

    c.push(chunk(4, 1));
    vi.advanceTimersByTime(10);
    c.push(chunk(4, 2));
    vi.advanceTimersByTime(COALESCE_MS - 10);

    expect(batches).toHaveLength(1);
    expect(batches[0].length).toBe(8);
  });

  it('dispose flushes pending bytes first, byte-exact, and stops the timer', () => {
    vi.useFakeTimers();
    const batches: Uint8Array[] = [];
    const c = new Coalescer({ onBatch: (b) => batches.push(b) });
    const parts = [new Uint8Array(5).fill(0xaa), new Uint8Array(1000).fill(0x00), new Uint8Array(7).fill(0xff)];

    for (const p of parts) {
      c.push(p.buffer);
    }
    c.dispose();

    expect(batches).toHaveLength(1);
    expect(batches[0].length).toBe(1012);
    expect(batches[0]).toEqual(concat(parts));
    vi.advanceTimersByTime(COALESCE_MS * 10);
    expect(batches).toHaveLength(1);
  });

  it('batches add up to exactly the bytes pushed, across size flushes, time flushes and dispose', () => {
    vi.useFakeTimers();
    let acked = 0;
    let pushed = 0;
    const c = new Coalescer({ onBatch: (b) => (acked += b.length) });
    const sizes = [100, 65000, 5, 70000, 16384, 1, 40000];

    sizes.forEach((n, i) => {
      c.push(chunk(n, i));
      pushed += n;
      if (i === 3) {
        vi.advanceTimersByTime(COALESCE_MS);
      }
    });
    c.dispose();

    expect(acked).toBe(pushed);
  });

  it('bytes pushed after dispose are forwarded at once, not dropped', () => {
    const batches: Uint8Array[] = [];
    const c = new Coalescer({ onBatch: (b) => batches.push(b) });
    c.dispose();

    c.push(chunk(3, 9));

    expect(batches).toHaveLength(1);
    expect(batches[0]).toEqual(new Uint8Array([9, 9, 9]));
  });

  it('discard drops pending bytes without emitting them, and the timer does not fire later', () => {
    vi.useFakeTimers();
    const batches: Uint8Array[] = [];
    const c = new Coalescer({ onBatch: (b) => batches.push(b) });
    c.push(chunk(10, 1));
    c.discard();
    vi.advanceTimersByTime(COALESCE_MS * 10);
    expect(batches).toHaveLength(0);

    c.push(chunk(3, 2));
    c.flush();
    expect(batches).toHaveLength(1);
    expect(batches[0]).toEqual(new Uint8Array([2, 2, 2]));
  });

  it('ignores empty chunks', () => {
    const batches: Uint8Array[] = [];
    const c = new Coalescer({ onBatch: (b) => batches.push(b) });
    c.push(new ArrayBuffer(0));
    c.flush();
    expect(batches).toHaveLength(0);
  });
});

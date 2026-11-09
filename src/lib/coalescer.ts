// Joins small PTY chunks into batches before they reach xterm. Per-chunk writes and acks collapse throughput
// (see plan.md, POC 3b), so the batch is flushed at 64 KiB or 16 ms, whichever comes first.

export const COALESCE_BYTES = 64 * 1024;
export const COALESCE_MS = 16;

export interface CoalescerOptions {
  /**
   * Receives each batch, in order. The caller writes it to the terminal and acks `batch.length` bytes from
   * the write callback. Acks must match bytes exactly, or Rust's flow control drifts.
   */
  onBatch: (batch: Uint8Array) => void;
  maxBytes?: number;
  maxDelayMs?: number;
}

export class Coalescer {
  private parts: Uint8Array[] = [];
  private size = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private disposed = false;
  private readonly maxBytes: number;
  private readonly maxDelayMs: number;

  constructor(private readonly opts: CoalescerOptions) {
    this.maxBytes = opts.maxBytes ?? COALESCE_BYTES;
    this.maxDelayMs = opts.maxDelayMs ?? COALESCE_MS;
  }

  push(chunk: ArrayBuffer | Uint8Array): void {
    const bytes = chunk instanceof Uint8Array ? chunk : new Uint8Array(chunk);
    if (bytes.length === 0) {
      return;
    }
    // After dispose nothing waits for a timer, so forward the bytes at once rather than drop them unacked.
    if (this.disposed) {
      this.opts.onBatch(bytes);
      return;
    }
    this.parts.push(bytes);
    this.size += bytes.length;
    if (this.size >= this.maxBytes) {
      this.flush();
    } else if (this.timer === null) {
      this.timer = setTimeout(() => {
        this.timer = null;
        this.flush();
      }, this.maxDelayMs);
    }
  }

  /** Emits the pending bytes now, if any. */
  flush(): void {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    if (this.size === 0) {
      return;
    }
    const batch = new Uint8Array(this.size);
    let offset = 0;
    for (const part of this.parts) {
      batch.set(part, offset);
      offset += part.length;
    }
    this.parts = [];
    this.size = 0;
    this.opts.onBatch(batch);
  }

  /**
   * Drops pending bytes without emitting them. Used when the session they came from is gone. They were never
   * handed to the caller, so they are never acked.
   */
  discard(): void {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    this.parts = [];
    this.size = 0;
  }

  /** Flushes pending bytes first, so their acks still happen, then stops the timer. */
  dispose(): void {
    this.flush();
    this.disposed = true;
  }
}

// The one connection to the Rust session: its two channels, the listeners for events, and the nonce.
//
// connect() is memoised. React StrictMode mounts effects twice, and a second start_session would only swap
// channels, so the first connection is kept for the page's lifetime. Nothing here kills the session: the Rust side
// does that on exit.

import { Channel } from '@tauri-apps/api/core';
import { api, errorText } from './api';
import type { StartResult, WireEvent } from './types';

export type Incoming = { kind: 'event'; event: WireEvent } | { kind: 'error'; message: string };
export type Listener = (msg: Incoming) => void;

const listeners = new Set<Listener>();
let connection: Promise<StartResult> | null = null;
let nonce: string | null = null;
let byteSink: ((buf: ArrayBuffer) => void) | null = null;
/** Bumped on every connect and restart. Callbacks from an older channel check it and drop their input. */
let generation = 0;

/** Adds a listener. Events that arrive while no listener is subscribed are not replayed. */
export function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function emit(msg: Incoming): void {
  for (const listener of [...listeners]) {
    listener(msg);
  }
}

/** Shows an error to the user (through the listeners). */
export function reportError(e: unknown): void {
  emit({ kind: 'error', message: errorText(e) });
}

/** The integration nonce, or null until the session has started. Used only to filter OSC payloads. */
export function getNonce(): string | null {
  return nonce;
}

/**
 * Starts the session on first call and returns the same promise after that. `onBytes` receives every raw PTY
 * chunk, in order, and must ack what it renders. It is fixed by the first call.
 */
export function connect(rows: number, cols: number, onBytes: (buf: ArrayBuffer) => void): Promise<StartResult> {
  if (connection === null) {
    byteSink = onBytes;
    connection = open(rows, cols, (r, c, data, events) => api.startSession(r, c, data, events));
  }
  return connection;
}

/**
 * Replaces an exited session with a new shell (new nonce) and attaches the channels to it. Call only after
 * connect(). The caller clears the terminal first (resetForNewSession): bytes from the old shell must not reach it.
 */
export function restart(rows: number, cols: number): Promise<StartResult> {
  connection = open(rows, cols, (r, c, data, events) => api.restartSession(r, c, data, events));
  return connection;
}

function open(
  rows: number,
  cols: number,
  call: (rows: number, cols: number, data: Channel<ArrayBuffer>, events: Channel<WireEvent>) => Promise<StartResult>,
): Promise<StartResult> {
  generation += 1;
  const gen = generation;
  const data = new Channel<ArrayBuffer>((buf) => {
    if (gen === generation) {
      byteSink?.(buf);
    }
  });
  const events = new Channel<WireEvent>((event) => {
    if (gen === generation) {
      emit({ kind: 'event', event });
    }
  });
  return call(rows, cols, data, events).then((result) => {
    if (gen === generation) {
      nonce = result.nonce;
    }
    return result;
  });
}

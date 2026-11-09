// Typed wrappers around the Tauri commands in src-tauri/src/lib.rs. Every call carries the session id.
// Tauri maps the Rust snake_case argument names to camelCase here (session_id -> sessionId).

import { Channel, invoke } from '@tauri-apps/api/core';
import type { StartResult, Status, WireEvent } from './types';

/** Slice 1 runs one session. Every call still passes its id, so more sessions later need no call-site change. */
export const SESSION_ID = 'main';

export function errorText(e: unknown): string {
  if (typeof e === 'string') {
    return e;
  }
  if (e instanceof Error) {
    return e.message;
  }
  return String(e);
}

export const api = {
  startSession(rows: number, cols: number, onData: Channel<ArrayBuffer>, onEvents: Channel<WireEvent>) {
    return invoke<StartResult>('start_session', { sessionId: SESSION_ID, rows, cols, onData, onEvents });
  },
  /** Starts a new shell if the session has exited (new nonce); otherwise attaches like startSession. */
  restartSession(rows: number, cols: number, onData: Channel<ArrayBuffer>, onEvents: Channel<WireEvent>) {
    return invoke<StartResult>('restart_session', { sessionId: SESSION_ID, rows, cols, onData, onEvents });
  },
  ack(bytes: number) {
    return invoke<void>('ack', { sessionId: SESSION_ID, bytes });
  },
  resize(rows: number, cols: number) {
    return invoke<void>('resize', { sessionId: SESSION_ID, rows, cols });
  },
  writeUser(text: string) {
    return invoke<void>('write_user', { sessionId: SESSION_ID, text });
  },
  submitInput(text: string) {
    return invoke<void>('submit_input', { sessionId: SESSION_ID, text });
  },
  agentMessage(text: string) {
    return invoke<void>('agent_message', { sessionId: SESSION_ID, text });
  },
  approveTool(callId: string, approve: boolean) {
    return invoke<void>('approve_tool', { sessionId: SESSION_ID, callId, approve });
  },
  setAutoApprove(on: boolean) {
    return invoke<void>('set_auto_approve', { sessionId: SESSION_ID, on });
  },
  getStatus() {
    return invoke<Status>('get_status', { sessionId: SESSION_ID });
  },
};

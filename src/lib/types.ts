// Wire types. These mirror the serde shapes in src-tauri/src/core (observer.rs, record.rs, agent.rs, lib.rs).
// Field names are snake_case because that is what Rust sends.

export type InputState =
  | 'booting'
  | 'prompt'
  | 'submitted'
  | 'running'
  | 'continuation'
  | 'alt'
  | 'raw'
  | 'exited';

export type IntegrationStatus = 'booting' | 'live' | 'raw';
export type ObserverStatus = 'ok' | 'desynced';

export interface Cwd {
  host: string;
  path: string;
}

export type Source =
  | { kind: 'user' }
  | { kind: 'agent'; call_id: string }
  | { kind: 'term_reply' }
  | { kind: 'unknown' };

export type RecordStatus = 'complete' | 'incomplete' | 'approximate';

export interface RecordSummary {
  id: number;
  command: string;
  source: Source;
  exit_code: number | null;
  status: RecordStatus;
  used_alt_screen: boolean;
  output_bytes: number;
}

export type SessionEvent =
  | { type: 'state'; state: InputState }
  | { type: 'integration'; status: IntegrationStatus }
  | { type: 'observer'; status: ObserverStatus }
  | { type: 'cwd'; cwd: Cwd | null }
  | { type: 'record_started'; id: number; command: string; source: Source }
  | { type: 'record_finished'; record: RecordSummary };

export type AgentEvent =
  | { type: 'text'; text: string }
  | { type: 'tool_call'; call_id: string; name: string; args: unknown }
  | { type: 'tool_result'; call_id: string; name: string; ok: boolean; result: unknown }
  | { type: 'approval_request'; call_id: string; text: string }
  /** The agent is executing an approved shell call (true) and has finished it (false). */
  | { type: 'agent_control'; active: boolean }
  | { type: 'done' };

export type WireEvent = SessionEvent | AgentEvent;

export interface StartResult {
  spawned: boolean;
  nonce: string;
}

export interface SessionStats {
  forged_events: number;
  overflow_events: number;
  dropped_bytes: number;
}

export interface Status {
  state: InputState;
  integration: IntegrationStatus;
  observer: ObserverStatus;
  cwd: Cwd | null;
  stats: SessionStats;
  auto_approve: boolean;
  agent_busy: boolean;
  agent_control: boolean;
}

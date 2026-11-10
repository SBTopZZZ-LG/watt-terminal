// The agent transcript: what the user said, what the agent said, its tool calls, and approval requests.
// Pure reducer over AgentEvents (from Rust) and local actions (sent messages, clicks, errors).

import type { AgentEvent, ToolDecision } from './types';
import { visibleEscapes } from './visibleEscapes';

export type ToolStatus = 'running' | 'ok' | 'failed';
export type ApprovalState = 'pending' | 'answering' | 'resolved';

export type TranscriptItem =
  | { id: number; kind: 'user'; clientId: number; text: string }
  /** `streaming` is true while text_delta events are still extending this message. */
  | { id: number; kind: 'agent'; text: string; streaming: boolean }
  | { id: number; kind: 'tool'; callId: string; name: string; args: unknown; status: ToolStatus; result: unknown; decision: ToolDecision | null }
  | { id: number; kind: 'approval'; callId: string; name: string; text: string; state: ApprovalState; approve: boolean | null }
  | { id: number; kind: 'notice'; text: string };

export interface Transcript {
  items: TranscriptItem[];
  /** Agent turns sent but not yet finished. Each accepted message ends with exactly one `done`. */
  inFlight: number;
  nextId: number;
  /** The model is working on a reply and has not produced output yet. */
  thinking: boolean;
}

export const EMPTY_TRANSCRIPT: Transcript = { items: [], inFlight: 0, nextId: 1, thinking: false };

/** Keeps the transcript bounded; the oldest items go first. */
export const MAX_ITEMS = 500;

export type TranscriptAction =
  | { type: 'agent_event'; event: AgentEvent }
  /** A message was handed to the agent. Counts as in flight until its `done`. */
  | { type: 'user_sent'; clientId: number; text: string }
  /** The agent refused the message (Rust error). Drops the user item and the in-flight count. */
  | { type: 'user_failed'; clientId: number; message: string }
  | { type: 'approval_answering'; callId: string; approve: boolean }
  | { type: 'approval_failed'; callId: string; message: string }
  | { type: 'notice'; text: string }
  /** Set from the status snapshot at startup, when a turn may already be running. */
  | { type: 'sync_busy'; busy: boolean }
  /** A new shell replaced the old one. Pending approvals and running tools can never finish, so they are closed. */
  | { type: 'session_reset' }
  /** The agent's conversation was cleared on the Rust side. Only sent after a successful reset, never while busy. */
  | { type: 'conversation_reset' };

export function transcriptReducer(t: Transcript, action: TranscriptAction): Transcript {
  switch (action.type) {
    case 'agent_event':
      return applyAgentEvent(t, action.event);
    case 'user_sent':
      return add({ ...t, inFlight: t.inFlight + 1 }, (id) => ({
        id,
        kind: 'user',
        clientId: action.clientId,
        text: action.text,
      }));
    case 'user_failed': {
      const items = t.items.filter((i) => !(i.kind === 'user' && i.clientId === action.clientId));
      return add({ ...t, items, inFlight: Math.max(0, t.inFlight - 1) }, (id) => ({
        id,
        kind: 'notice',
        text: action.message,
      }));
    }
    case 'approval_answering':
      return {
        ...t,
        items: t.items.map((i) =>
          i.kind === 'approval' && i.callId === action.callId && i.state === 'pending'
            ? { ...i, state: 'answering', approve: action.approve }
            : i,
        ),
      };
    case 'approval_failed': {
      const items = t.items.map((i) =>
        i.kind === 'approval' && i.callId === action.callId && i.state === 'answering'
          ? { ...i, state: 'pending' as const, approve: null }
          : i,
      );
      return add({ ...t, items }, (id) => ({ id, kind: 'notice', text: action.message }));
    }
    case 'notice':
      return add(t, (id) => ({ id, kind: 'notice', text: action.text }));
    case 'sync_busy':
      return { ...t, inFlight: action.busy ? Math.max(1, t.inFlight) : 0 };
    case 'session_reset': {
      const items = t.items.map((i): TranscriptItem => {
        if (i.kind === 'approval' && i.state !== 'resolved') {
          return { ...i, state: 'resolved' };
        }
        if (i.kind === 'tool' && i.status === 'running') {
          return { ...i, status: 'failed' };
        }
        return i;
      });
      return add({ ...t, items, inFlight: 0, thinking: false }, (id) => ({ id, kind: 'notice', text: 'New shell session started.' }));
    }
    case 'conversation_reset':
      // Keeps nextId, so ids stay unique for the React keys of the items that come after.
      return { ...t, items: [], inFlight: 0, thinking: false };
  }
}

function applyAgentEvent(t: Transcript, ev: AgentEvent): Transcript {
  switch (ev.type) {
    case 'text':
      return add(t, (id) => ({ id, kind: 'agent', text: ev.text, streaming: false }));
    case 'text_delta':
      return appendDelta(t, ev.text);
    case 'thinking':
      return { ...t, thinking: ev.active };
    case 'tool_call':
      return add(t, (id) => ({
        id,
        kind: 'tool',
        callId: ev.call_id,
        name: ev.name,
        args: ev.args,
        status: 'running',
        result: null,
        decision: null,
      }));
    case 'tool_result': {
      const hasTool = t.items.some((i) => i.kind === 'tool' && i.callId === ev.call_id);
      const items = t.items.map((i) => {
        if (i.kind === 'tool' && i.callId === ev.call_id) {
          return { ...i, status: ev.ok ? ('ok' as const) : ('failed' as const), result: ev.result, decision: ev.decision };
        }
        if (i.kind === 'approval' && i.callId === ev.call_id) {
          return { ...i, state: 'resolved' as const };
        }
        return i;
      });
      const next = { ...t, items };
      // A result without its call should not happen, but show it rather than drop it.
      return hasTool
        ? next
        : add(next, (id) => ({
            id,
            kind: 'tool',
            callId: ev.call_id,
            name: ev.name,
            args: null,
            status: ev.ok ? 'ok' : 'failed',
            result: ev.result,
            decision: ev.decision,
          }));
    }
    case 'approval_request':
      return add(t, (id) => ({
        id,
        kind: 'approval',
        callId: ev.call_id,
        name: ev.name,
        text: ev.text,
        state: 'pending',
        approve: null,
      }));
    case 'done':
      // The turn is over: the streamed message (if any) is closed, and so is the thinking state.
      return { ...t, items: closeStream(t.items), inFlight: Math.max(0, t.inFlight - 1), thinking: false };
    // Drives the terminal border (see agentControl.ts), not the transcript.
    case 'agent_control':
      return t;
  }
}

/** Extends the open streamed message, or starts one. The first delta also clears the thinking state. */
function appendDelta(t: Transcript, text: string): Transcript {
  // An empty delta carries nothing to show; it does not start a message.
  if (text === '') {
    return t;
  }
  const last = t.items[t.items.length - 1];
  if (last && last.kind === 'agent' && last.streaming) {
    const items = [...t.items.slice(0, -1), { ...last, text: last.text + text }];
    return { ...t, items, thinking: false };
  }
  return { ...add(t, (id) => ({ id, kind: 'agent', text, streaming: true })), thinking: false };
}

/** Any new item closes the streamed message before it, so the next delta starts a fresh one. */
function closeStream(items: TranscriptItem[]): TranscriptItem[] {
  const last = items[items.length - 1];
  if (last && last.kind === 'agent' && last.streaming) {
    return [...items.slice(0, -1), { ...last, streaming: false }];
  }
  return items;
}

function add(t: Transcript, make: (id: number) => TranscriptItem): Transcript {
  const item = make(t.nextId);
  const items = [...closeStream(t.items), item];
  // Trimming drops the oldest items only. The streamed message is always the last item, so it is never trimmed here.
  return { ...t, items: items.length > MAX_ITEMS ? items.slice(items.length - MAX_ITEMS) : items, nextId: t.nextId + 1 };
}

/** The call id of the oldest approval still waiting for an answer, or null. */
export function firstPendingApproval(t: Transcript): string | null {
  const found = t.items.find((i) => i.kind === 'approval' && i.state === 'pending');
  return found && found.kind === 'approval' ? found.callId : null;
}

export function isBusy(t: Transcript): boolean {
  return t.inFlight > 0;
}

/** The title of an approval card. The name comes from the model, so an unknown one is shown escaped. */
export function approvalTitle(name: string): string {
  switch (name) {
    case 'run_command':
      return 'The agent wants to run a command';
    case 'send_keys':
      return 'The agent wants to send keystrokes';
    default:
      return `The agent wants to use ${visibleEscapes(name)}`;
  }
}

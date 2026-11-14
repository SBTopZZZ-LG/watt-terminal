// The agent transcript: what the user said, what the agent said, its tool calls, and approval requests.
// Pure reducer over AgentEvents (from Rust) and local actions (sent messages, clicks, errors).

import type { AgentEvent, ApprovalReasonCode, ContextUsage, QuestionAnswerInput, QuestionResult, QuestionSpec, RecordSummary, TodoItem, ToolDecision } from './types';
import { APPROVAL_WINDOW_MS } from './approvalTime';
import { visibleEscapes } from './visibleEscapes';
import { TODO_TOOL } from './todoView';
import { ASK_USER_TOOL, closedResult, parseResult } from './questions';
import { openFramesBelow, openRecordIds, openedBy } from './interactive';

export type ToolStatus = 'running' | 'ok' | 'failed';
export type ApprovalState = 'pending' | 'answering' | 'resolved';
/**
 * A clarification question: `pending` waits for the user, `answering` sent the answers, `resolved` settled, `failed`
 * did not send (retryable), `closed` can no longer be answered (the shell restarted, the turn ended, or a saved call
 * had no result).
 */
export type QuestionState = 'pending' | 'answering' | 'resolved' | 'failed' | 'closed';

/** An attachment on a message the user sent. `kind` is 'file' for a text document. `thumb` is a data URL, for an image. */
export interface UserAttachment {
  kind: 'image' | 'file';
  name: string;
  bytes: number;
  thumb?: string;
}

export type TranscriptItem =
  /** `attachments` is absent when the message had none. */
  | { id: number; kind: 'user'; clientId: number; text: string; attachments?: UserAttachment[] }
  /** `streaming` is true while text_delta events are still extending this message. */
  | { id: number; kind: 'agent'; text: string; streaming: boolean }
  /**
   * `reason` is why the call asked or ran without asking. Null for read tools and calls refused before the policy.
   * `startedAt` is when the call started (Unix ms), set for the calls of this session; absent for a reopened conversation.
   * `levelDepth` is the shell depth when the result that opened an interactive group arrived (see interactive.ts). Absent otherwise.
   */
  | { id: number; kind: 'tool'; callId: string; name: string; args: unknown; status: ToolStatus; result: unknown; decision: ToolDecision | null; reason: ApprovalReasonCode | null; startedAt?: number; levelDepth?: number }
  /**
   * `reason` is why the card asks. `location` is the nested shell the command is for (its label), or null on the local shell.
   * `display` is the decoded input of a send_keys card (null for the others, and for a card from an older backend).
   * `expiresAt` is when the countdown runs out (Unix ms); absent for a card from an older session.
   */
  | { id: number; kind: 'approval'; callId: string; name: string; text: string; display: string | null; state: ApprovalState; approve: boolean | null; reason: ApprovalReasonCode | null; location: string | null; expiresAt?: number }
  /**
   * The agent's clarification question (`ask_user`). `questions` are the backend's, with its ids. `answers` are the
   * ones sent (or being sent); `result` is the settled outcome once the tool result arrives. `error` is why a send
   * failed (retryable). `note` is why a closed question can no longer be answered.
   */
  | { id: number; kind: 'question'; callId: string; questions: QuestionSpec[]; state: QuestionState; answers: QuestionAnswerInput[] | null; result: QuestionResult | null; error: string | null; note: string | null }
  /**
   * The command an interactive group opened on has ended (see interactive.ts). Not shown: it closes the group. `integrated` is set
   * when a shell level was pushed above the command (levels_changed), so the group ends while the command may still run.
   */
  | { id: number; kind: 'record_end'; recordId: number; exit: number | null; integrated?: boolean }
  /** `muted` is a quiet note (the trim note), drawn in the muted colour rather than the warning one. */
  | { id: number; kind: 'notice'; text: string; muted?: boolean };

/** A transcript item before the reducer gives it an id. `Omit` over the union would lose the variants, so it is distributed. */
export type NewItem = TranscriptItem extends infer T ? (T extends unknown ? Omit<T, 'id'> : never) : never;

export interface Transcript {
  items: TranscriptItem[];
  /** Agent turns sent but not yet finished. Each accepted message ends with exactly one `done`. */
  inFlight: number;
  nextId: number;
  /** The model is working on a reply and has not produced output yet. */
  thinking: boolean;
  /** The conversation's todo list, shown by the todo panel. Replaced as a whole. */
  todos: TodoItem[];
  /**
   * Agent commands that finished while no interactive group was open for them, newest last. The run_command result that
   * opens a group can arrive after the command has finished, so the group is closed at once when it does (see transcript.ts).
   */
  finished: FinishedRecord[];
  /** The shell depth now: the levels count, the local one included. 1 means no nested shell (see levels_changed). */
  depth: number;
  /** The context usage of the latest model request (the context meter). Null for a new conversation, and until a request runs. */
  context: ContextUsage | null;
  /** A trim note was added in the current turn. Cleared by user_sent, so each turn shows at most one. */
  trimNoted: boolean;
}

export interface FinishedRecord {
  recordId: number;
  exit: number | null;
}

export const EMPTY_TRANSCRIPT: Transcript = {
  items: [],
  inFlight: 0,
  nextId: 1,
  thinking: false,
  todos: [],
  finished: [],
  depth: 1,
  context: null,
  trimNoted: false,
};

/** The note shown once in a turn when older content was left out of a request. */
export const TRIM_NOTE = 'Older messages or tool output were left out of the request to fit the model\'s context window.';

/** Keeps the transcript bounded; the oldest items go first. */
export const MAX_ITEMS = 500;

/** How many finished agent commands are remembered for a group that opens late. */
const MAX_FINISHED = 16;

export type TranscriptAction =
  /** `at` is when the event arrived (Unix ms). Only a tool call's start time uses it, so it may be left out in tests. */
  | { type: 'agent_event'; event: AgentEvent; at?: number }
  /** A command the shell ran has finished. Closes the interactive group it opened on, if one is open (see interactive.ts). */
  | { type: 'record_finished'; record: RecordSummary }
  /** A message was handed to the agent. Counts as in flight until its `done`. */
  | { type: 'user_sent'; clientId: number; text: string; attachments?: UserAttachment[] }
  /** The agent refused the message (Rust error). Drops the user item and the in-flight count. */
  | { type: 'user_failed'; clientId: number; message: string }
  | { type: 'approval_answering'; callId: string; approve: boolean }
  | { type: 'approval_failed'; callId: string; message: string }
  /** The user's answers to a question were sent (or are being sent). The card shows the wait until its result arrives. */
  | { type: 'question_answering'; callId: string; answers: QuestionAnswerInput[] }
  /** Sending the answers failed: the card is back to pending, with the reason shown. */
  | { type: 'question_failed'; callId: string; message: string }
  | { type: 'notice'; text: string; muted?: boolean }
  /** Set from the status snapshot at startup, when a turn may already be running. */
  | { type: 'sync_busy'; busy: boolean }
  /** A new shell replaced the old one. Pending approvals and running tools can never finish, so they are closed. */
  | { type: 'session_reset' }
  /**
   * The shell levels changed (the levels event, or a status read). `depth` is the count now, the local one included. A level
   * pushed above an open group's command ends that group as integrated. A shallower depth ends nothing.
   */
  | { type: 'levels_changed'; depth: number }
  /** The agent's conversation was cleared on the Rust side. Only sent after a successful reset, never while busy. Clears the todos too. */
  | { type: 'conversation_reset' }
  /**
   * A saved conversation was opened as the live one. Replaces the items. Ids come from nextId; the newest MAX_ITEMS are kept.
   * Never sent while busy (the backend refuses to load then). `todos` is the loaded conversation's list; absent means none.
   * `context` is the usage the conversation was saved with (the meter's value on open); absent or null means none.
   */
  | { type: 'conversation_loaded'; items: NewItem[]; todos?: TodoItem[]; context?: ContextUsage | null };

export function transcriptReducer(t: Transcript, action: TranscriptAction): Transcript {
  switch (action.type) {
    case 'agent_event':
      return applyAgentEvent(t, action.event, action.at);
    case 'record_finished': {
      const { id, exit_code, source } = action.record;
      if (openRecordIds(t.items).has(id)) {
        return add(t, (nid) => ({ id: nid, kind: 'record_end', recordId: id, exit: exit_code }));
      }
      // No group to close yet. An agent command is remembered, in case its run_command result (which opens the group) comes later.
      if (source.kind !== 'agent') {
        return t;
      }
      const finished = [...t.finished.filter((f) => f.recordId !== id), { recordId: id, exit: exit_code }];
      return { ...t, finished: finished.slice(Math.max(0, finished.length - MAX_FINISHED)) };
    }
    case 'user_sent':
      return add({ ...t, inFlight: t.inFlight + 1, trimNoted: false }, (id) => ({
        id,
        kind: 'user',
        clientId: action.clientId,
        text: action.text,
        ...(action.attachments && action.attachments.length > 0 ? { attachments: action.attachments } : {}),
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
    case 'question_answering':
      return {
        ...t,
        items: t.items.map((i) =>
          i.kind === 'question' && i.callId === action.callId && (i.state === 'pending' || i.state === 'failed')
            ? { ...i, state: 'answering', answers: action.answers, error: null }
            : i,
        ),
      };
    case 'question_failed':
      // The send failed: the card keeps the error and the answers, so the user can retry them.
      return {
        ...t,
        items: t.items.map((i) =>
          i.kind === 'question' && i.callId === action.callId && i.state === 'answering'
            ? { ...i, state: 'failed' as const, error: action.message }
            : i,
        ),
      };
    case 'notice':
      return add(t, (id) => ({ id, kind: 'notice', text: action.text, ...(action.muted ? { muted: true } : {}) }));
    case 'sync_busy':
      return { ...t, inFlight: action.busy ? Math.max(1, t.inFlight) : 0 };
    case 'session_reset': {
      const items = t.items.map((i): TranscriptItem => {
        if (i.kind === 'approval' && i.state !== 'resolved') {
          return { ...i, state: 'resolved' };
        }
        // A question still waiting can never be answered now: the turn is gone with the old shell. A failed send
        // keeps its error and stays retryable.
        if (i.kind === 'question' && (i.state === 'pending' || i.state === 'answering')) {
          return { ...i, state: 'closed', result: closedResult(i.questions), note: 'The shell restarted before this question was answered.' };
        }
        if (i.kind === 'tool' && i.status === 'running') {
          return { ...i, status: 'failed' };
        }
        return i;
      });
      // The commands the old shell was running can never finish: their interactive groups end here, with no exit code.
      let next: Transcript = { ...t, items, inFlight: 0, thinking: false, finished: [] };
      for (const recordId of openRecordIds(items)) {
        next = add(next, (id) => ({ id, kind: 'record_end', recordId, exit: null }));
      }
      return add(next, (id) => ({ id, kind: 'notice', text: 'New shell session started.' }));
    }
    case 'levels_changed':
      return levelsChanged(t, action.depth);
    case 'conversation_reset':
      // Keeps nextId, so ids stay unique for the React keys of the items that come after.
      return { ...t, items: [], inFlight: 0, thinking: false, todos: [], context: null, trimNoted: false };
    case 'conversation_loaded': {
      // Every id is used up, even for the items the cap drops, so ids keep increasing.
      const made = action.items.map((item, i) => keyed(item, t.nextId + i));
      const items = made.length > MAX_ITEMS ? made.slice(made.length - MAX_ITEMS) : made;
      return {
        ...t,
        items,
        nextId: t.nextId + action.items.length,
        inFlight: 0,
        thinking: false,
        todos: action.todos ?? [],
        context: action.context ?? null,
        trimNoted: false,
      };
    }
  }
}

/** A new item with its id. A loaded message is never still streaming. */
function keyed(item: NewItem, id: number): TranscriptItem {
  return (item.kind === 'agent' ? { ...item, id, streaming: false } : { ...item, id }) as TranscriptItem;
}

/** Resolves a question card with its tool result. An invalid call has no card (nothing asked), so it is ignored. */
function resolveQuestion(t: Transcript, callId: string, ok: boolean, result: unknown): Transcript {
  if (!t.items.some((i) => i.kind === 'question' && i.callId === callId)) {
    return t;
  }
  const parsed = ok ? parseResult(result) : null;
  const error = ok ? null : resultError(result);
  return {
    ...t,
    items: t.items.map((i) => {
      if (i.kind !== 'question' || i.callId !== callId) {
        return i;
      }
  if (parsed !== null) {
      return { ...i, state: 'resolved' as const, result: parsed, error: null, note: null };
    }
    return { ...i, state: 'failed' as const, error: error ?? 'The answer could not be read.', note: null };
    }),
  };
}

/** The error text of a failed tool result (`{error: string}`), for the question card. Null when it has none. */
function resultError(result: unknown): string | null {
  if (typeof result === 'object' && result !== null && !Array.isArray(result)) {
    const error = (result as Record<string, unknown>).error;
    return typeof error === 'string' && error !== '' ? error : null;
  }
  return null;
}

function applyAgentEvent(t: Transcript, ev: AgentEvent, at: number | undefined): Transcript {
  switch (ev.type) {
    case 'text':
      return add(t, (id) => ({ id, kind: 'agent', text: ev.text, streaming: false }));
    case 'text_delta':
      return appendDelta(t, ev.text);
    case 'thinking':
      return { ...t, thinking: ev.active };
    case 'tool_call':
      // The todo tool's rows are not shown: the panel is the view of the list, so the call is not an item (or in flight).
      // The question tool's rows are not shown either: the card is the view of the question.
      if (ev.name === TODO_TOOL || ev.name === ASK_USER_TOOL) {
        return t;
      }
      return add(t, (id) => ({
        id,
        kind: 'tool',
        callId: ev.call_id,
        name: ev.name,
        args: ev.args,
        status: 'running',
        result: null,
        decision: null,
        reason: null,
        startedAt: at,
      }));
    case 'tool_result': {
      // A question's result resolves its card, and never adds a tool row (see tool_call).
      if (ev.name === ASK_USER_TOOL) {
        return resolveQuestion(t, ev.call_id, ev.ok, ev.result);
      }
      if (ev.name === TODO_TOOL) {
        return t;
      }
      const reason = ev.reason ?? null;
      const hasTool = t.items.some((i) => i.kind === 'tool' && i.callId === ev.call_id);
      const items = t.items.map((i) => {
        if (i.kind === 'tool' && i.callId === ev.call_id) {
          return withLevelDepth({ ...i, status: ev.ok ? ('ok' as const) : ('failed' as const), result: ev.result, decision: ev.decision, reason }, t.depth);
        }
        if (i.kind === 'approval' && i.callId === ev.call_id) {
          return { ...i, state: 'resolved' as const };
        }
        return i;
      });
      const next = { ...t, items };
      // A result without its call should not happen, but show it rather than drop it.
      if (hasTool) {
        return closeIfFinished(next, ev.call_id);
      }
      return add(next, (id) =>
        withLevelDepth(
          {
            id,
            kind: 'tool',
            callId: ev.call_id,
            name: ev.name,
            args: null,
            status: ev.ok ? 'ok' : 'failed',
            result: ev.result,
            decision: ev.decision,
            reason,
          },
          t.depth,
        ),
      );
    }
    case 'approval_request':
      return add(t, (id) => ({
        id,
        kind: 'approval',
        callId: ev.call_id,
        name: ev.name,
        text: ev.text,
        display: ev.display ?? null,
        state: 'pending',
        approve: null,
        reason: ev.reason ?? null,
        location: ev.location ?? null,
        expiresAt: (at ?? Date.now()) + APPROVAL_WINDOW_MS,
      }));
    case 'question_request':
      return add(t, (id) => ({
        id,
        kind: 'question',
        callId: ev.call_id,
        questions: ev.questions,
        state: 'pending',
        answers: null,
        result: null,
        error: null,
        note: null,
      }));
    case 'done':
      // The turn is over: the streamed message (if any) is closed, and so is the thinking state. A question still
      // waiting can never be answered now, so it closes (defensive: settled questions already resolved, and a failed
      // send stays retryable).
      return {
        ...t,
        items: closeStream(t.items).map((i): TranscriptItem =>
          i.kind === 'question' && i.state === 'pending'
            ? { ...i, state: 'closed', result: closedResult(i.questions), note: 'The turn ended before this question was answered.' }
            : i,
        ),
        inFlight: Math.max(0, t.inFlight - 1),
        thinking: false,
      };
    // Drives the terminal border (see agentControl.ts), not the transcript.
    case 'agent_control':
      return t;
    // The integration dialog and strip chip follow these (see integrate.ts). The row is the tool call's own (tool_result).
    case 'integration_request':
      return t;
    case 'todo_update':
      return { ...t, todos: ev.items };
    case 'notice':
      return add(t, (id) => ({ id, kind: 'notice', text: ev.text, muted: true }));
    case 'context_usage':
      return contextUsage(t, ev);
  }
}

/**
 * The context meter's value is the event's usage. The first event of a turn that left older content out adds the trim note,
 * once: the second event of the same request, and later requests of the turn, do not add another.
 */
function contextUsage(t: Transcript, ev: Extract<AgentEvent, { type: 'context_usage' }>): Transcript {
  const next: Transcript = {
    ...t,
    context: {
      limit: ev.limit,
      used: ev.used,
      reserved_output: ev.reserved_output,
      trimmed: ev.trimmed,
      calibrated: ev.calibrated,
      categories: ev.categories,
    },
  };
  if (!ev.trimmed || t.trimNoted) {
    return next;
  }
  return { ...add(next, (id) => ({ id, kind: 'notice', text: TRIM_NOTE, muted: true })), trimNoted: true };
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

/** A tool row whose result opens an interactive group records the shell depth it opened at. Other rows are returned as they are. */
function withLevelDepth(item: TranscriptItem, depth: number): TranscriptItem {
  if (item.kind !== 'tool' || openedBy(item) === null) {
    return item;
  }
  return { ...item, levelDepth: depth };
}

/**
 * The shell levels changed to `depth`. Deeper: each open group whose command was opened at a shallower depth ends as
 * integrated, since the agent's commands from now on run in the new level, not in the shell the group is for. Shallower
 * (a level popped): nothing ends, because the command the group is for may still run. Only the depth moves then.
 */
function levelsChanged(t: Transcript, depth: number): Transcript {
  if (depth <= t.depth) {
    return { ...t, depth };
  }
  let next: Transcript = { ...t, depth };
  for (const frame of openFramesBelow(t.items, depth)) {
    next = add(next, (id) => ({ id, kind: 'record_end', recordId: frame.recordId, exit: null, integrated: true }));
  }
  return next;
}

/**
 * The result of a run_command call that opened an interactive group, for a command that had already finished (see
 * `finished`). Its end goes right after the row, so the group opens and closes at once, with the command's exit code.
 */
function closeIfFinished(t: Transcript, callId: string): Transcript {
  const at = t.items.findIndex((i) => i.kind === 'tool' && i.callId === callId);
  if (at < 0) {
    return t;
  }
  const opener = openedBy(t.items[at]);
  const done = opener === null ? undefined : t.finished.find((f) => f.recordId === opener.recordId);
  if (done === undefined) {
    return t;
  }
  const marker: TranscriptItem = { id: t.nextId, kind: 'record_end', recordId: done.recordId, exit: done.exit };
  const items = [...t.items.slice(0, at + 1), marker, ...t.items.slice(at + 1)];
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
      return 'The agent wants to type into a running command';
    default:
      return `The agent wants to use ${visibleEscapes(name)}`;
  }
}

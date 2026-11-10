// The conversation history as the UI reads it: a saved conversation's messages turned into transcript items, and the
// history list's grouping, filtering and time labels. Pure, so the rules are tested without the DOM. Times are Unix
// seconds. Dates are local: "today" and the day boundaries are the machine's time zone.
//
// What a reopened conversation shows: user text, assistant text (markdown), and tool rows built from the assistant's
// tool calls (name and arguments) joined with their tool messages. Not shown, because they are not stored: approval
// cards, decision labels, UI notices, and raw tool results (only the projected text is kept).

import type { ChatMessage, ConversationMeta } from './types';
import type { NewItem, ToolStatus } from './transcript';
import { TODO_TOOL } from './todoView';

/**
 * The start of a denied call's error text. Mirrors DENIED in src-tauri/src/core/agent.rs. A stored tool message carries the
 * projected text "error: not_approved: ...", so the "error:" check is the one that matches a denial in practice. This check
 * also covers a tool message that carries the bare text.
 */
export const DENIED_PREFIX = 'not_approved';

/** The start of the synthetic close text the backend stores for a call with no result. See isFailure. */
export const SKIPPED_PREFIX = 'skipped:';

/** The result shown for a tool call with no tool message after it. The backend closes such calls, so this is a fallback. */
export const NO_RESULT_TEXT = 'no result was recorded';

const MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];
const MONTHS_SHORT = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

const DAY_S = 86_400;

/** The label for a time that cannot be shown (not a finite number, or out of Date's range). Never "NaN" on screen. */
export const UNKNOWN_DATE = 'Unknown date';

/** The local Date for a Unix-seconds time, or null when it is not a usable time. */
function dateOf(ts: number): Date | null {
  const d = new Date(ts * 1000);
  return Number.isFinite(d.getTime()) ? d : null;
}

/** Control characters become spaces. Bidi overrides and marks and the other invisible format characters are removed. ZWJ and ZWNJ (U+200D, U+200C) stay: emoji and some scripts need them. */
const CONTROL_CHARS = /\p{Cc}/gu;
const INVISIBLE_FORMAT = /[\u200B\u200E\u200F\u202A-\u202E\u2060\u2066-\u2069\uFEFF]/g;

/** Text as a History row or a title attribute shows it: stored titles, workspace labels and paths can hold these characters. */
export function safeText(text: string): string {
  return text.replace(CONTROL_CHARS, ' ').replace(INVISIBLE_FORMAT, '');
}

/** The text of a message's content. Strings pass through; null and missing give ''; anything else is shown as JSON. */
function textOf(content: unknown): string {
  if (typeof content === 'string') {
    return content;
  }
  if (content === null || content === undefined) {
    return '';
  }
  return JSON.stringify(content) ?? '';
}

/** Arguments are a JSON string in the chat format. Parsed when they are valid JSON, otherwise kept as the raw string. */
function parseArgs(raw: unknown): unknown {
  if (typeof raw !== 'string') {
    return raw ?? null;
  }
  try {
    return JSON.parse(raw) as unknown;
  } catch {
    return raw;
  }
}

/**
 * A tool result is a failure when it starts with `error:`, the DENIED prefix, or `skipped:`. The backend writes `skipped:` as the
 * synthetic close of a call that never got a result (src-tauri/src/core/chat/history.rs, e.g. "skipped: turn ended").
 */
function isFailure(content: string): boolean {
  return content.startsWith('error:') || content.startsWith(DENIED_PREFIX) || content.startsWith(SKIPPED_PREFIX);
}

interface ToolCall {
  id: string;
  name: string;
  args: unknown;
}

/** The well-formed tool calls of an assistant message. A call without a string id cannot be joined, so it is skipped. */
function callsOf(toolCalls: unknown): ToolCall[] {
  if (!Array.isArray(toolCalls)) {
    return [];
  }
  const calls: ToolCall[] = [];
  for (const raw of toolCalls) {
    if (typeof raw !== 'object' || raw === null) {
      continue;
    }
    const call = raw as Record<string, unknown>;
    const fn = typeof call.function === 'object' && call.function !== null ? (call.function as Record<string, unknown>) : {};
    if (typeof call.id !== 'string') {
      continue;
    }
    calls.push({
      id: call.id,
      name: typeof fn.name === 'string' ? fn.name : 'unknown',
      args: parseArgs(fn.arguments),
    });
  }
  return calls;
}

/**
 * The transcript items for a saved conversation's messages, in order. A user message is a user item. An assistant message
 * gives an agent item (when it has text), then one tool item per tool call. Each tool item takes the content of the tool
 * message with the same tool_call_id: failed when that content starts with `error:`, the DENIED prefix or `skipped:`, ok otherwise.
 * A call with no tool message is failed, with NO_RESULT_TEXT. Tool messages that answer no call are dropped. Empty
 * assistant text is skipped. A todo_write call gets no item (nor its tool message): the todo panel shows the list instead.
 * Ids are not set here; the transcript reducer gives them (conversation_loaded).
 */
export function itemsFromMessages(messages: ChatMessage[]): NewItem[] {
  const results = new Map<string, string>();
  for (const m of messages) {
    if (m.role === 'tool' && typeof m.tool_call_id === 'string') {
      results.set(m.tool_call_id, textOf(m.content));
    }
  }
  const items: NewItem[] = [];
  for (const m of messages) {
    if (m.role === 'user') {
      const text = textOf(m.content);
      if (text !== '') {
        items.push({ kind: 'user', clientId: 0, text });
      }
    } else if (m.role === 'assistant') {
      const text = textOf(m.content);
      if (text.trim() !== '') {
        items.push({ kind: 'agent', text, streaming: false });
      }
      for (const call of callsOf(m.tool_calls)) {
        if (call.name === TODO_TOOL) {
          continue;
        }
        const content = results.get(call.id);
        const status: ToolStatus = content !== undefined && !isFailure(content) ? 'ok' : 'failed';
        items.push({
          kind: 'tool',
          callId: call.id,
          name: call.name,
          args: call.args,
          status,
          result: content ?? NO_RESULT_TEXT,
          decision: null,
        });
      }
    }
  }
  return items;
}

/** Sorts a copy, most recently updated first. The sort is stable, so equal times keep the list's order. */
function newestFirst(metas: ConversationMeta[]): ConversationMeta[] {
  return [...metas].sort((a, b) => b.updated_at - a.updated_at);
}

/** The local midnight at the start of the day that `ms` falls in, moved by `days` calendar days. */
function dayStart(ms: number, days: number): number {
  const d = new Date(ms);
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() + days).getTime();
}

function pad2(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}

export interface DateGroup {
  key: string;
  label: string;
  items: ConversationMeta[];
}

/**
 * The history grouped by when each conversation was last updated, in local time: Today, Yesterday, Previous 7 days, then one
 * group per calendar month ("November 2026"). Previous 7 days is the seven days before yesterday: from local midnight eight days
 * back, up to yesterday's midnight. Anything older is by month. Groups with no conversations are left out. Newest first.
 * An update time after `now` (clock skew) counts as today. A time that cannot be read is in a last group, "Unknown date".
 */
export function groupByDate(metas: ConversationMeta[], now: number): DateGroup[] {
  const nowMs = now * 1000;
  const today = dayStart(nowMs, 0);
  const yesterday = dayStart(nowMs, -1);
  const weekStart = dayStart(nowMs, -8);
  const groups = new Map<string, DateGroup>();
  const unknown: ConversationMeta[] = [];
  for (const meta of newestFirst(metas)) {
    const ms = meta.updated_at * 1000;
    let key: string;
    let label: string;
    if (dateOf(meta.updated_at) === null) {
      // No usable time: kept apart, in a group of its own at the end.
      unknown.push(meta);
      continue;
    }
    if (ms >= today) {
      key = 'today';
      label = 'Today';
    } else if (ms >= yesterday) {
      key = 'yesterday';
      label = 'Yesterday';
    } else if (ms >= weekStart) {
      key = 'previous-7-days';
      label = 'Previous 7 days';
    } else {
      const d = new Date(ms);
      key = `month-${d.getFullYear()}-${pad2(d.getMonth() + 1)}`;
      label = `${MONTHS[d.getMonth()]} ${d.getFullYear()}`;
    }
    const group = groups.get(key);
    if (group) {
      group.items.push(meta);
    } else {
      groups.set(key, { key, label, items: [meta] });
    }
  }
  const out = [...groups.values()];
  if (unknown.length > 0) {
    out.push({ key: 'unknown', label: UNKNOWN_DATE, items: unknown });
  }
  return out;
}

export interface WorkspaceGroup {
  key: string;
  label: string;
  root: string;
  host: string;
  items: ConversationMeta[];
}

/**
 * The history grouped by workspace (host and root). Most recently active workspace first. The workspace of conversations
 * with no known directory (empty host and root) is last, whatever its activity.
 */
export function groupByWorkspace(metas: ConversationMeta[]): WorkspaceGroup[] {
  const groups = new Map<string, WorkspaceGroup>();
  for (const meta of newestFirst(metas)) {
    const { host, root, label } = meta.workspace;
    const key = JSON.stringify([host, root]);
    const group = groups.get(key);
    if (group) {
      group.items.push(meta);
    } else {
      groups.set(key, { key, label, root, host, items: [meta] });
    }
  }
  const all = [...groups.values()];
  const isUnknown = (g: WorkspaceGroup) => g.host === '' && g.root === '';
  return [...all.filter((g) => !isUnknown(g)), ...all.filter(isUnknown)];
}

/**
 * The conversations matching a filter. The query is split on whitespace, and every term must appear, ignoring case, in the
 * title or the workspace label or root. An empty query keeps everything. Order is kept.
 */
export function filterMetas(metas: ConversationMeta[], query: string): ConversationMeta[] {
  const terms = query.trim().toLowerCase().split(/\s+/).filter((t) => t !== '');
  if (terms.length === 0) {
    return metas;
  }
  return metas.filter((meta) => {
    const haystack = `${meta.title} ${meta.workspace.label} ${meta.workspace.root}`.toLowerCase();
    return terms.every((term) => haystack.includes(term));
  });
}

/** A clock time in local time: "HH:MM" for the same day, "Nov 8" for the same year, else "Nov 8, 2025". */
export function formatWhen(ts: number, now: number): string {
  const d = dateOf(ts);
  const n = dateOf(now);
  if (d === null || n === null) {
    return UNKNOWN_DATE;
  }
  if (d.getFullYear() === n.getFullYear() && d.getMonth() === n.getMonth() && d.getDate() === n.getDate()) {
    return `${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
  }
  const day = `${MONTHS_SHORT[d.getMonth()]} ${d.getDate()}`;
  return d.getFullYear() === n.getFullYear() ? day : `${day}, ${d.getFullYear()}`;
}

/**
 * How long ago, in short words: "just now" under a minute, "5m ago", "3h ago", and "2d ago" under a week. Older, or more than
 * a minute in the future, it is formatWhen. A time a little ahead (clock skew, under a minute) is "just now".
 */
export function relativeTime(ts: number, now: number): string {
  if (!Number.isFinite(ts) || !Number.isFinite(now)) {
    return UNKNOWN_DATE;
  }
  const diff = now - ts;
  if (diff < -60) {
    return formatWhen(ts, now);
  }
  if (diff < 60) {
    return 'just now';
  }
  if (diff < 3600) {
    return `${Math.floor(diff / 60)}m ago`;
  }
  if (diff < DAY_S) {
    return `${Math.floor(diff / 3600)}h ago`;
  }
  if (diff < 7 * DAY_S) {
    return `${Math.floor(diff / DAY_S)}d ago`;
  }
  return formatWhen(ts, now);
}

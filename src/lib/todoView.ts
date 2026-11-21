// The conversation's todo list as the transcript's todo panel shows it: the counts, the summary line, the rows of the expanded
// list, and the rules for showing content as plain text. Pure, so the rules are tested without the DOM.
//
// The list comes from the backend (todo_update events, and the loaded conversation's todos). Its content is model text, so
// it is shown as plain text only, with format and control characters (bidi overrides included) turned into spaces.

import type { TodoItem, TodoStatus } from './types';

/** The tool name of the todo tool. Its rows are not shown in the transcript: the panel is the view of the list. */
export const TODO_TOOL = 'todo_write';

/** The limits the backend's todo_write validation enforces (src-tauri/src/core/chat/todo.rs: MAX_ITEMS, MAX_CONTENT_CHARS). */
const MAX_ITEMS = 30;
const MAX_CONTENT_CHARS = 200;

/** Format and control characters: a control character is refused by the backend, so it is refused here too. */
const CONTROL = /\p{Cc}/u;

/**
 * The list a `todo_write` call would set, or null when the backend refuses the call (todo.rs `parse`): the items are not an
 * array, an item has no non-empty content (trimmed, no control characters, at most 200 characters) or a known status, there
 * are more than 30 items, or more than one is in_progress. A refused call sets nothing and is shown nothing. The content is
 * returned trimmed, as the backend stores it.
 */
export function parseTodoArgs(args: unknown): TodoItem[] | null {
  if (typeof args !== 'object' || args === null || !Array.isArray((args as Record<string, unknown>).items)) {
    return null;
  }
  const raw = (args as { items: unknown[] }).items;
  if (raw.length > MAX_ITEMS) {
    return null;
  }
  const items: TodoItem[] = [];
  for (const entry of raw) {
    const item = typeof entry === 'object' && entry !== null ? (entry as Record<string, unknown>) : {};
    const content = typeof item.content === 'string' ? item.content.trim() : '';
    if (content === '' || CONTROL.test(content) || Array.from(content).length > MAX_CONTENT_CHARS) {
      return null;
    }
    if (item.status !== 'pending' && item.status !== 'in_progress' && item.status !== 'completed') {
      return null;
    }
    items.push({ content, status: item.status });
  }
  if (items.filter((i) => i.status === 'in_progress').length > 1) {
    return null;
  }
  return items;
}

/** The status as a word for screen readers. The marks are drawn in CSS and carry no text. */
export function statusWord(status: TodoStatus): string {
  switch (status) {
    case 'completed':
      return 'Done';
    case 'in_progress':
      return 'In progress';
    default:
      return 'Pending';
  }
}

/** The longest task text kept in a title attribute, in characters. The backend refuses longer items, so this is a guard. */
export const TITLE_MAX = 200;

/** Format and control characters (bidi controls are format characters). Each run becomes one space. */
const UNSAFE = /[\p{Cc}\p{Cf}]+/gu;

/** The text as it is shown: unsafe characters become spaces, white space runs collapse, and the ends are trimmed. */
export function displayText(text: string): string {
  return text.replace(UNSAFE, ' ').replace(/\s+/g, ' ').trim();
}

/** A task's text as it is shown. An empty task (the backend refuses those) still needs something to show. */
function taskText(content: string): string {
  const text = displayText(content);
  return text === '' ? '(empty task)' : text;
}

/** The task's full text for a title attribute, cut to TITLE_MAX characters with an ellipsis. Cut by code point, never mid-pair. */
export function titleText(content: string): string {
  const chars = Array.from(taskText(content));
  return chars.length > TITLE_MAX ? `${chars.slice(0, TITLE_MAX - 1).join('')}…` : chars.join('');
}

export interface TodoCounts {
  total: number;
  done: number;
  inProgress: number;
  pending: number;
}

export function todoCounts(items: TodoItem[]): TodoCounts {
  let done = 0;
  let inProgress = 0;
  let pending = 0;
  for (const item of items) {
    if (item.status === 'completed') {
      done += 1;
    } else if (item.status === 'in_progress') {
      inProgress += 1;
    } else {
      pending += 1;
    }
  }
  return { total: items.length, done, inProgress, pending };
}

/**
 * The item the list marks as current: the first in_progress item, else the first item that is not completed. -1 when every
 * item is completed or the list is empty.
 */
export function currentIndex(items: TodoItem[]): number {
  const active = items.findIndex((i) => i.status === 'in_progress');
  if (active >= 0) {
    return active;
  }
  return items.findIndex((i) => i.status !== 'completed');
}

/** The one-line summary: `TODO 2/5 · <current task>`, or `TODO 5/5 done` when everything is completed. Empty for an empty list. */
export function todoSummary(items: TodoItem[]): string {
  if (items.length === 0) {
    return '';
  }
  const { total, done } = todoCounts(items);
  const current = currentIndex(items);
  if (current < 0) {
    return `TODO ${done}/${total} done`;
  }
  return `TODO ${done}/${total} · ${taskText(items[current].content)}`;
}

export interface TodoRow {
  /** The item's position in the list. Rows keep the list's order. */
  index: number;
  text: string;
  status: TodoStatus;
  current: boolean;
}

/** The rows of the expanded list, in the list's own order. The current item is marked, and nothing else changes its place. */
export function todoRows(items: TodoItem[]): TodoRow[] {
  const current = currentIndex(items);
  return items.map((item, index) => ({
    index,
    text: taskText(item.content),
    status: item.status,
    current: index === current,
  }));
}

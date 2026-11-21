// The transcript's tool row summary: a caption line and a detail line. Pure, so the wording, the escaping and the
// length caps are tested without the DOM. The model sends `summary` alongside the call's arguments (see the backend
// tool schemas); it is shown, never executed.

import { capDisplay, sendKeysDisplay } from './keysDisplay';
import { visibleEscapes } from './visibleEscapes';
import type { ToolStatus } from './transcript';

/** A caption keeps at most this many words; the cut is marked with an ellipsis. */
export const MAX_CAPTION_WORDS = 10;
/** A caption keeps at most this many characters; the cut is marked with an ellipsis. */
export const MAX_CAPTION_CHARS = 80;
/** A run_command row shows at most this many characters of the command. */
export const MAX_COMMAND_PREVIEW_CHARS = 80;
/** A send_keys row shows at most this many characters of the decoded input. */
export const MAX_SEND_KEYS_PREVIEW_CHARS = 60;

/**
 * The call's caption: its `summary` argument, trimmed, cut to ten words and ~80 characters, and escaped so a lookalike
 * or a bidi override in model text cannot hide what the row says. Newlines split words, so the caption is one line.
 * Null when there is no usable summary (absent, empty, or not a string).
 */
export function toolCaption(args: unknown): string | null {
  if (typeof args !== 'object' || args === null) {
    return null;
  }
  const { summary } = args as { summary?: unknown };
  if (typeof summary !== 'string') {
    return null;
  }
  const words = summary.trim().split(/\s+/).filter((w) => w !== '');
  if (words.length === 0) {
    return null;
  }
  const wordCut = words.length > MAX_CAPTION_WORDS;
  let kept = words.slice(0, MAX_CAPTION_WORDS).join(' ');
  // The cut is made before escaping, so an escape is never split; the ellipsis marking it is appended unescaped,
  // as capDisplay does.
  const chars = [...kept];
  const charCut = chars.length > MAX_CAPTION_CHARS;
  if (charCut) {
    kept = chars.slice(0, MAX_CAPTION_CHARS - 1).join('');
  }
  const escaped = visibleEscapes(kept);
  return wordCut || charCut ? `${escaped}…` : escaped;
}

/**
 * The row's second line: the run_command's command (newlines shown as ⏎, capped at 80 characters), or the send_keys
 * decoded input (capped at 60). Null for other tools, and when there is nothing to show.
 */
export function toolPreview(name: string, args: unknown): string | null {
  if (name === 'run_command') {
    if (typeof args !== 'object' || args === null) {
      return null;
    }
    const { command } = args as { command?: unknown };
    if (typeof command !== 'string' || command === '') {
      return null;
    }
    return capDisplay(command.replace(/\r\n|\n|\r/g, '⏎'), MAX_COMMAND_PREVIEW_CHARS);
  }
  if (name === 'send_keys') {
    const typed = sendKeysDisplay(args);
    if (typed === '') {
      return null;
    }
    return capDisplay(typed, MAX_SEND_KEYS_PREVIEW_CHARS);
  }
  return null;
}

/** A tool row's summary: the bare tool name, its caption, and its detail line. */
export interface ToolRowHead {
  label: string;
  caption: string | null;
  detail: string | null;
}

export function toolRowHead(name: string, args: unknown): ToolRowHead {
  return { label: visibleEscapes(name), caption: toolCaption(args), detail: toolPreview(name, args) };
}

/** The backend's default and largest `wait` seconds (core/tools DEFAULT_WAIT_SECONDS, MAX_WAIT_SECONDS). */
const DEFAULT_WAIT_SECONDS = 10;
const MAX_WAIT_SECONDS = 120;

/** The seconds a wait was asked for, as the backend clamps them: the default when absent or not an integer. */
function requestedWaitSeconds(args: unknown): number {
  const seconds = typeof args === 'object' && args !== null ? (args as { seconds?: unknown }).seconds : undefined;
  if (typeof seconds !== 'number' || !Number.isInteger(seconds)) {
    return DEFAULT_WAIT_SECONDS;
  }
  return Math.min(Math.max(seconds, 1), MAX_WAIT_SECONDS);
}

/** The time a wait took: from `waited_ms` of a live result, or from the text the model saw (`waited 4.9s: ...`). */
function waitedSeconds(result: unknown): string | null {
  if (typeof result === 'string') {
    const m = /^waited (\d+(?:\.\d+)?)s:/.exec(result);
    return m ? m[1] : null;
  }
  if (typeof result === 'object' && result !== null) {
    const ms = (result as { waited_ms?: unknown }).waited_ms;
    if (typeof ms === 'number' && Number.isFinite(ms) && ms >= 0) {
      return (ms / 1000).toFixed(1);
    }
  }
  return null;
}

/**
 * The duration a `wait` row shows in place of its bare status: `waiting 5s` while it runs (the requested seconds), then
 * `waited 4.9s` once it ends (the time it took, or the requested seconds when the result has no time). Null for other tools,
 * and for a failed wait (its status word stays).
 */
export function waitDuration(name: string, args: unknown, result: unknown, status: ToolStatus): string | null {
  if (name !== 'wait' || status === 'failed') {
    return null;
  }
  if (status === 'running') {
    return `waiting ${requestedWaitSeconds(args)}s`;
  }
  return `waited ${waitedSeconds(result) ?? String(requestedWaitSeconds(args))}s`;
}

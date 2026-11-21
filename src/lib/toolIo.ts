// The two views of a tool row's expanded body: the plain input and output, and the full JSON. Pure, so the layout of
// each view is tested without the DOM. The rows show the backend's raw result when the call ran in this session, and the
// text the model saw (a string) when the conversation was restored from disk. Both are handled here.

import { sendKeysDisplay, displayText } from './keysDisplay';
import { previewJson } from './format';
import { visibleEscapes } from './visibleEscapes';

/** The JSON view keeps this many characters before it cuts (the cut is marked). */
export const MAX_JSON_PREVIEW_CHARS = 20000;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** One `key: value` line. A string with line breaks starts on the line after its key, so its lines stay as they are. */
function keyLine(key: string, value: unknown, text: (s: string) => string): string {
  // A list of strings (a screen's rows, the keys sent) is its items, one per line.
  const str = Array.isArray(value) && value.length > 0 && value.every((v) => typeof v === 'string') ? value.join('\n') : value;
  if (typeof str === 'string') {
    const shown = text(str);
    return shown.includes('\n') ? `${key}:\n${shown}` : `${key}: ${shown}`;
  }
  return `${key}: ${JSON.stringify(value) ?? 'null'}`;
}

/** Drops the parts that are absent (null) or empty, and joins the rest as lines. */
function lines(parts: (string | null)[]): string {
  return parts.filter((s): s is string => s !== null && s !== '').join('\n');
}

/**
 * The input as it is sent: run_command's command as typed, send_keys' decoded input, and any other call's arguments as
 * `key: value` lines. Text is escaped as visibleEscapes does (the same as the approval card), so a lookalike or a bidi
 * override cannot hide what runs. Newlines are kept.
 */
export function toolInput(name: string, args: unknown): string {
  if (name === 'run_command' && isRecord(args) && typeof args.command === 'string') {
    return visibleEscapes(args.command) || '(empty)';
  }
  if (name === 'send_keys') {
    const typed = sendKeysDisplay(args);
    if (typed !== '') {
      return displayText(typed);
    }
  }
  if (args === undefined || args === null) {
    return '';
  }
  if (!isRecord(args)) {
    return typeof args === 'string' ? visibleEscapes(args) : JSON.stringify(args) ?? '';
  }
  return Object.entries(args)
    .map(([key, value]) => keyLine(key, value, visibleEscapes))
    .join('\n');
}

/** How long a wait ended after, as the wait text says it: `waited 4.9s`. Null without a usable number. */
function waitedLabel(v: Record<string, unknown>): string | null {
  return typeof v.waited_ms === 'number' ? `waited ${(v.waited_ms / 1000).toFixed(1)}s` : null;
}

const WAIT_REASONS: Record<string, string> = {
  idle: 'the shell was idle at the prompt',
  session_exited: 'the session exited',
  command_finished: 'the command finished',
  screen_changed: 'the screen changed',
  timeout: 'the wait time ran out',
  cancelled: 'the turn was cancelled',
};

/** The screen rows of a result, trailing blank rows dropped. Empty when there is no screen. */
function screenRows(screen: unknown): string[] {
  if (!isRecord(screen) || !Array.isArray(screen.rows)) {
    return [];
  }
  const rows = screen.rows.map((row) => (typeof row === 'string' ? row.trimEnd() : ''));
  while (rows.length > 0 && rows[rows.length - 1] === '') {
    rows.pop();
  }
  return rows;
}

function runOutput(v: Record<string, unknown>): string {
  const note = typeof v.note === 'string' ? v.note : null;
  // A restored finished command is its flat record (no outcome): the text the model saw has the record's fields.
  if (v.outcome === 'finished' || (v.outcome === undefined && typeof v.output === 'string')) {
    const record = v.outcome === undefined ? v : isRecord(v.record) ? v.record : {};
    const output = typeof record.output === 'string' ? record.output.replace(/\n+$/, '') : '';
    const exit = record.exit_code === undefined || record.exit_code === null ? 'exit unknown' : `exit ${String(record.exit_code)}`;
    const status = [exit, typeof record.status === 'string' ? record.status : null, record.output_truncated === true ? 'output truncated' : null]
      .filter((s) => s !== null)
      .join(' · ');
    return lines([output, status, note]);
  }
  if (v.outcome === 'running') {
    const status = [
      'still running',
      typeof v.reason === 'string' ? v.reason : null,
      typeof v.record_id === 'number' ? `record ${v.record_id}` : null,
    ]
      .filter((s) => s !== null)
      .join(' · ');
    const tail = Array.isArray(v.screen_tail) ? v.screen_tail.map((row) => (typeof row === 'string' ? row : '')).join('\n') : null;
    return lines([tail, status, note]);
  }
  if (v.outcome === 'not_run') {
    return lines([`not run: ${typeof v.reason === 'string' ? v.reason : 'unknown reason'}`, note]);
  }
  return genericOutput(v);
}

function waitOutput(v: Record<string, unknown>): string {
  const reason = typeof v.reason === 'string' ? (WAIT_REASONS[v.reason] ?? v.reason) : 'the wait ended';
  const head = [waitedLabel(v) ?? 'waited ?', reason].join(': ');
  const note = typeof v.note === 'string' ? v.note : null;
  const record = isRecord(v.record) ? v.record : null;
  const body = record !== null && typeof record.output === 'string' ? record.output.replace(/\n+$/, '') : screenRows(v.screen).join('\n');
  return lines([head, note, body]);
}

/** Every key of an object as a line, in its order. */
function genericOutput(v: Record<string, unknown>): string {
  return Object.entries(v)
    .map(([key, value]) => keyLine(key, value, (s) => s))
    .join('\n');
}

/** A result text that is a JSON object or array (run_command's and the other tools' records are stored as JSON text), or null. */
function parsedObject(text: string): Record<string, unknown> | null {
  try {
    const value: unknown = JSON.parse(text);
    return isRecord(value) ? value : null;
  } catch {
    return null;
  }
}

/**
 * The output as the user reads it. A string result is shown verbatim (a restored row's result is the model's text). A
 * run_command result shows its output, then a status line (exit code, outcome), then the note the backend added. A wait
 * result shows how long it waited and why it ended, then the command's output or the screen. Other objects are `key: value`
 * lines. Empty when there is no result yet.
 */
export function toolOutput(name: string, result: unknown): string {
  if (result === undefined || result === null) {
    return '';
  }
  let value: unknown = result;
  if (typeof result === 'string') {
    if (name !== 'run_command') {
      return result;
    }
    const parsed = parsedObject(result);
    if (parsed === null) {
      return result;
    }
    value = parsed;
  }
  if (typeof value === 'string') {
    return value;
  }
  if (!isRecord(value)) {
    return JSON.stringify(value) ?? '';
  }
  if (name === 'run_command') {
    return runOutput(value);
  }
  if (name === 'wait') {
    return waitOutput(value);
  }
  return genericOutput(value);
}

/**
 * The JSON view: the call's arguments and its result, pretty-printed and cut at MAX_JSON_PREVIEW_CHARS. A string result
 * that is a JSON object or array is shown parsed, so the record reads as JSON; any other string is shown as stored.
 */
export function toolJson(args: unknown, result: unknown): string {
  const shown = typeof result === 'string' ? (parsedObject(result) ?? result) : result;
  return previewJson({ args, result: shown }, MAX_JSON_PREVIEW_CHARS);
}

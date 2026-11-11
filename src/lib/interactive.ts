// Interactive command groups. A command the agent started is still running after its run_command call returned (the result
// is outcome 'running' with a record_id). From that tool row until the command ends, the agent's steps for it (wait, get_screen,
// send_keys, approvals, its text) are nested in one group. The group opens on the tool row and ends at the record's
// `record_end` item (the reducer appends it on record_finished), or at a tool result that names the record (a wait, send_keys or
// run_command result with its `record`). Pure: the transcript view builds the groups from the items, and the reducer asks
// which ones are open.
//
// Nesting: a group opened while another is open is nested in it, and the items go to the latest one still open.

import { capDisplay } from './keysDisplay';
import type { TranscriptItem } from './transcript';

/** A run_command result this group opens on: a live result (an object), with the record id of the command it left running. */
export interface OpenedBy {
  recordId: number;
  command: string;
}

/** An event that ends the group of `recordId`, with the exit code when there is one. */
interface EndedBy {
  recordId: number;
  exit: number | null;
}

/** A group's contents: an item, or a nested group. */
export type FrameNode = { kind: 'item'; item: TranscriptItem } | { kind: 'frame'; frame: Frame };

export interface Frame {
  /** The id of the opening tool row. Stable, so it can be a React key. */
  id: number;
  /** The call id of the opening tool row (its approval card, if any, comes between the row and its result). */
  callId: string;
  recordId: number;
  command: string;
  /** When the call that opened the group started (Unix ms), or null when unknown. */
  startedAt: number | null;
  /** Null while the command runs. */
  ended: { exit: number | null } | null;
  children: FrameNode[];
}

/** The max length of the command text in a group's header. */
export const HEADER_COMMAND_CHARS = 60;

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** The group a tool row opens: a live run_command result that says the command is still running. Null for anything else. */
export function openedBy(item: TranscriptItem): OpenedBy | null {
  if (item.kind !== 'tool' || item.name !== 'run_command' || !isObject(item.result)) {
    return null;
  }
  const { outcome, record_id: recordId } = item.result;
  if (outcome !== 'running' || typeof recordId !== 'number') {
    return null;
  }
  const command = isObject(item.args) && typeof item.args.command === 'string' ? item.args.command : '';
  return { recordId, command };
}

/** The end of a group: a record_end item, or a tool result whose `record` is the command (a finished wait, send_keys or run_command). */
export function endedBy(item: TranscriptItem): EndedBy | null {
  if (item.kind === 'record_end') {
    return { recordId: item.recordId, exit: item.exit };
  }
  if (item.kind !== 'tool' || !isObject(item.result)) {
    return null;
  }
  const record = item.result.record;
  if (!isObject(record) || typeof record.id !== 'number') {
    return null;
  }
  return { recordId: record.id, exit: typeof record.exit_code === 'number' ? record.exit_code : null };
}

/**
 * The groups of a transcript: the root's nodes (items and top-level groups, in order) and the groups still open at the end.
 * An end seen before its opener (the command finished while the call was returning) gives a group with no contents, ended.
 */
export function buildFrames(items: TranscriptItem[]): { children: FrameNode[]; open: Frame[] } {
  // The first end seen for each record. A group whose opener comes after its end is closed where it opens.
  const firstEnd = new Map<number, { index: number; exit: number | null }>();
  items.forEach((item, index) => {
    const end = endedBy(item);
    if (end !== null && !firstEnd.has(end.recordId)) {
      firstEnd.set(end.recordId, { index, exit: end.exit });
    }
  });

  const root: Frame = { id: -1, callId: '', recordId: -1, command: '', startedAt: null, ended: null, children: [] };
  const stack: Frame[] = [root];
  const opened = new Set<number>();

  items.forEach((item, index) => {
    const top = stack[stack.length - 1];
    const opener = openedBy(item);
    if (opener !== null && !opened.has(opener.recordId)) {
      opened.add(opener.recordId);
      const frame: Frame = {
        id: item.id,
        callId: item.kind === 'tool' ? item.callId : '',
        recordId: opener.recordId,
        command: opener.command,
        startedAt: item.kind === 'tool' ? item.startedAt ?? null : null,
        ended: null,
        children: [],
      };
      top.children.push({ kind: 'frame', frame });
      const end = firstEnd.get(opener.recordId);
      if (end === undefined || end.index > index) {
        stack.push(frame);
      } else {
        frame.ended = { exit: end.exit };
      }
      return;
    }
    // The approval of the run_command call that opened the group came between its row and its result. It is about the start,
    // so it stays above the group, not inside it.
    if (item.kind === 'approval' && stack.length > 1 && top.children.length === 0 && top.callId === item.callId) {
      const parent = stack[stack.length - 2];
      parent.children.splice(parent.children.length - 1, 0, { kind: 'item', item });
      return;
    }
    const end = endedBy(item);
    if (end !== null) {
      const found = latestIndexOf(stack, end.recordId);
      if (found > 0) {
        const frame = stack[found];
        // A finished tool row belongs to the group it ends. A record_end item is not shown.
        if (item.kind === 'tool') {
          frame.children.push({ kind: 'item', item });
        }
        frame.ended = { exit: end.exit };
        stack.splice(found, 1);
        return;
      }
    }
    top.children.push({ kind: 'item', item });
  });

  return { children: root.children, open: stack.slice(1) };
}

/** The index of the last frame in the stack for `recordId`, or -1. (Array.prototype.findLastIndex is not in this build's lib.) */
function latestIndexOf(stack: Frame[], recordId: number): number {
  for (let i = stack.length - 1; i >= 0; i -= 1) {
    if (stack[i].recordId === recordId) {
      return i;
    }
  }
  return -1;
}

/** The record ids of the groups open at the end of the transcript. */
export function openRecordIds(items: TranscriptItem[]): Set<number> {
  return new Set(buildFrames(items).open.map((f) => f.recordId));
}

/** `ms` as the header shows it: "42s", "3m 05s", "2h 07m". Negative values read as zero. */
export function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  if (total < 60) {
    return `${total}s`;
  }
  const minutes = Math.floor(total / 60);
  const seconds = String(total % 60).padStart(2, '0');
  if (minutes < 60) {
    return `${minutes}m ${seconds}s`;
  }
  return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, '0')}m`;
}

/**
 * The header of a group: "Interactive · <command> · running 3m 05s", then "finished · exit N", or "ended" when the exit code is
 * not known. The command is escaped and capped; a newline in it shows as \n so the header stays one line.
 */
export function interactiveHeader(group: { command: string; startedAt: number | null; ended: { exit: number | null } | null }, now: number): string {
  const command = capDisplay(group.command, HEADER_COMMAND_CHARS).replace(/\n/g, '\\n');
  const prefix = command === '' ? 'Interactive' : `Interactive · ${command}`;
  if (group.ended !== null) {
    return group.ended.exit === null ? `${prefix} · ended` : `${prefix} · finished · exit ${group.ended.exit}`;
  }
  return group.startedAt === null ? `${prefix} · running` : `${prefix} · running ${formatElapsed(now - group.startedAt)}`;
}

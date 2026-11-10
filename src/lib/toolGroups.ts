// Groups a run of consecutive tool calls in the transcript into one collapsed line. Pure: the view feeds it the items
// as they are and renders what comes back.

import type { TranscriptItem } from './transcript';
import { visibleEscapes } from './visibleEscapes';

type ToolItem = Extract<TranscriptItem, { kind: 'tool' }>;
type ApprovalItem = Extract<TranscriptItem, { kind: 'approval' }>;

/** What a group may hold: a tool call, or an approval that has been answered. */
export type GroupItem = ToolItem | ApprovalItem;

export type TranscriptEntry = { kind: 'item'; item: TranscriptItem } | { kind: 'tools'; key: number; items: GroupItem[] };

/**
 * A run of consecutive tool and resolved approval items. A pending or answering approval is never inside a run: it is
 * rendered as its own card, and it ends the run before it. Any other item ends a run too.
 */
function groupable(item: TranscriptItem): item is GroupItem {
  return item.kind === 'tool' || (item.kind === 'approval' && item.state === 'resolved');
}

/**
 * Collapses each run with two or more tool calls into one entry. A run with one tool call (plus any resolved approvals)
 * is returned item by item, as it was before grouping. The key is the id of the run's first item, so it stays the same
 * while the run grows and a group the user opened stays open.
 */
export function groupTranscript(items: TranscriptItem[]): TranscriptEntry[] {
  const entries: TranscriptEntry[] = [];
  let run: GroupItem[] = [];
  const flush = () => {
    if (run.length === 0) {
      return;
    }
    const toolCount = run.filter((i) => i.kind === 'tool').length;
    if (toolCount >= 2) {
      entries.push({ kind: 'tools', key: run[0].id, items: run });
    } else {
      for (const item of run) {
        entries.push({ kind: 'item', item });
      }
    }
    run = [];
  };
  for (const item of items) {
    if (groupable(item)) {
      run.push(item);
    } else {
      flush();
      entries.push({ kind: 'item', item });
    }
  }
  flush();
  return entries;
}

/**
 * The one-line summary of a group: the call count, the tool names with repeat counts, then the status. A call still
 * running names the last one; failures and denials are counted or named after it.
 */
export function toolGroupSummary(items: GroupItem[]): string {
  const tools = items.filter((i): i is ToolItem => i.kind === 'tool');
  const counts = new Map<string, number>();
  for (const tool of tools) {
    counts.set(tool.name, (counts.get(tool.name) ?? 0) + 1);
  }
  const names = [...counts].map(([name, n]) => (n > 1 ? `${visibleEscapes(name)} ×${n}` : visibleEscapes(name))).join(', ');
  let text = `${tools.length} tool calls: ${names}`;

  const running = [...tools].reverse().find((t) => t.status === 'running');
  if (running) {
    text += ` · running ${visibleEscapes(running.name)}`;
  }
  const failed = tools.filter((t) => t.status === 'failed').length;
  if (failed > 0) {
    text += ` · ${failed} failed`;
  }
  if (tools.some((t) => t.decision === 'denied')) {
    text += ' · denied';
  }
  return text;
}

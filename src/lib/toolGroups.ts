// Groups a run of consecutive tool calls in the transcript into one collapsed line. Pure: the view feeds it the items
// as they are and renders what comes back.

import { isDeclinedIntegration } from './integrate';
import { buildFrames } from './interactive';
import type { FrameNode, GroupEnd } from './interactive';
import { toolCaption } from './toolDisplay';
import type { TranscriptItem } from './transcript';
import { visibleEscapes } from './visibleEscapes';

type ToolItem = Extract<TranscriptItem, { kind: 'tool' }>;
type ApprovalItem = Extract<TranscriptItem, { kind: 'approval' }>;

/** What a group may hold: a tool call, or an approval that has been answered. */
export type GroupItem = ToolItem | ApprovalItem;

/** An interactive group as the view shows it: its header's facts, and its contents in order (see interactive.ts). */
export interface InteractiveGroup {
  recordId: number;
  command: string;
  startedAt: number | null;
  /** Null while the command runs. */
  ended: GroupEnd | null;
  entries: TranscriptEntry[];
}

export type TranscriptEntry =
  | { kind: 'item'; item: TranscriptItem }
  | { kind: 'tools'; key: number; items: GroupItem[] }
  /** `key` is the id of the tool row that opened the group. */
  | { kind: 'interactive'; key: number; group: InteractiveGroup };

/**
 * A run of consecutive tool and resolved approval items. A pending or answering approval is never inside a run: it is
 * rendered as its own card, and it ends the run before it. Any other item ends a run too.
 */
function groupable(item: TranscriptItem): item is GroupItem {
  return item.kind === 'tool' || (item.kind === 'approval' && item.state === 'resolved');
}

/**
 * The transcript's entries. Items are collapsed into runs (see collapseRun), and each interactive group (interactive.ts) is an
 * entry holding its own entries, so the runs inside a group collapse the same way.
 */
export function groupTranscript(items: TranscriptItem[]): TranscriptEntry[] {
  return entriesOf(buildFrames(items).children);
}

function entriesOf(nodes: FrameNode[]): TranscriptEntry[] {
  const entries: TranscriptEntry[] = [];
  let run: TranscriptItem[] = [];
  const flush = () => {
    entries.push(...collapseRun(run));
    run = [];
  };
  for (const node of nodes) {
    if (node.kind === 'frame') {
      flush();
      const { frame } = node;
      entries.push({
        kind: 'interactive',
        key: frame.id,
        group: {
          recordId: frame.recordId,
          command: frame.command,
          startedAt: frame.startedAt,
          ended: frame.ended,
          entries: entriesOf(frame.children),
        },
      });
    } else if (groupable(node.item)) {
      run.push(node.item);
    } else {
      flush();
      entries.push({ kind: 'item', item: node.item });
    }
  }
  flush();
  return entries;
}

/**
 * Collapses a run into one entry when it has two or more tool calls, or one tool call and at least one resolved
 * approval (the approval is the reason for the call, so it is folded in with it). A run with a single tool call and
 * no approval, or with approvals and no tool call, is returned item by item. The key is the id of the run's first
 * item, so it stays the same while the run grows and a group the user opened stays open.
 */
function collapseRun(run: TranscriptItem[]): TranscriptEntry[] {
  if (run.length === 0) {
    return [];
  }
  const items = run.filter(groupable);
  const toolCount = items.filter((i) => i.kind === 'tool').length;
  const approvalCount = items.length - toolCount;
  if (toolCount >= 2 || (toolCount >= 1 && approvalCount >= 1)) {
    return [{ kind: 'tools', key: items[0].id, items }];
  }
  return run.map((item): TranscriptEntry => ({ kind: 'item', item }));
}

/**
 * The one-line summary of a group: the call count, the tool names with repeat counts, the first caption, then the
 * status. A call still running names the last one; failures and denials are counted or named after it.
 */
export function toolGroupSummary(items: GroupItem[]): string {
  const tools = items.filter((i): i is ToolItem => i.kind === 'tool');
  const counts = new Map<string, number>();
  for (const tool of tools) {
    counts.set(tool.name, (counts.get(tool.name) ?? 0) + 1);
  }
  const names = [...counts].map(([name, n]) => (n > 1 ? `${visibleEscapes(name)} ×${n}` : visibleEscapes(name))).join(', ');
  let text = `${tools.length === 1 ? '1 tool call' : `${tools.length} tool calls`}: ${names}`;

  // A collapsed group says what its first call does; the expanded rows show every caption. More captions hide
  // behind an ellipsis.
  const captions = tools.map((t) => toolCaption(t.args)).filter((c): c is string => c !== null);
  if (captions.length > 0) {
    text += ` · ${captions[0]}`;
    if (captions.length > 1) {
      text += ' …';
    }
  }

  const running = [...tools].reverse().find((t) => t.status === 'running');
  if (running) {
    text += ` · running ${visibleEscapes(running.name)}`;
  }
  // A declined integration request is counted as declined, not as failed or denied.
  const declined = (t: ToolItem) => isDeclinedIntegration(t.name, t.decision);
  const failed = tools.filter((t) => t.status === 'failed' && !declined(t)).length;
  if (failed > 0) {
    text += ` · ${failed} failed`;
  }
  if (tools.some((t) => t.decision === 'denied' && !declined(t))) {
    text += ' · denied';
  }
  if (tools.some(declined)) {
    text += ' · declined';
  }
  return text;
}

import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { Transcript } from './Transcript';
import { HistoryContent, groupsFor } from './HistoryView';
import type { HistoryContentProps, LoadState } from './HistoryView';
import { BUSY_TIP } from '../lib/switchGuard';
import type { ConversationMeta, Workspace } from '../lib/types';

// The New button and the delete controls are blocked while a switch is not allowed (busy, or a side command runs). They are
// aria-disabled rather than disabled, so the reason in data-tip stays reachable on hover and focus.

const NOOP = () => undefined;

function renderTranscript(over: { busy?: boolean; newBlocked?: boolean } = {}): string {
  return renderToStaticMarkup(
    <Transcript
      items={[]}
      busy={over.busy ?? false}
      newBlocked={over.newBlocked}
      thinking={false}
      open
      onToggle={NOOP}
      onApprove={NOOP}
      onAnswerQuestions={NOOP}
      onNewConversation={NOOP}
    />,
  );
}

/** The opening tag of the New button. */
function newButton(html: string): string {
  const at = html.indexOf('class="transcript-new"');
  expect(at, 'the New button is in the markup').toBeGreaterThan(-1);
  return html.slice(html.lastIndexOf('<button', at), html.indexOf('>', at) + 1);
}

describe('Transcript: the New button while a switch is blocked', () => {
  it('idle: enabled, with the plain tooltip', () => {
    const b = newButton(renderTranscript());
    expect(b).not.toContain('aria-disabled');
    expect(b).not.toMatch(/\sdisabled(=|\s|>)/);
    expect(b).toContain('data-tip="Clears the agent conversation"');
  });

  it('while the agent is busy: aria-disabled, with the busy tooltip, and no native disabled', () => {
    const b = newButton(renderTranscript({ busy: true }));
    expect(b).toContain('aria-disabled="true"');
    expect(b).not.toMatch(/\sdisabled(=|\s|>)/);
    expect(b).toContain(`data-tip="${BUSY_TIP}"`);
  });

  it('while a side command runs (newBlocked, the agent itself idle): blocked too', () => {
    const b = newButton(renderTranscript({ busy: false, newBlocked: true }));
    expect(b).toContain('aria-disabled="true"');
    expect(b).toContain(`data-tip="${BUSY_TIP}"`);
  });

  it('newBlocked overrides busy in both directions', () => {
    expect(newButton(renderTranscript({ busy: true, newBlocked: false }))).not.toContain('aria-disabled');
    expect(newButton(renderTranscript({ busy: false, newBlocked: true }))).toContain('aria-disabled="true"');
  });
});

const ALPHA: Workspace = { host: 'mac', root: '/Users/me/alpha', label: 'alpha' };
const NOW = Math.floor(new Date(2026, 10, 10, 12, 0).getTime() / 1000);

function meta(id: string, title: string): ConversationMeta {
  return { id, title, created_at: NOW, updated_at: NOW, message_count: 2, workspace: ALPHA, model: null };
}

const METAS = [meta('t1', 'Fix the build')];

function historyHtml(over: Partial<HistoryContentProps> = {}): string {
  const load: LoadState = over.load ?? { kind: 'ready', metas: METAS, now: NOW };
  const props: HistoryContentProps = {
    load,
    query: '',
    groupBy: 'date',
    groups: groupsFor(load.kind === 'ready' ? load.metas : [], 'date', NOW),
    now: NOW,
    currentId: null,
    busy: false,
    opening: null,
    actionError: null,
    collapsed: new Set(),
    confirm: null,
    onQuery: NOOP,
    onGroupBy: NOOP,
    onToggleGroup: NOOP,
    onConfirm: NOOP,
    onOpen: NOOP,
    onDelete: NOOP,
    onDeleteAll: NOOP,
    onRetry: NOOP,
    onEscape: NOOP,
    ...over,
  };
  return renderToStaticMarkup(<HistoryContent {...props} />);
}

/** The opening tags of every element whose markup contains `cls`. */
function tagsWith(html: string, cls: string): string[] {
  return html.match(new RegExp(`<button[^>]*class="${cls}"[^>]*>`, 'g')) ?? [];
}

describe('HistoryContent: delete controls while the agent is busy', () => {
  it('the row delete button is aria-disabled with the busy tooltip', () => {
    const [b] = tagsWith(historyHtml({ busy: true }), 'history-delete');
    expect(b).toContain('aria-disabled="true"');
    expect(b).not.toMatch(/\sdisabled(=|\s|>)/);
    expect(b).toContain(`data-tip="${BUSY_TIP}"`);
  });

  it('when idle the row delete button is enabled, with its own tooltip', () => {
    const [b] = tagsWith(historyHtml(), 'history-delete');
    expect(b).not.toContain('aria-disabled');
    expect(b).toContain('data-tip="Delete this conversation"');
  });

  it('Delete all history is aria-disabled with the busy tooltip while busy, and enabled when idle', () => {
    const busy = historyHtml({ busy: true });
    expect(busy).toContain('class="history-delete-all" aria-disabled="true"');
    expect(busy).toContain(`data-tip="${BUSY_TIP}"`);
    const idle = historyHtml();
    expect(idle).toContain('class="history-delete-all"');
    expect(idle).not.toContain('class="history-delete-all" aria-disabled');
  });

  it('the confirm Yes buttons are aria-disabled while busy', () => {
    const rowYes = historyHtml({ busy: true, confirm: { kind: 'row', id: 't1' } });
    expect(rowYes).toMatch(/class="history-danger" aria-disabled="true"[^>]*>Yes</);
    const allYes = historyHtml({ busy: true, confirm: { kind: 'all' } });
    expect(allYes).toMatch(/class="history-danger" aria-disabled="true"[^>]*>Yes</);
  });
});

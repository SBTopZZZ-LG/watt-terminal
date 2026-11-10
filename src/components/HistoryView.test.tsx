import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { groupsFor, HistoryContent } from './HistoryView';
import type { HistoryContentProps, LoadState } from './HistoryView';
import type { ConversationMeta, Workspace } from '../lib/types';

/** Local time, so the grouping in these tests is the same in any time zone. */
const local = (y: number, mo: number, d: number, h = 0, mi = 0) => Math.floor(new Date(y, mo, d, h, mi, 0).getTime() / 1000);
const NOW = local(2026, 10, 10, 12, 0);

const ALPHA: Workspace = { host: 'mac', root: '/Users/me/alpha', label: 'alpha' };
const BETA: Workspace = { host: 'mac', root: '/Users/me/beta', label: 'beta' };

function meta(id: string, title: string, updated: number, workspace: Workspace = ALPHA, messages = 4): ConversationMeta {
  return { id, title, created_at: updated, updated_at: updated, message_count: messages, workspace, model: null };
}

const METAS = [meta('t1', 'Fix the build', local(2026, 10, 10, 9), ALPHA), meta('t2', 'Explain <b>bold</b> log', local(2026, 10, 9, 20), BETA, 1)];

const NOOP = () => undefined;

/** Props for the presentational view. Every callback is a no-op; tests override the fields they look at. */
function props(over: Partial<HistoryContentProps> = {}): HistoryContentProps {
  const load: LoadState = over.load ?? { kind: 'ready', metas: METAS, now: NOW };
  const metas = load.kind === 'ready' ? load.metas : [];
  const groupBy = over.groupBy ?? 'date';
  return {
    load,
    query: '',
    groupBy,
    groups: groupsFor(metas, groupBy, NOW),
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
}

const html = (over: Partial<HistoryContentProps> = {}) => renderToStaticMarkup(<HistoryContent {...props(over)} />);

describe('HistoryContent: groups and rows', () => {
  it('shows a heading per date group, with its count, and each conversation as a row', () => {
    const out = html();
    expect(out).toContain('Today');
    expect(out).toContain('Yesterday');
    expect(out).toContain('class="history-count">1<');
    expect(out).toContain('Fix the build');
    expect(out).toContain('Explain &lt;b&gt;bold&lt;/b&gt; log');
  });

  it('titles and workspace labels are escaped text, never markup', () => {
    const out = html();
    expect(out).not.toContain('<b>bold</b>');
    expect(out).toContain('&lt;b&gt;bold&lt;/b&gt;');
  });

  it('date mode shows the workspace chip on each row', () => {
    const out = html({ groupBy: 'date' });
    expect(out).toContain('class="history-chip"');
    expect(out).toContain('>alpha</span>');
  });

  it('workspace mode shows the root in the heading, and the time on each row, with no chip', () => {
    const out = html({ groupBy: 'workspace', groups: groupsFor(METAS, 'workspace', NOW) });
    expect(out).toContain('class="history-root"');
    expect(out).toContain('/Users/me/alpha');
    expect(out).not.toContain('class="history-chip"');
    expect(out).toContain('09:00');
  });

  it('message counts are singular for one and plural otherwise', () => {
    const out = html();
    expect(out).toContain('1 message<');
    expect(out).toContain('4 messages<');
  });

  it('the grouping toggle marks the current grouping as pressed', () => {
    expect(html({ groupBy: 'date' })).toMatch(/aria-pressed="true"[^>]*>Date</);
    expect(html({ groupBy: 'workspace' })).toMatch(/aria-pressed="true"[^>]*>Workspace</);
    expect(html({ groupBy: 'workspace' })).toMatch(/aria-pressed="false"[^>]*>Date</);
  });

  it('a collapsed group keeps its heading and hides its rows', () => {
    const key = groupsFor(METAS, 'date', NOW)[0].key;
    const out = html({ collapsed: new Set([key]) });
    expect(out).toContain('aria-expanded="false"');
    expect(out).not.toContain('Fix the build');
    expect(out).toContain('Explain');
  });

  it('a group heading says whether it is expanded', () => {
    expect(html()).toContain('aria-expanded="true"');
  });

  it('the filter box is a labelled search input with its placeholder', () => {
    const out = html({ query: 'build' });
    expect(out).toContain('placeholder="Filter conversations"');
    expect(out).toContain('aria-label="Filter conversations"');
    expect(out).toContain('value="build"');
  });
});

describe('HistoryContent: states', () => {
  it('loading shows a loading line and no list', () => {
    const out = html({ load: { kind: 'loading' }, groups: [] });
    expect(out).toContain('Loading conversations...');
    expect(out).not.toContain('history-open');
  });

  it('an error shows the message in an alert, with Retry, and no rows', () => {
    const out = html({ load: { kind: 'error', message: 'store is gone' }, groups: [] });
    expect(out).toContain('role="alert"');
    expect(out).toContain('Could not load the history: store is gone');
    expect(out).toContain('>Retry</button>');
    expect(out).not.toContain('history-open');
  });

  it('no saved conversations gives the empty state, and no delete-all footer', () => {
    const out = html({ load: { kind: 'ready', metas: [], now: NOW }, groups: [] });
    expect(out).toContain('No saved conversations yet.');
    expect(out).not.toContain('Delete all history');
  });

  it('a filter that matches nothing gives the no-match line, with the query as text', () => {
    const out = html({ query: '  <nope>  ', groups: [] });
    expect(out).toContain('No conversations match &quot;&lt;nope&gt;&quot;.');
    expect(out).not.toContain('No saved conversations yet.');
  });

  it('an action error is shown above the list', () => {
    const out = html({ actionError: 'the agent is working' });
    expect(out).toContain('role="alert"');
    expect(out).toContain('the agent is working');
  });
});

describe('HistoryContent: current conversation and busy rows', () => {
  it('marks the live conversation as current, with aria-current and a tag', () => {
    const out = html({ currentId: 't2' });
    expect(out.split('is-current').length).toBe(2);
    expect(out.split('aria-current="true"').length).toBe(2);
    expect(out).toContain('class="history-current">current<');
  });

  it('with no live conversation in the list, nothing is marked', () => {
    expect(html({ currentId: 'ses_missing' })).not.toContain('history-current');
  });

  it('while the agent is busy every open button is disabled, with the reason as its title', () => {
    const out = html({ busy: true });
    const buttons = out.match(/<button[^>]*class="history-open"[^>]*>/g) ?? [];
    expect(buttons).toHaveLength(2);
    for (const b of buttons) {
      expect(b).toContain('disabled=""');
      expect(b).toContain('Wait for the agent to finish');
    }
  });

  it('when idle the open buttons are enabled, with the title and root', () => {
    const out = html();
    expect(out).not.toMatch(/class="history-open"[^>]*disabled/);
    expect(out).toContain('title="Fix the build\n/Users/me/alpha"');
  });

  it('while one conversation is opening the others are disabled and it says so', () => {
    const out = html({ opening: 't1' });
    expect(out).toContain('Opening...');
    const buttons = out.match(/<button[^>]*class="history-open"[^>]*>/g) ?? [];
    expect(buttons.every((b) => b.includes('disabled=""'))).toBe(true);
  });

  it('busy does not disable the delete buttons', () => {
    const out = html({ busy: true });
    expect(out).not.toMatch(/class="history-delete"[^>]*disabled/);
  });
});

describe('HistoryContent: delete confirmation', () => {
  it('a row shows its delete button by default, labelled with its title', () => {
    const out = html();
    expect(out).toContain('aria-label="Delete Fix the build"');
    expect(out).not.toContain('Delete?');
  });

  it('a row awaiting confirmation shows Delete? with Yes and No, and no delete button', () => {
    const out = html({ confirm: { kind: 'row', id: 't1' } });
    expect(out).toContain('Delete?');
    expect(out).toContain('>Yes</button>');
    expect(out).toContain('>No</button>');
    // Only the other row keeps its delete button.
    expect(out.split('class="history-delete"').length).toBe(2);
  });

  it('the footer asks before deleting everything', () => {
    expect(html()).toContain('>Delete all history</button>');
    const out = html({ confirm: { kind: 'all' } });
    expect(out).toContain('Delete all conversations?');
    expect(out).not.toContain('>Delete all history</button>');
  });

  it('the footer is not shown while the list is loading', () => {
    expect(html({ load: { kind: 'loading' }, groups: [] })).not.toContain('history-footer');
  });
});

describe('groupsFor', () => {
  it('date mode groups by day, and the keys are prefixed so modes do not share collapse state', () => {
    const groups = groupsFor(METAS, 'date', NOW);
    expect(groups.map((g) => g.label)).toEqual(['Today', 'Yesterday']);
    expect(groups[0].key.startsWith('date:')).toBe(true);
    expect(groups.every((g) => g.sub === '')).toBe(true);
  });

  it('workspace mode groups by workspace, with the root as the sub line', () => {
    const groups = groupsFor(METAS, 'workspace', NOW);
    expect(groups.map((g) => g.label)).toEqual(['alpha', 'beta']);
    expect(groups[0]).toMatchObject({ sub: '/Users/me/alpha' });
    expect(groups[0].key.startsWith('workspace:')).toBe(true);
  });
});

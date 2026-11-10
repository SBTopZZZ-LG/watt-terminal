import { useEffect, useMemo, useState } from 'react';
import type { KeyboardEvent } from 'react';
import { api, errorText } from '../lib/api';
import { filterMetas, groupByDate, groupByWorkspace, relativeTime } from '../lib/historyView';
import type { ConversationMeta, HistoryGroupBy, LoadedConversation } from '../lib/types';

/** A delete waiting for its second click: one row, or the whole history. */
export type Confirm = { kind: 'row'; id: string } | { kind: 'all' };

/** The list as loaded. Opening the view always loads it again, so it is never older than the last visit. */
export type LoadState = { kind: 'loading' } | { kind: 'error'; message: string } | { kind: 'ready'; metas: ConversationMeta[]; now: number };

/** One heading and its rows. `sub` is the workspace root in workspace mode, and empty in date mode. */
export interface HistoryGroup {
  key: string;
  label: string;
  sub: string;
  items: ConversationMeta[];
}

const BUSY_TITLE = 'Wait for the agent to finish, or cancel it with Ctrl-C, first';

/** The groups for the list. The keys are prefixed by mode, so a collapsed group stays collapsed only in its own grouping. */
export function groupsFor(metas: ConversationMeta[], groupBy: HistoryGroupBy, now: number): HistoryGroup[] {
  if (groupBy === 'workspace') {
    return groupByWorkspace(metas).map((g) => ({ key: `workspace:${g.key}`, label: g.label, sub: g.root, items: g.items }));
  }
  return groupByDate(metas, now).map((g) => ({ key: `date:${g.key}`, label: g.label, sub: '', items: g.items }));
}

function messageCount(n: number): string {
  return n === 1 ? '1 message' : `${n} messages`;
}

const MONTHS_SHORT = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/** The absolute local date and time, for a row's title: "Nov 10, 2026 09:00". */
function absoluteWhen(ts: number): string {
  const d = new Date(ts * 1000);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${MONTHS_SHORT[d.getMonth()]} ${d.getDate()}, ${d.getFullYear()} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export interface HistoryContentProps {
  load: LoadState;
  query: string;
  groupBy: HistoryGroupBy;
  /** The groups of the matching conversations, already filtered and ordered. */
  groups: HistoryGroup[];
  /** The time the list is read at, for relative times. */
  now: number;
  /** The live conversation's id: its row is marked. */
  currentId: string | null;
  /** The agent is working: rows are disabled, since a conversation cannot be opened then. */
  busy: boolean;
  /** The conversation being loaded, if any. */
  opening: string | null;
  /** The reason an open or delete failed, shown above the list. */
  actionError: string | null;
  collapsed: ReadonlySet<string>;
  confirm: Confirm | null;
  onQuery(query: string): void;
  onGroupBy(groupBy: HistoryGroupBy): void;
  onToggleGroup(key: string): void;
  onConfirm(confirm: Confirm | null): void;
  onOpen(id: string): void;
  onDelete(id: string): void;
  onDeleteAll(): void;
  onRetry(): void;
  onEscape(): void;
}

/**
 * The History view's body: the filter, the grouping toggle, the groups, and the footer. Given its data, it draws it; it
 * holds no state of its own, so it renders the same in a test as in the app.
 */
export function HistoryContent(p: HistoryContentProps) {
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      e.stopPropagation();
      p.onEscape();
    }
  };
  return (
    <div className="history-view" onKeyDown={onKeyDown}>
      <div className="history-toolbar">
        <input
          type="search"
          className="history-filter"
          placeholder="Filter"
          aria-label="Filter conversations"
          value={p.query}
          onChange={(e) => p.onQuery(e.target.value)}
          autoFocus
        />
        <div className="history-toggle" role="group" aria-label="Group by">
          <button type="button" aria-pressed={p.groupBy === 'date'} onClick={() => p.onGroupBy('date')}>
            Date
          </button>
          <button type="button" aria-pressed={p.groupBy === 'workspace'} onClick={() => p.onGroupBy('workspace')}>
            Workspace
          </button>
        </div>
      </div>
      {p.actionError !== null && (
        <div className="history-state history-state-error" role="alert">
          {p.actionError}
        </div>
      )}
      <div className="history-list">
        <ListBody {...p} />
      </div>
      {p.load.kind === 'ready' && p.load.metas.length > 0 && (
        <div className="history-footer">
          {p.confirm?.kind === 'all' ? (
            <>
              <span className="history-confirm-text">Delete all conversations?</span>
              <button type="button" className="history-danger" onClick={p.onDeleteAll}>
                Yes
              </button>
              <button type="button" onClick={() => p.onConfirm(null)}>
                No
              </button>
            </>
          ) : (
            <button type="button" className="history-delete-all" onClick={() => p.onConfirm({ kind: 'all' })}>
              Delete all history
            </button>
          )}
        </div>
      )}
    </div>
  );
}

function ListBody(p: HistoryContentProps) {
  if (p.load.kind === 'loading') {
    return <div className="history-state">Loading conversations...</div>;
  }
  if (p.load.kind === 'error') {
    return (
      <div className="history-state history-state-error" role="alert">
        <span>Could not load the history: {p.load.message}</span>
        <button type="button" onClick={p.onRetry}>
          Retry
        </button>
      </div>
    );
  }
  if (p.load.metas.length === 0) {
    return <div className="history-state">No saved conversations yet.</div>;
  }
  if (p.groups.length === 0) {
    return <div className="history-state">No conversations match &quot;{p.query.trim()}&quot;.</div>;
  }
  return (
    <>
      {p.groups.map((group) => {
        const expanded = !p.collapsed.has(group.key);
        return (
          <section key={group.key} className="history-group">
            <button
              type="button"
              className="history-group-head"
              aria-expanded={expanded}
              onClick={() => p.onToggleGroup(group.key)}
            >
              <span className="history-group-label">{group.label}</span>
              {group.sub !== '' && <span className="history-root">{group.sub}</span>}
              <span className="history-count">{group.items.length}</span>
            </button>
            {expanded && (
              <ul className="history-rows">
                {group.items.map((meta) => (
                  <HistoryRow
                    key={meta.id}
                    meta={meta}
                    groupBy={p.groupBy}
                    now={p.now}
                    isCurrent={meta.id === p.currentId}
                    busy={p.busy}
                    opening={p.opening === meta.id}
                    anyOpening={p.opening !== null}
                    confirmingDelete={p.confirm?.kind === 'row' && p.confirm.id === meta.id}
                    onOpen={p.onOpen}
                    onConfirm={p.onConfirm}
                    onDelete={p.onDelete}
                  />
                ))}
              </ul>
            )}
          </section>
        );
      })}
    </>
  );
}

interface RowProps {
  meta: ConversationMeta;
  groupBy: HistoryGroupBy;
  now: number;
  isCurrent: boolean;
  busy: boolean;
  opening: boolean;
  anyOpening: boolean;
  confirmingDelete: boolean;
  onOpen(id: string): void;
  onConfirm(confirm: Confirm | null): void;
  onDelete(id: string): void;
}

function HistoryRow({ meta, groupBy, now, isCurrent, busy, opening, anyOpening, confirmingDelete, onOpen, onConfirm, onDelete }: RowProps) {
  const title = meta.title === '' ? 'Untitled conversation' : meta.title;
  const root = meta.workspace.root;
  return (
    <li className={`history-row${isCurrent ? ' is-current' : ''}`}>
      <button
        type="button"
        className="history-open"
        disabled={busy || anyOpening}
        aria-current={isCurrent ? 'true' : undefined}
        title={busy ? BUSY_TITLE : [title, root, absoluteWhen(meta.updated_at)].filter((s) => s !== '').join('\n')}
        onClick={() => onOpen(meta.id)}
      >
        <span className="history-title">{title}</span>
        <span className="history-meta">
          {groupBy === 'date' && (
            <span className="history-chip" title={root === '' ? undefined : root}>
              {meta.workspace.label}
            </span>
          )}
          <span>{messageCount(meta.message_count)}</span>
          <span>{relativeTime(meta.updated_at, now)}</span>
          {isCurrent && <span className="history-current">current</span>}
          {opening && <span className="history-opening">Opening...</span>}
        </span>
      </button>
      <span className="history-row-actions">
        {confirmingDelete ? (
          <>
            <span className="history-confirm-text">Delete?</span>
            <button type="button" className="history-danger" onClick={() => onDelete(meta.id)}>
              Yes
            </button>
            <button type="button" onClick={() => onConfirm(null)}>
              No
            </button>
          </>
        ) : (
          <button
            type="button"
            className="history-delete"
            aria-label={`Delete ${title}`}
            title="Delete this conversation"
            onClick={() => onConfirm({ kind: 'row', id: meta.id })}
          >
            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
              <path d="M3.5 3.5l9 9M12.5 3.5l-9 9" />
            </svg>
          </button>
        )}
      </span>
    </li>
  );
}

interface HistoryViewProps {
  currentId: string | null;
  busy: boolean;
  groupBy: HistoryGroupBy;
  onGroupBy(groupBy: HistoryGroupBy): void;
  /** A conversation was opened: the caller makes it the live transcript and leaves the view. */
  onOpened(loaded: LoadedConversation): void;
  onClose(): void;
}

/**
 * The History view with its state: the list (loaded on mount, so each visit refreshes it), the filter, the collapsed groups,
 * the pending delete, and the open and delete calls. Failures are shown in the view and never block the chat.
 */
export function HistoryView({ currentId, busy, groupBy, onGroupBy, onOpened, onClose }: HistoryViewProps) {
  const [load, setLoad] = useState<LoadState>({ kind: 'loading' });
  const [attempt, setAttempt] = useState(0);
  const [query, setQuery] = useState('');
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set());
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const [opening, setOpening] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api.listConversations().then(
      (metas) => {
        if (live) {
          setLoad({ kind: 'ready', metas, now: Math.floor(Date.now() / 1000) });
        }
      },
      (e: unknown) => {
        if (live) {
          setLoad({ kind: 'error', message: errorText(e) });
        }
      },
    );
    return () => {
      live = false;
    };
  }, [attempt]);

  const now = load.kind === 'ready' ? load.now : 0;
  const groups = useMemo(
    () => (load.kind === 'ready' ? groupsFor(filterMetas(load.metas, query), groupBy, load.now) : []),
    [load, query, groupBy],
  );

  const toggleGroup = (key: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(key)) {
        next.delete(key);
      } else {
        next.add(key);
      }
      return next;
    });
  };

  const open = (id: string) => {
    setOpening(id);
    setActionError(null);
    api
      .loadConversation(id)
      .then(
        (loaded) => onOpened(loaded),
        (e: unknown) => setActionError(errorText(e)),
      )
      .finally(() => setOpening(null));
  };

  const removeMetas = (keep: (m: ConversationMeta) => boolean) =>
    setLoad((s) => (s.kind === 'ready' ? { ...s, metas: s.metas.filter(keep) } : s));

  const deleteOne = (id: string) => {
    setConfirm(null);
    setActionError(null);
    api.deleteConversation(id).then(
      () => removeMetas((m) => m.id !== id),
      (e: unknown) => setActionError(errorText(e)),
    );
  };

  const deleteAll = () => {
    setConfirm(null);
    setActionError(null);
    api.deleteAllConversations().then(
      () => removeMetas(() => false),
      (e: unknown) => setActionError(errorText(e)),
    );
  };

  const retry = () => {
    setLoad({ kind: 'loading' });
    setAttempt((a) => a + 1);
  };

  // Escape first cancels a pending delete, and only then closes the view.
  const escape = () => {
    if (confirm !== null) {
      setConfirm(null);
    } else {
      onClose();
    }
  };

  return (
    <HistoryContent
      load={load}
      query={query}
      groupBy={groupBy}
      groups={groups}
      now={now}
      currentId={currentId}
      busy={busy}
      opening={opening}
      actionError={actionError}
      collapsed={collapsed}
      confirm={confirm}
      onQuery={setQuery}
      onGroupBy={onGroupBy}
      onToggleGroup={toggleGroup}
      onConfirm={setConfirm}
      onOpen={open}
      onDelete={deleteOne}
      onDeleteAll={deleteAll}
      onRetry={retry}
      onEscape={escape}
    />
  );
}

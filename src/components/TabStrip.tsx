import { useEffect, useRef, useSyncExternalStore } from 'react';
import type { KeyboardEvent, MouseEvent, WheelEvent } from 'react';
import { CHAT_ID } from '../lib/api';
import { effectiveState } from '../lib/inputState';
import { nextTabId } from '../lib/tabKeys';
import { tabs } from '../lib/tabManager';
import type { TabController } from '../lib/tab';
import { pickTabTitle } from '../lib/tabState';
import { isOwnedByChat, TAB_LIMIT_MESSAGE, tabLimitReached } from '../lib/tabStrip';

/** A click or a press on a tab, the "+" or a "×" keeps the focus where it was: the terminal keeps the keys. */
function keepFocus(e: MouseEvent): void {
  e.preventDefault();
}

function focusTab(id: string): void {
  document.querySelector<HTMLElement>(`[data-tab-id="${id}"]`)?.focus();
}

function newTab(): void {
  void tabs.create().then((tab) => {
    if (tab !== null) {
      void tab.fitToContainer();
      tab.focus();
    }
  });
}

function closeTab(id: string): void {
  void tabs.close(id).then((closed) => {
    if (closed) {
      tabs.active?.focus();
    }
  });
}

interface TabButtonProps {
  tab: TabController;
  selected: boolean;
  canClose: boolean;
  onKeyDown: (e: KeyboardEvent<HTMLButtonElement>, id: string) => void;
}

function TabButton({ tab, selected, canClose, onKeyDown }: TabButtonProps) {
  const s = useSyncExternalStore(tab.subscribe, tab.getSnapshot);
  const label = pickTabTitle({ xtermTitle: s.title, foreground: s.input.foreground, cwd: s.cwd, index: tabs.ordinal(tab.id) });
  const ended = effectiveState(s.input) === 'exited';
  const visible = isOwnedByChat(s.owner, CHAT_ID);
  const cls = ['term-tab', selected ? 'is-active' : '', ended ? 'is-ended' : ''].filter((c) => c !== '').join(' ');
  return (
    <div className="term-tab-cell">
      <button
        type="button"
        role="tab"
        aria-selected={selected}
        tabIndex={selected ? 0 : -1}
        data-tab-id={tab.id}
        className={cls}
        title={visible ? `${label} (visible to the agent)` : label}
        onMouseDown={keepFocus}
        onClick={() => {
          tabs.setActive(tab.id);
          tab.focus();
        }}
        onKeyDown={(e) => onKeyDown(e, tab.id)}
      >
        {visible && <span className="term-tab-dot" aria-hidden="true" />}
        <span className="term-tab-label">{label}</span>
        {ended && <span className="term-tab-ended">ended</span>}
      </button>
      {canClose && (
        <button
          type="button"
          className="term-tab-close"
          aria-label={`Close ${label}`}
          data-tip="Close terminal"
          onMouseDown={keepFocus}
          onClick={() => closeTab(tab.id)}
        >
          ×
        </button>
      )}
    </div>
  );
}

/**
 * The terminal tabs, above the terminals. Each tab shows its title. A dot marks a terminal the chat can see; an ended one is
 * dimmed. "+" makes a terminal (aria-disabled at the limit). Arrow keys, Home and End move between tabs, as the WAI-ARIA tabs
 * pattern gives them. A vertical wheel scrolls the row sideways.
 */
export function TabStrip() {
  const { ids, activeId } = useSyncExternalStore(tabs.subscribeStore, tabs.getSnapshot);
  const scroller = useRef<HTMLDivElement>(null);
  const full = tabLimitReached(ids.length);

  // The active tab is kept in view when it changes: a new tab, a switch, a key move.
  useEffect(() => {
    scroller.current?.querySelector<HTMLElement>('[aria-selected="true"]')?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' });
  }, [activeId]);

  const onWheel = (e: WheelEvent<HTMLDivElement>) => {
    if (scroller.current !== null && Math.abs(e.deltaY) >= Math.abs(e.deltaX)) {
      scroller.current.scrollLeft += e.deltaY;
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>, id: string) => {
    const next = nextTabId(ids, id, e.key);
    if (next === null) {
      return;
    }
    e.preventDefault();
    tabs.setActive(next);
    focusTab(next);
  };

  return (
    <div className="term-tabs">
      <div className="term-tabs-scroll" role="tablist" aria-label="Terminals" ref={scroller} onWheel={onWheel}>
        {ids.map((id) => {
          const tab = tabs.get(id);
          return tab === undefined ? null : (
            <TabButton key={id} tab={tab} selected={id === activeId} canClose={ids.length > 1} onKeyDown={onKeyDown} />
          );
        })}
      </div>
      <button
        type="button"
        className={`term-tab-add${full ? ' is-disabled' : ''}`}
        aria-label="New terminal"
        aria-disabled={full}
        data-tip={full ? TAB_LIMIT_MESSAGE : 'New terminal'}
        onMouseDown={keepFocus}
        onClick={() => {
          if (!full) {
            newTab();
          }
        }}
      >
        +
      </button>
    </div>
  );
}

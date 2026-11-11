import { useId, useRef } from 'react';
import type { KeyboardEvent, ReactNode } from 'react';
import { nextTabId } from '../lib/tabKeys';

export interface TabDef {
  id: string;
  label: string;
}

export interface SettingsTabsProps {
  /** The tabs, in order. Adding a tab is one entry here and a panel in `panel`. */
  tabs: readonly TabDef[];
  /** The id of the active tab. */
  selected: string;
  /** The ids of the tabs whose draft has unsaved changes. Each shows a dot, with "unsaved changes" for screen readers. */
  dirty: readonly string[];
  /** The accessible name of the tab list. */
  label: string;
  onSelect(id: string): void;
  /** The content of a tab. Every panel stays mounted; the ones not selected are hidden, so their drafts survive a switch. */
  panel(id: string): ReactNode;
}

/**
 * A row of tabs and their panels, following the WAI-ARIA tabs pattern. Only the selected tab is in the Tab order (roving
 * tabindex). Arrow keys move between tabs and wrap, Home and End go to the ends, and a tab is selected as it gets focus.
 * The panels are all in the DOM, so a hidden panel keeps its state. The strip and the panels are one column; the caller
 * places anything that must stay in view above or below them.
 */
export function SettingsTabs(p: SettingsTabsProps) {
  const uid = useId();
  const buttons = useRef(new Map<string, HTMLButtonElement>());
  const ids = p.tabs.map((t) => t.id);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const next = nextTabId(ids, p.selected, e.key);
    if (next === null) {
      return;
    }
    e.preventDefault();
    p.onSelect(next);
    buttons.current.get(next)?.focus();
  };

  return (
    <div className="tabs">
      <div role="tablist" aria-label={p.label} className="tab-strip" onKeyDown={onKeyDown}>
        {p.tabs.map((t) => {
          const active = t.id === p.selected;
          const dirty = p.dirty.includes(t.id);
          return (
            <button
              key={t.id}
              ref={(el) => {
                if (el) {
                  buttons.current.set(t.id, el);
                } else {
                  buttons.current.delete(t.id);
                }
              }}
              type="button"
              role="tab"
              id={`${uid}-tab-${t.id}`}
              aria-selected={active}
              aria-controls={`${uid}-panel-${t.id}`}
              tabIndex={active ? 0 : -1}
              className={`tab${active ? ' is-active' : ''}`}
              onClick={() => p.onSelect(t.id)}
            >
              <span className="tab-label">{t.label}</span>
              {dirty && <span className="tab-dot" aria-hidden="true" />}
              {dirty && <span className="tab-unsaved"> unsaved changes</span>}
            </button>
          );
        })}
      </div>
      {p.tabs.map((t) => (
        <div
          key={t.id}
          role="tabpanel"
          id={`${uid}-panel-${t.id}`}
          aria-labelledby={`${uid}-tab-${t.id}`}
          className="tab-panel"
          hidden={t.id !== p.selected}
        >
          {p.panel(t.id)}
        </div>
      ))}
    </div>
  );
}

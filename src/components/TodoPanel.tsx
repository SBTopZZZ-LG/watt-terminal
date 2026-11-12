import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import type { TodoItem } from '../lib/types';
import { currentIndex, titleText, todoRows, todoSummary } from '../lib/todoView';
import { listMaxHeight, revealIndex, scrollTopToReveal, shouldExpand, todoShape, TODO_LIST_MAX_PX } from '../lib/todoPanel';
import { settleTight, TIGHT_ATTR } from '../lib/panelTight';

interface Props {
  items: TodoItem[];
}

/** How far the rows may overflow the list's box before it scrolls, in px: above sub-pixel rounding, far below a row. */
const SCROLL_THRESHOLD_PX = 1.5;

/** The user's choice, and the list shape it was made for. A different shape (a new list, an added item) makes it stale. */
interface Choice {
  shape: string;
  expanded: boolean;
}

/**
 * The conversation's todo list, pinned between the transcript and the context meter (a sibling of the transcript, so it never
 * scrolls away with the body). One line when collapsed: the count and the current task. Expanded, the whole list in its own
 * order, with the current item marked.
 *
 * The list expands while any item is unfinished and collapses once every item is done. A click on the toggle is the user's
 * choice, and it holds until the list changes shape. Capped at 40% of the agent panel (180px at most) and scrolled inside that
 * cap: after an update the item that just changed comes into view.
 *
 * Hidden when the list is empty. Content is plain text: React escapes it, and it is cleaned by todoView.
 */
export function TodoPanel({ items }: Props) {
  const shape = todoShape(items);
  const [choice, setChoice] = useState<Choice | null>(null);
  const [cap, setCap] = useState(TODO_LIST_MAX_PX);
  const sectionRef = useRef<HTMLElement>(null);
  const listRef = useRef<HTMLOListElement>(null);
  const shownItems = useRef<TodoItem[] | null>(null);
  const listId = useId();
  const hasItems = items.length > 0;

  // A changed shape ends the user's choice: the list decides again. Reset during render, so no stale choice is ever applied.
  const stale = choice !== null && choice.shape !== shape;
  if (stale) {
    setChoice(null);
  }
  const manual = choice !== null && !stale ? choice.expanded : null;
  const expanded = shouldExpand(items, manual);

  // The agent panel's tight flag (see panelTight.ts) is settled here after every render, and a frame after the list's or the panel's
  // size changes without a render. The flag is set by the agent panel's overflow, so the transcript settles it too. The panel is
  // remembered, so the flag is cleared when this panel goes away (the list emptied, or the view changed).
  const agentRef = useRef<HTMLElement | null>(null);
  const settleAgent = useCallback(() => {
    const agent = sectionRef.current?.closest<HTMLElement>('.agent-panel') ?? agentRef.current;
    agentRef.current = agent;
    settleTight(agent);
  }, []);
  const settleLater = useCallback(() => {
    requestAnimationFrame(settleAgent);
  }, [settleAgent]);
  // After every render: the sizes the render gave are settled here, before paint.
  useLayoutEffect(() => {
    settleAgent();
  });
  useLayoutEffect(
    () => () => {
      agentRef.current?.removeAttribute(TIGHT_ATTR);
    },
    [],
  );

  // The cap follows the agent panel's height: 40% of it, or 180px, whichever is smaller.
  useLayoutEffect(() => {
    const agent = sectionRef.current?.closest<HTMLElement>('.agent-panel');
    if (!agent || typeof ResizeObserver === 'undefined') {
      return;
    }
    const measure = () => {
      setCap(listMaxHeight(agent.clientHeight));
      settleLater();
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(agent);
    return () => observer.disconnect();
  }, [hasItems, settleLater]);

  // The item to bring into view after an update, waiting for the list's settled size (see revealPending). Null when none is waiting.
  const pendingReveal = useRef<number | null>(null);

  // The reveal runs once the list has its final height: the cap the agent panel gives now is applied to the list. On arrival the
  // first render has the 180px initial cap, so the reveal waits for the measured one. A user who scrolls the list drops the mark.
  const revealPending = useCallback(() => {
    const list = listRef.current;
    const index = pendingReveal.current;
    if (!list || index === null) {
      return;
    }
    const agent = sectionRef.current?.closest<HTMLElement>('.agent-panel');
    if (agent && parseFloat(getComputedStyle(list).maxHeight) !== listMaxHeight(agent.clientHeight)) {
      return;
    }
    pendingReveal.current = null;
    const row = index >= 0 ? (list.children[index] as HTMLElement | undefined) : undefined;
    if (list.hidden || !row || list.scrollHeight <= list.clientHeight) {
      return;
    }
    const top = scrollTopToReveal(row.offsetTop, row.offsetHeight, list.clientHeight, list.scrollTop);
    if (top !== list.scrollTop) {
      list.scrollTo({ top, behavior: prefersReducedMotion() ? 'auto' : 'smooth' });
    }
  }, []);

  /** The user scrolls the list themselves: a pending reveal is dropped rather than fought. */
  const dropPendingReveal = () => {
    pendingReveal.current = null;
  };

  // The list scrolls only when its rows are taller than its box. Overflow is hidden until that is measured, so a list that fits
  // never shows a bar. The threshold is above sub-pixel rounding. Re-checked after each change that can move the rows (items, the
  // cap, the expand state) and when the list's own box changes (a resize, the list showing). The class changes overflow only.
  // A check that settles the list's height also runs the pending reveal.
  const [scrollable, setScrollable] = useState(false);
  useLayoutEffect(() => {
    const list = listRef.current;
    if (!list) {
      return;
    }
    const check = () => {
      setScrollable(list.scrollHeight - list.clientHeight > SCROLL_THRESHOLD_PX);
      revealPending();
    };
    check();
    if (typeof ResizeObserver === 'undefined') {
      return;
    }
    const observer = new ResizeObserver(() => {
      check();
      settleLater();
    });
    observer.observe(list);
    return () => observer.disconnect();
  }, [items, cap, expanded, hasItems, revealPending, settleLater]);

  // After an update, the item that changed is in view. Runs on the list's changes only, not on a toggle. The reveal itself waits
  // for the list's settled height (see revealPending), so it may run in a later render.
  useEffect(() => {
    const prev = shownItems.current;
    shownItems.current = items;
    pendingReveal.current = revealIndex(prev, items);
    revealPending();
  }, [items, revealPending]); // Only the list's changes move the scroll. `expanded` is read from the list's `hidden` attribute.

  if (!hasItems) {
    return null;
  }
  const summary = todoSummary(items);
  const current = currentIndex(items);
  const currentTitle = current >= 0 ? titleText(items[current].content) : undefined;
  return (
    <section
      ref={sectionRef}
      className={`todo-panel${expanded ? ' expanded' : ''}${current < 0 ? ' all-done' : ''}${items.length > 1 ? ' multi' : ''}`}
      aria-label="Todo list"
    >
      <button
        type="button"
        className="todo-toggle"
        aria-expanded={expanded}
        aria-controls={listId}
        onClick={() => setChoice({ shape, expanded: !expanded })}
        title={currentTitle !== undefined ? `${expanded ? 'Hide' : 'Show'} the todo list. Current: ${currentTitle}` : `${expanded ? 'Hide' : 'Show'} the todo list`}
      >
        <span className="todo-summary">{summary}</span>
        <svg className="todo-chevron" viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
          <path d="M4 6l4 4 4-4" />
        </svg>
      </button>
      <ol
        id={listId}
        ref={listRef}
        className={`todo-list${scrollable ? ' is-scrollable' : ''}`}
        hidden={!expanded}
        style={{ maxHeight: cap }}
        onWheel={dropPendingReveal}
        onPointerDown={dropPendingReveal}
      >
        {todoRows(items).map((row) => (
          <li
            key={row.index}
            className={`todo-item todo-${row.status}${row.current ? ' todo-current' : ''}`}
            aria-current={row.current ? 'step' : undefined}
          >
            <span className="todo-mark" aria-hidden="true" />
            <span className="todo-sr-only">{statusWord(row.status)}: </span>
            <span className="todo-text">{row.text}</span>
          </li>
        ))}
      </ol>
    </section>
  );
}

/** Reduced motion: the list jumps instead of scrolling smoothly. */
function prefersReducedMotion(): boolean {
  return typeof window.matchMedia === 'function' && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

/** The status as a word for screen readers. The marks are drawn in CSS and carry no text. */
function statusWord(status: TodoItem['status']): string {
  switch (status) {
    case 'completed':
      return 'Done';
    case 'in_progress':
      return 'In progress';
    default:
      return 'Pending';
  }
}

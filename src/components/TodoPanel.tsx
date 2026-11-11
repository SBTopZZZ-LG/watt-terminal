import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import type { TodoItem } from '../lib/types';
import { currentIndex, titleText, todoRows, todoSummary } from '../lib/todoView';
import { listMaxHeight, revealIndex, scrollTopToReveal, shouldExpand, todoShape, TODO_LIST_MAX_PX } from '../lib/todoPanel';

interface Props {
  items: TodoItem[];
}

/** The user's choice, and the list shape it was made for. A different shape (a new list, an added item) makes it stale. */
interface Choice {
  shape: string;
  expanded: boolean;
}

/**
 * The conversation's todo list, pinned above the transcript body (not inside it, so it never scrolls away). One line when
 * collapsed: the count and the current task. Expanded, the whole list in its own order, with the current item marked.
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

  // The cap follows the agent panel's height: 40% of it, or 180px, whichever is smaller.
  useLayoutEffect(() => {
    const agent = sectionRef.current?.closest<HTMLElement>('.agent-panel');
    if (!agent || typeof ResizeObserver === 'undefined') {
      return;
    }
    const measure = () => setCap(listMaxHeight(agent.clientHeight));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(agent);
    return () => observer.disconnect();
  }, [hasItems]);

  // After an update, the item that changed is in view. Runs on the list's changes only, not on a toggle.
  useEffect(() => {
    const prev = shownItems.current;
    shownItems.current = items;
    const list = listRef.current;
    if (!list || !expanded) {
      return;
    }
    const target = revealIndex(prev, items);
    const row = target >= 0 ? (list.children[target] as HTMLElement | undefined) : undefined;
    if (!row || list.scrollHeight <= list.clientHeight) {
      return;
    }
    const top = scrollTopToReveal(row.offsetTop, row.offsetHeight, list.clientHeight, list.scrollTop);
    if (top !== list.scrollTop) {
      list.scrollTo({ top, behavior: prefersReducedMotion() ? 'auto' : 'smooth' });
    }
  }, [items]); // Only the list's changes move the scroll. `expanded` is read as of the render that changed the list.

  if (!hasItems) {
    return null;
  }
  const summary = todoSummary(items);
  const current = currentIndex(items);
  const currentTitle = current >= 0 ? titleText(items[current].content) : undefined;
  return (
    <section
      ref={sectionRef}
      className={`todo-panel${expanded ? ' expanded' : ''}${current < 0 ? ' all-done' : ''}`}
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
      <ol id={listId} ref={listRef} className="todo-list" hidden={!expanded} style={{ maxHeight: cap }}>
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

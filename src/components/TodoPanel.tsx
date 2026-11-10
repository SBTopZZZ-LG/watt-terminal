import { useId, useState } from 'react';
import type { TodoItem } from '../lib/types';
import { currentIndex, titleText, todoRows, todoSummary } from '../lib/todoView';

interface Props {
  items: TodoItem[];
  /** Whether the list starts expanded. The panel is collapsed by default. */
  defaultExpanded?: boolean;
}

/**
 * The conversation's todo list, pinned above the transcript body (not inside it, so it never scrolls away). One line when
 * collapsed: the count and the current task. Expanded, the whole list in its own order, with the current item marked.
 * Hidden when the list is empty. Content is plain text: React escapes it, and it is cleaned by todoView.
 */
export function TodoPanel({ items, defaultExpanded = false }: Props) {
  const [expanded, setExpanded] = useState(defaultExpanded);
  const listId = useId();
  if (items.length === 0) {
    return null;
  }
  const summary = todoSummary(items);
  const current = currentIndex(items);
  const currentTitle = current >= 0 ? titleText(items[current].content) : undefined;
  return (
    <section className={`todo-panel${expanded ? ' expanded' : ''}${current < 0 ? ' all-done' : ''}`} aria-label="Todo list">
      <button
        type="button"
        className="todo-toggle"
        aria-expanded={expanded}
        aria-controls={listId}
        onClick={() => setExpanded((e) => !e)}
        title={currentTitle !== undefined ? `${expanded ? 'Hide' : 'Show'} the todo list. Current: ${currentTitle}` : `${expanded ? 'Hide' : 'Show'} the todo list`}
      >
        <span className="todo-summary">{summary}</span>
        <svg className="todo-chevron" viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
          <path d="M4 6l4 4 4-4" />
        </svg>
      </button>
      <ol id={listId} className="todo-list" hidden={!expanded}>
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

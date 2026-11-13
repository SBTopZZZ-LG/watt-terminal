import { useEffect, useRef } from 'react';
import type { CommandMenuItem } from '../lib/commandMenu';
import { scrollTopToReveal } from '../lib/todoPanel';

/** The menu's id, as the bar's textarea names it in aria-controls. Constant, so it is stable across renders. */
export const COMMAND_MENU_ID = 'command-menu';

/** The id of a row, as the textarea names it in aria-activedescendant. Index-based, so it is stable for a list. */
export function commandMenuItemId(index: number): string {
  return `command-menu-item-${index}`;
}

interface Props {
  items: CommandMenuItem[];
  /** The selected row's index into `items`. */
  index: number;
  onAccept: (item: CommandMenuItem) => void;
  onHover: (index: number) => void;
}

/**
 * The slash-command autocomplete menu. Presentational: the rows, which one is selected, and the mouse
 * handling. The bar owns the filtering, the selection and the keyboard. A mousedown never takes focus
 * (it is prevented, as for the bar's buttons), and a click accepts the row.
 */
export function CommandMenu({ items, index, onAccept, onHover }: Props) {
  const menuRef = useRef<HTMLDivElement | null>(null);

  // Keeps the keyboard selection in view as it moves, setting the menu's own scrollTop (never
  // scrollIntoView, which can scroll ancestors).
  useEffect(() => {
    const menu = menuRef.current;
    if (menu === null) {
      return;
    }
    const row = menu.children[index] as HTMLElement | undefined;
    if (row === undefined) {
      return;
    }
    const top = scrollTopToReveal(row.offsetTop, row.offsetHeight, menu.clientHeight, menu.scrollTop);
    if (top !== menu.scrollTop) {
      menu.scrollTop = top;
    }
  }, [index]);

  return (
    <div ref={menuRef} id={COMMAND_MENU_ID} className="command-menu" role="listbox" aria-label="Slash commands">
      {items.map((item, i) => (
        <div
          key={item.source === null ? `builtin-${item.name}` : `${item.source}-${item.name}`}
          id={commandMenuItemId(i)}
          role="option"
          aria-selected={i === index}
          className={i === index ? 'command-menu-item is-selected' : 'command-menu-item'}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => onAccept(item)}
          onMouseEnter={() => onHover(i)}
        >
          <span className="command-menu-name">/{item.name}</span>
          {item.hint !== null && <span className="command-menu-hint">{item.hint}</span>}
          <span className="command-menu-description">{item.description}</span>
          {item.source !== null && <span className="command-menu-source">{item.source}</span>}
        </div>
      ))}
    </div>
  );
}

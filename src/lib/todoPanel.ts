// The todo panel's behaviour as plain rules, so they are tested without the DOM: when the list is expanded, how big the list
// may get, and which item to bring into view after an update. TodoPanel.tsx applies them.

import type { TodoItem } from './types';

/** The list's cap, in px, however tall the agent panel is. */
export const TODO_LIST_MAX_PX = 180;

/** The list's cap as a share of the agent panel's height, when that is smaller than TODO_LIST_MAX_PX. */
export const TODO_LIST_MAX_SHARE = 0.4;

/** True while some item is not completed. An empty list has none. */
export function hasUnfinished(items: TodoItem[]): boolean {
  return items.some((item) => item.status !== 'completed');
}

/**
 * The list's shape: its tasks in order, not their statuses. A status update keeps the shape. A new list or an added or removed
 * item changes it.
 */
export function todoShape(items: TodoItem[]): string {
  return JSON.stringify(items.map((item) => item.content));
}

/**
 * Whether the panel is expanded. A choice the user made (`manual`, true or false) holds; without one the list decides: expanded
 * while any item is unfinished, collapsed once every item is done.
 */
export function shouldExpand(items: TodoItem[], manual: boolean | null): boolean {
  return manual ?? hasUnfinished(items);
}

/**
 * The index of the item to bring into view after an update. When the shape is unchanged, items line up by position, so the last
 * item whose status changed to done or in progress wins. Otherwise (a new list, an added item, or the first render, with no
 * previous list) the first unfinished item. -1 when every item is done.
 */
export function revealIndex(prev: TodoItem[] | null, next: TodoItem[]): number {
  if (prev !== null && todoShape(prev) === todoShape(next)) {
    for (let i = next.length - 1; i >= 0; i -= 1) {
      const status = next[i].status;
      if (status !== prev[i].status && (status === 'completed' || status === 'in_progress')) {
        return i;
      }
    }
  }
  return next.findIndex((item) => item.status !== 'completed');
}

/**
 * The scrollTop that brings a row (its top and height, in the list's coordinates) into a view of `viewHeight`, moving as little
 * as possible. A row taller than the view shows its top.
 */
export function scrollTopToReveal(rowTop: number, rowHeight: number, viewHeight: number, scrollTop: number): number {
  if (rowTop < scrollTop || rowHeight > viewHeight) {
    return rowTop;
  }
  if (rowTop + rowHeight > scrollTop + viewHeight) {
    return rowTop + rowHeight - viewHeight;
  }
  return scrollTop;
}

/** The list's max height for an agent panel of `agentHeight` px: 40% of it, or 180px, whichever is smaller. */
export function listMaxHeight(agentHeight: number): number {
  if (agentHeight <= 0) {
    // Not laid out yet: keep the cap, and the resize observer sets the real one.
    return TODO_LIST_MAX_PX;
  }
  return Math.floor(Math.min(TODO_LIST_MAX_PX, TODO_LIST_MAX_SHARE * agentHeight));
}

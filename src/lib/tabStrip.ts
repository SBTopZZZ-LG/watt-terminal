// The rules of the tab strip, as pure functions: the tab limit, the ids of new tabs, which tab takes over when one closes, and
// which tabs the chat owns. The store (tabManager.ts) and the strip (TabStrip.tsx) call these.

/** The most terminals the page holds. "+" is disabled at this count. */
export const TAB_LIMIT = 20;

export const TAB_LIMIT_MESSAGE = `Tab limit reached (${TAB_LIMIT})`;

export function tabLimitReached(count: number): boolean {
  return count >= TAB_LIMIT;
}

/**
 * The next free id `t<N>` from `counter` on, skipping ids already taken (a reload keeps the backend's terminals, and the
 * counter starts again). Returns the id and the counter to pass next time.
 */
export function allocTabId(counter: number, taken: ReadonlySet<string>): { id: string; counter: number } {
  let n = counter;
  while (taken.has(`t${n}`)) {
    n += 1;
  }
  return { id: `t${n}`, counter: n + 1 };
}

/**
 * The tab that becomes active when `closedId` closes: its right neighbour, else its left one. Null when no other tab is
 * left, or the id is unknown.
 */
export function neighbourAfterClose(ids: readonly string[], closedId: string): string | null {
  const i = ids.indexOf(closedId);
  if (i === -1) {
    return null;
  }
  return ids[i + 1] ?? ids[i - 1] ?? null;
}

/** Whether a terminal with this owner (list_tabs) belongs to the chat. An unassigned terminal (null) belongs to none. */
export function isOwnedByChat(owner: string | null, chatId: string): boolean {
  return owner === chatId;
}

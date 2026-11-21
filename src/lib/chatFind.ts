// Chat find: the matches of a query over a list of text strings (the transcript's text nodes, in document order). Pure, so
// the matching, the wrap and the limit are tested without the DOM. chatFindDom.ts maps the offsets back to Ranges.
// A match lies within one text: a phrase split across two nodes (say, by emphasis) is not found.

import { HIGHLIGHT_LIMIT } from './find';

/** The most matches kept, the same cap as the terminal find. Past it the search stops and the label says "1000+ matches". */
export const CHAT_FIND_LIMIT = HIGHLIGHT_LIMIT;

export interface ChatMatch {
  /** Index into the texts the search ran over. */
  nodeIndex: number;
  /** Offsets in that text, start inclusive and end exclusive. */
  start: number;
  end: number;
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * Every match of `query` in `texts`, in order, not overlapping within a text. Case-insensitive unless `caseSensitive`.
 * An empty query has no matches. At most `limit` matches are returned.
 */
export function findChatMatches(
  texts: readonly string[],
  query: string,
  caseSensitive: boolean,
  limit: number = CHAT_FIND_LIMIT,
): ChatMatch[] {
  if (query === '') {
    return [];
  }
  const pattern = new RegExp(escapeRegExp(query), caseSensitive ? 'gu' : 'giu');
  const matches: ChatMatch[] = [];
  for (let nodeIndex = 0; nodeIndex < texts.length; nodeIndex++) {
    for (const m of texts[nodeIndex].matchAll(pattern)) {
      if (matches.length >= limit) {
        return matches;
      }
      const start = m.index ?? 0;
      matches.push({ nodeIndex, start, end: start + m[0].length });
    }
  }
  return matches;
}

/** The match after (next) or before (prev) `current`, wrapping. From no current match, next is the first and prev the last. -1 when there are none. */
export function stepMatch(current: number, count: number, direction: 'next' | 'prev'): number {
  if (count <= 0) {
    return -1;
  }
  if (current < 0 || current >= count) {
    return direction === 'next' ? 0 : count - 1;
  }
  return direction === 'next' ? (current + 1) % count : (current - 1 + count) % count;
}

/** The current match to keep when the matches are searched again: the same index while it exists, else the last one. First when there was none. */
export function keepMatch(current: number, count: number): number {
  if (count <= 0) {
    return -1;
  }
  return current < 0 ? 0 : Math.min(current, count - 1);
}

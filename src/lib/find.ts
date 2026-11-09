// The find bar's label. Pure, so the wording and the edge cases are tested without xterm or the DOM.

/** The addon highlights at most this many matches (its highlightLimit). Past it the result index is -1. */
export const HIGHLIGHT_LIMIT = 1000;

export interface FindResults {
  /** Index of the selected match, or -1 when none is selected (or the matches are past HIGHLIGHT_LIMIT). */
  index: number;
  count: number;
}

/** The text shown next to the find input. Empty when there is no query. */
export function findLabel(query: string, results: FindResults): string {
  if (query === '') {
    return '';
  }
  if (results.count === 0) {
    return 'No results';
  }
  if (results.count >= HIGHLIGHT_LIMIT) {
    return `${HIGHLIGHT_LIMIT}+ matches`;
  }
  if (results.index < 0) {
    return `${results.count} matches`;
  }
  return `${results.index + 1} of ${results.count}`;
}

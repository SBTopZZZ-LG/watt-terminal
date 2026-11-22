// Chat find, DOM side: the text nodes to search, their Ranges, the highlights and the reveal of a match. The DOM is not
// unit-tested (no DOM in vitest); the pure matching is in chatFind.ts, and the pure selection decision is shouldSelectMatch.

import type { ChatMatch } from './chatFind';

/** Text under these is not searched: controls, hidden panels (the tool tabs not selected), decorations, and form fields. */
const SKIP_SELECTOR = 'button, [aria-hidden="true"], [hidden], .md-code-copy, script, style, textarea, select';

/** The CSS Custom Highlight names: every match, and the current one (drawn over the others). Styled in App.css. */
export const ALL_MATCHES_HIGHLIGHT = 'chat-find';
export const CURRENT_MATCH_HIGHLIGHT = 'chat-find-current';

/** The text nodes under `root` that a search reads, in document order. */
export function searchableTextNodes(root: Element): Text[] {
  const walker = root.ownerDocument.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const nodes: Text[] = [];
  for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
    const parent = node.parentElement;
    if (parent !== null && parent.closest(SKIP_SELECTOR) === null) {
      nodes.push(node as Text);
    }
  }
  return nodes;
}

/** A Range per match. `nodes` is the list the matches were found in (their nodeIndex indexes it). */
export function rangesFor(nodes: readonly Text[], matches: readonly ChatMatch[]): Range[] {
  return matches.map((m) => {
    const node = nodes[m.nodeIndex];
    const range = node.ownerDocument.createRange();
    range.setStart(node, m.start);
    range.setEnd(node, m.end);
    return range;
  });
}

/** The CSS Custom Highlight API: Chromium 105+ (WebView2) and Safari 17.2+ (WKWebView). Feature-detected, not assumed. */
export function highlightsSupported(): boolean {
  return typeof CSS !== 'undefined' && 'highlights' in CSS && typeof Highlight !== 'undefined';
}

/** Paints every range, and the one at `current` (an index, or -1 for none) as the current match. No-op without the API. */
export function paintHighlights(ranges: readonly Range[], current: number): void {
  if (!highlightsSupported()) {
    return;
  }
  CSS.highlights.set(ALL_MATCHES_HIGHLIGHT, new Highlight(...ranges));
  const now = ranges[current];
  if (now === undefined) {
    CSS.highlights.delete(CURRENT_MATCH_HIGHLIGHT);
  } else {
    CSS.highlights.set(CURRENT_MATCH_HIGHLIGHT, new Highlight(now));
  }
}

/** Removes both highlights. No-op without the API. */
export function clearHighlights(): void {
  if (!highlightsSupported()) {
    return;
  }
  CSS.highlights.delete(ALL_MATCHES_HIGHLIGHT);
  CSS.highlights.delete(CURRENT_MATCH_HIGHLIGHT);
}

/**
 * Whether the current match gets the document selection (selectRange). In WebView2/Chromium a selection in non-editable
 * text takes focus from a focused input, so while the find input has focus the highlight alone marks the match. Without the
 * highlight API the selection is the only mark, so it is made anyway (the caller then gives the input its focus back).
 */
export function shouldSelectMatch(state: { inputFocused: boolean; highlightsSupported: boolean }): boolean {
  return !state.inputFocused || !state.highlightsSupported;
}

/** Makes the range's text the document selection. The current match also has its highlight, so it shows without this. */
export function selectRange(range: Range): void {
  const selection = document.getSelection();
  if (selection === null) {
    return;
  }
  selection.removeAllRanges();
  selection.addRange(range);
}

/** Opens the closed <details> around a range (tool rows), then scrolls its element to the centre of the view. */
export function revealRange(range: Range): void {
  const el = range.startContainer.parentElement;
  if (el === null) {
    return;
  }
  for (let details = el.closest('details'); details !== null; details = details.parentElement?.closest('details') ?? null) {
    details.open = true;
  }
  el.scrollIntoView({ block: 'center' });
}

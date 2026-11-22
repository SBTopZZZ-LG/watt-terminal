import { useEffect, useRef, useState } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent, MouseEvent as ReactMouseEvent } from 'react';
import { findChatMatches, keepMatch, stepMatch } from '../lib/chatFind';
import type { ChatMatch } from '../lib/chatFind';
import {
  clearHighlights,
  highlightsSupported,
  paintHighlights,
  rangesFor,
  revealRange,
  searchableTextNodes,
  selectRange,
  shouldSelectMatch,
} from '../lib/chatFindDom';
import { findLabel } from '../lib/find';
import type { FindResults } from '../lib/find';

const TRANSCRIPT = '.transcript';
const TRANSCRIPT_BODY = '.transcript-body';
/** A transcript change re-runs the search after this quiet time, so streaming text does not search per token. */
const RERUN_DELAY_MS = 150;

/** What to do with the current match: keep it (after a transcript change), take the first (a new query), or step. */
type Move = { kind: 'keep' } | { kind: 'first' } | { kind: 'step'; direction: 'next' | 'prev' };

interface Props {
  /** Bumped by the parent (Ctrl+F in the chat) to focus and select the input, also when the bar is already open. */
  focusToken: number;
  /** Esc, or the close button. The parent unmounts the bar, which clears the highlights. */
  onClose: () => void;
}

function elementOf(node: Node): Element | null {
  return node instanceof Element ? node : node.parentElement;
}

/**
 * The find bar over the agent transcript, at the top of the agent panel. The matches are the transcript's text (see
 * chatFindDom.ts): all are highlighted, and the current one is also scrolled to and, unless the input has focus, selected. Typing searches as you go,
 * Enter and Shift+Enter step, Esc closes. A change in the transcript searches again, and keeps the current match.
 */
export function ChatFindBar({ focusToken, onClose }: Props) {
  const [query, setQuery] = useState('');
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [results, setResults] = useState<FindResults>({ index: -1, count: 0 });
  const inputRef = useRef<HTMLInputElement>(null);
  const barRef = useRef<HTMLDivElement>(null);
  // Read by the search, which the transcript observer also calls: the handlers keep them current before a search.
  const latest = useRef({ query: '', caseSensitive: false });
  const matchRanges = useRef<Range[]>([]);
  const current = useRef(-1);

  function search(move: Move) {
    const { query: text, caseSensitive: sensitive } = latest.current;
    const body = document.querySelector<HTMLElement>(TRANSCRIPT_BODY);
    const nodes = body === null ? [] : searchableTextNodes(body);
    const matches: ChatMatch[] = findChatMatches(
      nodes.map((n) => n.data),
      text,
      sensitive,
    );
    const count = matches.length;
    let index: number;
    switch (move.kind) {
      case 'keep':
        index = keepMatch(current.current, count);
        break;
      case 'first':
        index = stepMatch(-1, count, 'next');
        break;
      case 'step':
        index = stepMatch(current.current, count, move.direction);
        break;
    }
    const ranges = rangesFor(nodes, matches);
    matchRanges.current = ranges;
    current.current = index;
    paintHighlights(ranges, index);
    if (index >= 0 && move.kind !== 'keep') {
      // Typing and Enter keep focus in the input: a selection would take it (see shouldSelectMatch). The highlight shows the match.
      const inputFocused = document.activeElement === inputRef.current;
      revealRange(ranges[index]);
      if (shouldSelectMatch({ inputFocused, highlightsSupported: highlightsSupported() })) {
        selectRange(ranges[index]);
      }
      // The fallback selection (no highlight API) can still take the focus: give it back to the input.
      if (inputFocused && document.activeElement !== inputRef.current) {
        inputRef.current?.focus({ preventScroll: true });
      }
    }
    setResults({ index, count });
  }

  // Focus and select the input whenever the parent asks (also on mount).
  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, [focusToken]);

  // The transcript changes (streaming, a new item, a tool row's tab): search again after a quiet time. The bar's own
  // updates are not transcript changes, so the search never triggers itself.
  useEffect(() => {
    const root = barRef.current?.parentElement ?? null;
    if (root === null) {
      return;
    }
    let timer: number | undefined;
    const observer = new MutationObserver((records) => {
      const inTranscript = records.some((r) => elementOf(r.target)?.closest(TRANSCRIPT) != null);
      if (!inTranscript) {
        return;
      }
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        if (latest.current.query !== '') {
          search({ kind: 'keep' });
        }
      }, RERUN_DELAY_MS);
    });
    observer.observe(root, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ['hidden'] });
    return () => {
      observer.disconnect();
      window.clearTimeout(timer);
    };
  }, []);

  // On close: the highlights go, and so does the selection when it is a match in the transcript.
  useEffect(() => {
    return () => {
      clearHighlights();
      const selection = document.getSelection();
      const anchor = selection?.anchorNode;
      if (selection && anchor && elementOf(anchor)?.closest(TRANSCRIPT_BODY) != null) {
        selection.removeAllRanges();
      }
    };
  }, []);

  function onQueryChange(text: string) {
    latest.current.query = text;
    setQuery(text);
    search({ kind: 'first' });
  }

  function toggleCase() {
    const next = !caseSensitive;
    latest.current.caseSensitive = next;
    setCaseSensitive(next);
    search({ kind: 'first' });
  }

  function onInputKeyDown(e: ReactKeyboardEvent<HTMLInputElement>) {
    if (e.nativeEvent.isComposing || e.key !== 'Enter') {
      return;
    }
    e.preventDefault();
    search({ kind: 'step', direction: e.shiftKey ? 'prev' : 'next' });
  }

  function onBarKeyDown(e: ReactKeyboardEvent<HTMLDivElement>) {
    // Esc closes this bar only. Stopped here, so nothing above the bar (the panel, the document) takes the key too.
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    }
  }

  // The buttons do not take focus (mousedown is prevented), so a click leaves it where it was: in the input while typing,
  // so Enter and Esc keep working after a click.
  const keepFocus = (e: ReactMouseEvent<HTMLButtonElement>) => e.preventDefault();

  return (
    <div ref={barRef} className="chat-find" role="search" onKeyDown={onBarKeyDown}>
      <input
        ref={inputRef}
        className="find-input"
        value={query}
        placeholder="Find in chat"
        aria-label="Find in chat"
        spellCheck={false}
        autoComplete="off"
        onChange={(e) => onQueryChange(e.currentTarget.value)}
        onKeyDown={onInputKeyDown}
      />
      <span className="find-count" aria-live="polite">
        {findLabel(query, results)}
      </span>
      <button
        type="button"
        className="find-button"
        aria-label="Previous match"
        data-tip="Previous (Shift+Enter)"
        onMouseDown={keepFocus}
        onClick={() => search({ kind: 'step', direction: 'prev' })}
      >
        ↑
      </button>
      <button
        type="button"
        className="find-button"
        aria-label="Next match"
        data-tip="Next (Enter)"
        onMouseDown={keepFocus}
        onClick={() => search({ kind: 'step', direction: 'next' })}
      >
        ↓
      </button>
      <button
        type="button"
        className={caseSensitive ? 'find-button is-on' : 'find-button'}
        aria-pressed={caseSensitive}
        aria-label="Match case"
        data-tip="Match case"
        onMouseDown={keepFocus}
        onClick={toggleCase}
      >
        Aa
      </button>
      <button type="button" className="find-button" aria-label="Close find" data-tip="Close (Esc)" onMouseDown={keepFocus} onClick={onClose}>
        ×
      </button>
    </div>
  );
}

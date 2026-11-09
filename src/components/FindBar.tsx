import { useEffect, useRef, useState } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent, MouseEvent as ReactMouseEvent } from 'react';
import { search, SEARCH_DECORATIONS } from '../lib/terminal';
import { findLabel } from '../lib/find';
import type { FindResults } from '../lib/find';

interface Props {
  /** Bumped by the parent (Cmd+F) to focus and select the input, also when the bar is already open. */
  focusToken: number;
  /** Esc, or the close button. The parent unmounts the bar, which clears the highlights. */
  onClose: () => void;
}

/**
 * The find bar over the terminal. It is mounted while open, so the addon's highlights live exactly as long as the bar:
 * unmounting clears them. Typing searches as you go. Enter and Shift+Enter step through the matches.
 */
export function FindBar({ focusToken, onClose }: Props) {
  const [query, setQuery] = useState('');
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [results, setResults] = useState<FindResults>({ index: -1, count: 0 });
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const sub = search.onDidChangeResults((r) => setResults({ index: r.resultIndex, count: r.resultCount }));
    return () => {
      sub.dispose();
      search.clearDecorations();
    };
  }, []);

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, [focusToken]);

  function run(text: string, sensitive: boolean, direction: 'next' | 'prev', incremental: boolean) {
    if (text === '') {
      search.clearDecorations();
      setResults({ index: -1, count: 0 });
      return;
    }
    const options = { caseSensitive: sensitive, incremental, decorations: SEARCH_DECORATIONS };
    if (direction === 'next') {
      search.findNext(text, options);
    } else {
      search.findPrevious(text, options);
    }
  }

  function onInputKeyDown(e: ReactKeyboardEvent<HTMLInputElement>) {
    if (e.nativeEvent.isComposing || e.key !== 'Enter') {
      return;
    }
    e.preventDefault();
    run(query, caseSensitive, e.shiftKey ? 'prev' : 'next', false);
  }

  function onBarKeyDown(e: ReactKeyboardEvent<HTMLDivElement>) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
    }
  }

  function toggleCase() {
    const next = !caseSensitive;
    setCaseSensitive(next);
    run(query, next, 'next', true);
  }

  // The buttons keep focus in the input, so Enter and Esc keep working after a click.
  const keepFocus = (e: ReactMouseEvent<HTMLButtonElement>) => e.preventDefault();

  return (
    <div className="find-bar" role="search" onKeyDown={onBarKeyDown}>
      <input
        ref={inputRef}
        className="find-input"
        value={query}
        placeholder="Find"
        aria-label="Find in terminal"
        spellCheck={false}
        autoComplete="off"
        onChange={(e) => {
          const text = e.currentTarget.value;
          setQuery(text);
          run(text, caseSensitive, 'next', true);
        }}
        onKeyDown={onInputKeyDown}
      />
      <span className="find-count" aria-live="polite">
        {findLabel(query, results)}
      </span>
      <button
        type="button"
        className="find-button"
        aria-label="Previous match"
        title="Previous (Shift+Enter)"
        onMouseDown={keepFocus}
        onClick={() => run(query, caseSensitive, 'prev', false)}
      >
        ↑
      </button>
      <button
        type="button"
        className="find-button"
        aria-label="Next match"
        title="Next (Enter)"
        onMouseDown={keepFocus}
        onClick={() => run(query, caseSensitive, 'next', false)}
      >
        ↓
      </button>
      <button
        type="button"
        className={caseSensitive ? 'find-button is-on' : 'find-button'}
        aria-pressed={caseSensitive}
        title="Match case"
        onMouseDown={keepFocus}
        onClick={toggleCase}
      >
        Aa
      </button>
      <button type="button" className="find-button" aria-label="Close find" title="Close (Esc)" onMouseDown={keepFocus} onClick={onClose}>
        ×
      </button>
    </div>
  );
}

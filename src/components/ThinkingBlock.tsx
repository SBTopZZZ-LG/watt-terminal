import { createContext, useContext, useId, useLayoutEffect, useRef, useState } from 'react';
import type { UIEvent } from 'react';
import { isAtBottom } from '../lib/stickScroll';
import type { TranscriptItem } from '../lib/transcript';
import { DEFAULT_THINKING_MODE, thinkingOpen, thoughtLabel } from '../lib/thinking';
import type { ThinkingMode } from '../lib/types';
import { BrailleSpinner } from './BrailleSpinner';

type ThinkingItem = Extract<TranscriptItem, { kind: 'thinking' }>;

/** The setting the transcript's reasoning blocks follow. Provided by Transcript; a block outside it uses the default. */
export const ThinkingModeContext = createContext<ThinkingMode>(DEFAULT_THINKING_MODE);

/**
 * One model call's reasoning. The header toggles it (a button, aria-expanded). The body is plain text in a box of fixed
 * height while it streams, and it follows the newest text unless the user scrolls up in it. Collapsing animates the height
 * (see App.css); the collapsed body is inert, so it is out of the tab order and the accessibility tree.
 */
export function ThinkingBlock({ item }: { item: ThinkingItem }) {
  const uid = useId();
  const mode = useContext(ThinkingModeContext);
  // A click overrides the mode for this block. It is kept with the mode it was made in, so a changed setting takes over.
  const [manual, setManual] = useState<{ mode: ThinkingMode; open: boolean } | null>(null);
  const override = manual !== null && manual.mode === mode ? manual.open : null;
  const open = thinkingOpen(mode, item.streaming, override);
  const boxRef = useRef<HTMLDivElement>(null);
  /** Whether the box follows its newest text. A scroll up inside it stops that; back at the bottom resumes it. */
  const follow = useRef(true);

  useLayoutEffect(() => {
    const box = boxRef.current;
    if (box && item.streaming && follow.current) {
      box.scrollTop = box.scrollHeight;
    }
  }, [item.text, item.streaming]);

  const onScroll = (e: UIEvent<HTMLDivElement>) => {
    follow.current = isAtBottom(e.currentTarget);
  };

  const bodyId = `${uid}-body`;
  return (
    <div className={`thinking${item.streaming ? ' is-streaming' : ''}`}>
      <button
        type="button"
        className="thinking-head"
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={() => setManual({ mode, open: !open })}
      >
        {item.streaming ? (
          <>
            <BrailleSpinner /> Thinking…
          </>
        ) : (
          thoughtLabel(item.startedAt, item.endedAt)
        )}
      </button>
      <div className="thinking-collapse" id={bodyId} inert={!open}>
        <div className="thinking-clip">
          <div className="thinking-text" ref={boxRef} onScroll={onScroll} tabIndex={0}>
            {item.text}
          </div>
        </div>
      </div>
    </div>
  );
}

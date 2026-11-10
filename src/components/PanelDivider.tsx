import { useRef, useState } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent, PointerEvent as ReactPointerEvent } from 'react';
import { isSideDock, keyboardSize, nextSizeFromDrag, panelBounds } from '../lib/panelLayout';
import type { Container, Dock } from '../lib/panelLayout';

interface Props {
  dock: Dock;
  container: Container;
  /** The panel's size as drawn now, on its axis (for aria-valuenow). */
  valueNow: number;
  /** The panel's size as it is at this moment, read from the DOM. Taken at drag start and for each key. */
  measure: () => number;
  onResize: (size: number) => void;
  onReset: () => void;
  /** After a drag or a key step. The owner refits the terminal once the new size is on screen. */
  onSettle: () => void;
}

/**
 * The divider between the agent panel and the terminal. A 6px hit area with a 1px dashed rule. Drag with the pointer
 * (captured, so a fast drag does not lose it), move with the arrow keys (Shift for a larger step, Home and End for the
 * limits), and double-click to reset. role="separator" with the panel's size as the value.
 */
export function PanelDivider({ dock, container, valueNow, measure, onResize, onReset, onSettle }: Props) {
  const side = isSideDock(dock);
  const { min, max } = panelBounds(dock, container);
  const [dragging, setDragging] = useState(false);
  const drag = useRef<{ pointer: number; start: number } | null>(null);

  const along = (e: { clientX: number; clientY: number }) => (side ? e.clientX : e.clientY);

  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) {
      return;
    }
    // No text selection while dragging.
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { pointer: along(e), start: measure() };
    setDragging(true);
  };

  const onPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (d === null) {
      return;
    }
    onResize(nextSizeFromDrag(dock, d.start, d.pointer, along(e), container));
  };

  const endDrag = () => {
    if (drag.current === null) {
      return;
    }
    drag.current = null;
    setDragging(false);
    onSettle();
  };

  const onKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    const next = keyboardSize(dock, measure(), e.key, e.shiftKey, container);
    if (next === null) {
      return;
    }
    e.preventDefault();
    onResize(next);
    onSettle();
  };

  const onDoubleClick = () => {
    onReset();
    onSettle();
  };

  const classes = ['panel-divider'];
  if (!side) {
    classes.push('is-horizontal');
  }
  if (dragging) {
    classes.push('is-active');
  }

  return (
    <div
      role="separator"
      tabIndex={0}
      aria-orientation={side ? 'vertical' : 'horizontal'}
      aria-label="Resize the agent panel"
      aria-valuenow={Math.round(valueNow)}
      aria-valuemin={min}
      aria-valuemax={max}
      title="Drag to resize. Double-click to reset."
      className={classes.join(' ')}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onKeyDown={onKeyDown}
      onDoubleClick={onDoubleClick}
    />
  );
}

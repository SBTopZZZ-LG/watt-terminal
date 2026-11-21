import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { Ref } from 'react';
import { isClipped, placement, resolveTip, showDelay } from '../lib/tooltip';
import type { Placement } from '../lib/tooltip';

/** The id of the one tip. An anchor's aria-describedby points here while its tip shows. */
export const TIP_ID = 'watt-tip';

interface View {
  text: string;
  /** Null until the tip is measured: it is rendered hidden first, then placed before the first paint. */
  pos: Placement | null;
}

function clippedNow(el: HTMLElement): boolean {
  return isClipped(el);
}

function keyboardFocused(el: Element): boolean {
  try {
    return el.matches(':focus-visible');
  } catch {
    return false;
  }
}

/**
 * The app's one tooltip. Mounted once; document-level listeners find the nearest element with data-tip (see
 * src/lib/tooltip.ts) on hover and on keyboard focus, and show its text after a short delay. A data-tip-clip tip shows only
 * while its element is really clipped. The tip hides on scroll, mouse down, Escape, blur, resize, when its anchor leaves
 * the page, and when the pointer or focus leaves the anchor. It has pointer-events: none, so it never takes focus or hover.
 */
export function Tooltip() {
  const [view, setView] = useState<View | null>(null);
  const tipRef = useRef<HTMLDivElement>(null);
  const anchorRef = useRef<HTMLElement | null>(null);

  // After the tip renders hidden, measure it and place it. Runs before paint, so the tip never shows in the wrong place.
  useLayoutEffect(() => {
    const tip = tipRef.current;
    const anchor = anchorRef.current;
    if (view === null || view.pos !== null || !tip || !anchor) {
      return;
    }
    const box = anchor.getBoundingClientRect();
    const size = { width: tip.offsetWidth, height: tip.offsetHeight };
    const viewport = { width: window.innerWidth, height: window.innerHeight };
    setView({ text: view.text, pos: placement(box, size, viewport) });
  }, [view]);

  useEffect(() => {
    let timer: number | undefined;
    let lastHideAt: number | null = null;
    // The anchor whose tip is pending or showing, and whether it is showing.
    let current: HTMLElement | null = null;
    let visible = false;
    let savedDescribedBy: string | null = null;
    let observer: MutationObserver | null = null;

    const hide = () => {
      window.clearTimeout(timer);
      timer = undefined;
      observer?.disconnect();
      observer = null;
      if (current !== null && visible) {
        if (savedDescribedBy === null) {
          current.removeAttribute('aria-describedby');
        } else {
          current.setAttribute('aria-describedby', savedDescribedBy);
        }
        lastHideAt = Date.now();
        setView(null);
      }
      current = null;
      anchorRef.current = null;
      visible = false;
    };

    const show = (anchor: HTMLElement, text: string) => {
      current = anchor;
      anchorRef.current = anchor;
      timer = window.setTimeout(() => {
        timer = undefined;
        if (current !== anchor || !anchor.isConnected) {
          hide();
          return;
        }
        visible = true;
        savedDescribedBy = anchor.getAttribute('aria-describedby');
        anchor.setAttribute('aria-describedby', TIP_ID);
        setView({ text, pos: null });
        // The anchor can be removed without a mouse event (a list re-renders, a row goes): then the tip goes too.
        observer = new MutationObserver(() => {
          if (!anchor.isConnected) {
            hide();
          }
        });
        observer.observe(document.body, { childList: true, subtree: true });
      }, showDelay(Date.now(), lastHideAt));
    };

    // The nearest tip for a target. A new anchor starts its delay; the same one keeps its state; no tip hides the current.
    const consider = (target: Element) => {
      const hit = resolveTip(target as HTMLElement, clippedNow);
      if (hit === null) {
        if (current !== null) {
          hide();
        }
        return;
      }
      if (hit.anchor === current) {
        return;
      }
      hide();
      show(hit.anchor, hit.text);
    };

    const onMouseOver = (e: MouseEvent) => {
      if (e.target instanceof Element) {
        consider(e.target);
      }
    };
    const onMouseOut = (e: MouseEvent) => {
      const to = e.relatedTarget;
      if (current !== null && (!(to instanceof Node) || !current.contains(to))) {
        hide();
      }
    };
    // Keyboard focus shows a tip; a mouse click focuses too, and that must not show one.
    const onFocusIn = (e: FocusEvent) => {
      if (e.target instanceof Element && keyboardFocused(e.target)) {
        consider(e.target);
      }
    };
    const onFocusOut = (e: FocusEvent) => {
      if (current !== null && e.target instanceof Node && current.contains(e.target)) {
        hide();
      }
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        hide();
      }
    };

    document.addEventListener('mouseover', onMouseOver);
    document.addEventListener('mouseout', onMouseOut);
    document.addEventListener('focusin', onFocusIn);
    document.addEventListener('focusout', onFocusOut);
    document.addEventListener('mousedown', hide);
    document.addEventListener('scroll', hide, { capture: true, passive: true });
    document.addEventListener('keydown', onKeyDown);
    window.addEventListener('blur', hide);
    window.addEventListener('resize', hide);
    return () => {
      document.removeEventListener('mouseover', onMouseOver);
      document.removeEventListener('mouseout', onMouseOut);
      document.removeEventListener('focusin', onFocusIn);
      document.removeEventListener('focusout', onFocusOut);
      document.removeEventListener('mousedown', hide);
      document.removeEventListener('scroll', hide, { capture: true });
      document.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('blur', hide);
      window.removeEventListener('resize', hide);
      hide();
    };
  }, []);

  if (view === null) {
    return null;
  }
  return <TipBox ref={tipRef} text={view.text} pos={view.pos} />;
}

interface TipBoxProps {
  text: string;
  /** Null while the tip is being measured: it is then hidden, so it cannot flash in the wrong place. */
  pos: Placement | null;
  ref?: Ref<HTMLDivElement>;
}

/** The tip's markup. Its text is React text, so it is escaped; it has no focusable content and takes no pointer events. */
export function TipBox({ text, pos, ref }: TipBoxProps) {
  return (
    <div
      ref={ref}
      id={TIP_ID}
      role="tooltip"
      className={`tooltip${pos !== null ? ` tooltip-${pos.side}` : ''}`}
      style={pos === null ? { left: 0, top: 0, visibility: 'hidden' } : { left: pos.x, top: pos.y }}
    >
      {text}
    </div>
  );
}

import { createContext, useContext, useEffect, useId, useRef } from 'react';
import type { KeyboardEvent, MouseEvent, ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { nextStop } from '../lib/sheetFocus';

/**
 * The element a sheet is drawn in: the settings card, so the sheet covers the card and not the window. Null (no host, as in a
 * static render) draws the sheet where it is.
 */
export const SheetHostContext = createContext<HTMLElement | null>(null);

const FOCUSABLE = 'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

function focusables(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter((el) => el.closest('[hidden]') === null);
}

export interface SheetProps {
  title: string;
  /** Asked for by Escape, the close button and a click on the backdrop. The owner decides whether the sheet goes. */
  onClose(): void;
  children: ReactNode;
  /** Pinned under the body: the actions. */
  footer?: ReactNode;
  /** A wider sheet, for the provider editor. */
  wide?: boolean;
}

/**
 * A second-level dialog on top of the settings card. It moves focus in on open (the element with `data-autofocus`, else the
 * first control), keeps Tab inside, and gives focus back to the opener on close. Escape and a click on the backdrop ask the
 * owner to close it; Escape does not reach the card's own handler.
 */
export function Sheet({ title, onClose, children, footer, wide = false }: SheetProps) {
  const uid = useId();
  const host = useContext(SheetHostContext);
  const sheetRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef(onClose);
  const backdropPressed = useRef(false);

  useEffect(() => {
    closeRef.current = onClose;
  });

  useEffect(() => {
    const sheet = sheetRef.current;
    if (sheet === null) {
      return;
    }
    const opener = document.activeElement;
    const initial = () => sheet.querySelector<HTMLElement>('[data-autofocus]') ?? focusables(sheet)[0] ?? sheet;
    initial().focus();
    // Focus that leaves the sheet (a shortcut, the terminal taking it) comes back to it.
    const pull = (e: FocusEvent) => {
      if (e.target instanceof Node && !sheet.contains(e.target)) {
        initial().focus();
      }
    };
    document.addEventListener('focusin', pull);
    return () => {
      document.removeEventListener('focusin', pull);
      if (opener instanceof HTMLElement && opener.isConnected && opener !== document.body) {
        opener.focus();
      }
    };
    // Runs once per mount: a sheet whose content changes is remounted with a key instead.
  }, []);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      closeRef.current();
      return;
    }
    if (e.key === 'Tab') {
      // The card's own Tab trap must not see this key: the sheet keeps focus to itself.
      e.stopPropagation();
      const sheet = sheetRef.current;
      if (sheet === null) {
        return;
      }
      e.preventDefault();
      const stops = focusables(sheet);
      if (stops.length === 0) {
        return;
      }
      const at = stops.indexOf(document.activeElement as HTMLElement);
      stops[nextStop(stops.length, at, e.shiftKey)].focus();
    }
  };

  const titleId = `${uid}-title`;
  const node = (
    <div
      className="sheet-backdrop"
      onMouseDown={(e: MouseEvent<HTMLDivElement>) => {
        backdropPressed.current = e.target === e.currentTarget;
      }}
      onClick={(e: MouseEvent<HTMLDivElement>) => {
        // Only a press and release on the backdrop itself closes it.
        if (backdropPressed.current && e.target === e.currentTarget) {
          closeRef.current();
        }
      }}
    >
      <div
        ref={sheetRef}
        className={wide ? 'sheet sheet-wide' : 'sheet'}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        <header className="sheet-head">
          <h3 className="sheet-title" id={titleId}>
            {title}
          </h3>
          <button type="button" className="sheet-close" aria-label="Close" onClick={() => closeRef.current()}>
            ✕
          </button>
        </header>
        <div className="sheet-body">{children}</div>
        {footer !== undefined && <footer className="sheet-foot">{footer}</footer>}
      </div>
    </div>
  );
  return host === null ? node : createPortal(node, host);
}

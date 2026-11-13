import { useEffect, useId, useRef } from 'react';
import type { KeyboardEvent } from 'react';

/** The help text as loaded. A failed load shows the error with Retry; the dialog itself never touches the transcript. */
export type HelpLoad = { kind: 'loading' } | { kind: 'ready'; text: string } | { kind: 'error'; message: string };

interface Props {
  load: HelpLoad;
  onRetry(): void;
  onClose(): void;
}

const FOCUSABLE = 'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"]';

/**
 * The `/help` dialog: a modal over the window showing the slash-command list. The card reuses the settings
 * dialog's classes (with an extra `help-dialog`), so the head, the foot and the buttons match. Focus moves
 * into the dialog on open and stays inside while open (a stray focus is pulled back, Tab wraps); Esc and a
 * backdrop press-release close it. Restoring focus to the bar is the caller's `focusToken`, not this dialog.
 */
export function HelpDialog({ load, onRetry, onClose }: Props) {
  const titleId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const backdropPressed = useRef(false);

  // Focus moves into the dialog on open.
  useEffect(() => {
    dialogRef.current?.focus();
  }, []);

  // Keyboard focus stays in the dialog: a focus that moves outside it is pulled back.
  useEffect(() => {
    const pull = (e: FocusEvent) => {
      const dialog = dialogRef.current;
      if (dialog && e.target instanceof Node && !dialog.contains(e.target)) {
        dialog.focus();
      }
    };
    document.addEventListener('focusin', pull);
    return () => document.removeEventListener('focusin', pull);
  }, []);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      onClose();
      return;
    }
    if (e.key === 'Tab') {
      // Tab moves within the dialog: from the last control to the first, and back.
      const items = Array.from(dialogRef.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []);
      if (items.length === 0) {
        e.preventDefault();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement;
      if (e.shiftKey && (active === first || active === dialogRef.current)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      }
    }
  };

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        backdropPressed.current = e.target === e.currentTarget;
      }}
      onClick={(e) => {
        // Only a press and release on the backdrop itself closes it, so a drag that ends outside the card does not.
        if (backdropPressed.current && e.target === e.currentTarget) {
          onClose();
        }
      }}
    >
      <div
        ref={dialogRef}
        className="settings-dialog help-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        <header className="settings-head">
          <h2 className="settings-title" id={titleId}>
            Slash commands
          </h2>
        </header>
        <div className="help-body">
          {load.kind === 'loading' ? (
            <div className="help-state" role="status">
              Loading slash commands...
            </div>
          ) : load.kind === 'error' ? (
            <div className="help-state help-state-error" role="alert">
              {`Could not load the slash commands: ${load.message}`}
              <button type="button" onClick={onRetry}>
                Retry
              </button>
            </div>
          ) : (
            <p className="help-text">{load.text}</p>
          )}
        </div>
        <footer className="settings-foot">
          <button type="button" onClick={onClose}>
            Close
          </button>
        </footer>
      </div>
    </div>
  );
}

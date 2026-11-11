import { useEffect, useId, useRef, useState } from 'react';
import type { KeyboardEvent, ReactNode } from 'react';
import { outcomeView } from '../lib/integrate';
import type { OpenPhase } from '../lib/integrate';

interface Props {
  phase: OpenPhase;
  /** Starts the integration. Only offered on the consent screen. */
  onStart(): void;
  /** Closes the dialog. Ignored while the integration runs: the handshake cannot be stopped from here. */
  onClose(): void;
}

/** Moves focus back to the element that had it when the dialog opened, or to the terminal when that is gone. */
function restoreFocus(opener: Element | null) {
  if (opener instanceof HTMLElement && opener.isConnected && opener !== document.body) {
    opener.focus();
    return;
  }
  document.querySelector<HTMLElement>('.xterm-helper-textarea')?.focus();
}

/**
 * The consent dialog for integrating the shell in the foreground. Three screens: the consent (Cancel is focused, so Enter
 * does not start it), the running handshake (no buttons, keys are locked), and the result. It opens only on a click, never on
 * its own. Escape and the backdrop close it, except while it runs.
 */
export function IntegrateDialog({ phase, onStart, onClose }: Props) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const backdropPressed = useRef(false);
  const titleId = useId();
  // The element that had focus when the dialog opened. Read only in a browser: the server render (tests) has no document.
  const [opener] = useState<Element | null>(() => (typeof document === 'undefined' ? null : document.activeElement));
  const running = phase.kind === 'running';

  // Focus follows the screen: Cancel on the consent, Close on the result, the dialog itself while it runs.
  useEffect(() => {
    if (phase.kind === 'consent') {
      cancelRef.current?.focus();
    } else if (phase.kind === 'result') {
      closeRef.current?.focus();
    } else {
      dialogRef.current?.focus();
    }
  }, [phase.kind]);

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

  useEffect(() => () => restoreFocus(opener), [opener]);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      if (!running) {
        onClose();
      }
    }
  };

  let body: ReactNode;
  let foot: ReactNode;
  if (phase.kind === 'consent') {
    body = (
      <>
        <p>
          This types one short command into the shell you are connected to, then sends a small script, about 4 KB, that adds
          prompt and command markers so the agent can run and read commands there. Nothing is written to disk on the remote
          machine. The script lives in that shell&apos;s memory only and ends when you exit it.
        </p>
        <p>Make sure the remote shell is at its prompt, not at a password or other prompt: it will receive the typed lines.</p>
        <p>Works with zsh, and bash 4.4 or newer. Other shells are left untouched.</p>
        <p className="integrate-note">The one-line loader may remain in zsh history (it is removed from bash history).</p>
      </>
    );
    foot = (
      <>
        <button ref={cancelRef} type="button" onClick={onClose}>
          Cancel
        </button>
        <button type="button" className="integrate-primary" onClick={onStart}>
          Integrate
        </button>
      </>
    );
  } else if (phase.kind === 'running') {
    body = (
      <p className="integrate-running" role="status">
        Integrating…
      </p>
    );
    foot = null;
  } else {
    const view = outcomeView(phase.outcome);
    body = (
      <p className={`integrate-result is-${view.tone}`} role="status">
        {view.text}
      </p>
    );
    foot = (
      <button ref={closeRef} type="button" onClick={onClose}>
        Close
      </button>
    );
  }

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        backdropPressed.current = e.target === e.currentTarget;
      }}
      onClick={(e) => {
        // Only a press and release on the backdrop itself closes it, so a drag that ends outside the card does not.
        if (backdropPressed.current && e.target === e.currentTarget && !running) {
          onClose();
        }
      }}
    >
      <div
        ref={dialogRef}
        className="integrate-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        <header className="integrate-head">
          <h2 className="integrate-title" id={titleId}>
            {phase.kind === 'consent' ? 'Integrate this shell?' : 'Integrate this shell'}
          </h2>
        </header>
        <div className="integrate-body">{body}</div>
        {foot !== null && <footer className="integrate-foot">{foot}</footer>}
      </div>
    </div>
  );
}

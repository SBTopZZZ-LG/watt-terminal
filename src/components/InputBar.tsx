import { useEffect, useRef, useState } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent } from 'react';
import { barKeyIntent } from '../lib/inputState';
import type { BarMode } from '../lib/inputState';
import { History, messageFor } from '../lib/barInput';

interface Props {
  mode: BarMode;
  /** Why the bar is limited. Shown under the input, or as the input's placeholder when hintInline is set. */
  hint: string | null;
  /** Show the hint as the input's placeholder instead of a row under it, so the bar's height does not change. */
  hintInline: boolean;
  hasPendingApproval: boolean;
  /** Bumped by the parent to move focus into the input (or the restart button, when the shell exited). */
  focusToken: number;
  /** Reports when the input gains or loses focus. */
  onFocusChange: (focused: boolean) => void;
  /** The shell exited: starts a new session. */
  onRestart: () => void;
  restarting: boolean;
  /** Resolves to an error message, or null on success. The text is kept on error. */
  onAgent: (text: string) => Promise<string | null>;
  onInterrupt: () => void;
  onApproveFirst: () => void;
  onDenyFirst: () => void;
  /** Escape with nothing to deny: focus goes back to the terminal. */
  onClose: () => void;
}

export function InputBar(props: Props) {
  const { mode, hint, hintInline, hasPendingApproval, focusToken } = props;
  const [text, setText] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const restartRef = useRef<HTMLButtonElement>(null);
  const history = useRef(new History());

  // A new token moves focus. The effect reads the mode at that moment, when the input is rendered.
  useEffect(() => {
    if (focusToken === 0) {
      return;
    }
    if (mode === 'exited') {
      restartRef.current?.focus();
    } else if (mode === 'chat') {
      inputRef.current?.focus();
    }
  }, [focusToken]);

  if (mode === 'exited') {
    return (
      <div className="bar bar-exited">
        <span className="bar-notice">{hint}</span>
        <button ref={restartRef} type="button" className="bar-restart" disabled={props.restarting} onClick={props.onRestart}>
          Start new session
        </button>
      </div>
    );
  }

  if (mode === 'collapsed') {
    return (
      <div className="bar bar-collapsed" title="Ctrl/Cmd+Shift+A to message the agent">
        <span className="bar-hint">{hint}</span>
      </div>
    );
  }

  async function submit() {
    const message = messageFor(text);
    if (message === null) {
      return;
    }
    setSending(true);
    const err = await props.onAgent(message);
    setSending(false);
    if (err) {
      setError(err);
      return;
    }
    history.current.record(text);
    setText('');
    setError(null);
  }

  function onKeyDown(e: ReactKeyboardEvent<HTMLInputElement>) {
    if (e.nativeEvent.isComposing) {
      return;
    }
    const intent = barKeyIntent(
      e.key,
      { ctrl: e.ctrlKey, shift: e.shiftKey },
      { inputEmpty: text === '', hasPendingApproval },
    );
    switch (intent) {
      case 'interrupt':
        // ^C always goes to the PTY, whatever the state. Rust also cancels pending approvals on it.
        e.preventDefault();
        props.onInterrupt();
        return;
      case 'approve_first':
        e.preventDefault();
        props.onApproveFirst();
        return;
      case 'deny_first':
        e.preventDefault();
        props.onDenyFirst();
        return;
      case 'close':
        e.preventDefault();
        props.onClose();
        return;
      case 'submit':
        e.preventDefault();
        if (!sending) {
          void submit();
        }
        return;
      case 'history_prev': {
        e.preventDefault();
        const older = history.current.back(text);
        if (older !== null) {
          setText(older);
        }
        return;
      }
      case 'history_next': {
        e.preventDefault();
        const newer = history.current.forward();
        if (newer !== null) {
          setText(newer);
        }
        return;
      }
      case 'none':
        return;
    }
  }

  // The inline hint is the placeholder, so it shows only while the input is empty. An error takes the row instead,
  // and the placeholder goes back to the normal text.
  const inlineHint = hintInline && hint !== null && error === null ? hint : null;

  return (
    <div className={`bar bar-${mode}`}>
      <span className="bar-mode mode-agent">agent</span>
      <input
        ref={inputRef}
        className="bar-input"
        value={text}
        placeholder={inlineHint ?? 'Ask the agent…  (Ctrl/Cmd+Shift+A to focus, Esc to return)'}
        spellCheck={false}
        autoComplete="off"
        onChange={(e) => {
          setText(e.currentTarget.value);
          setError(null);
        }}
        onKeyDown={onKeyDown}
        onFocus={() => props.onFocusChange(true)}
        onBlur={() => props.onFocusChange(false)}
      />
      {error !== null && <div className="bar-error">{error}</div>}
      {error === null && hint !== null && !hintInline && <div className="bar-hint">{hint}</div>}
    </div>
  );
}

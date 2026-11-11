import { useEffect, useRef, useState } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent } from 'react';
import { barKeyIntent } from '../lib/inputState';
import type { BarMode } from '../lib/inputState';
import { History, historyKeyTakesLine, messageFor, sendButtonFor } from '../lib/barInput';

const DEFAULT_PLACEHOLDER = 'Ask the agent…  (Ctrl/Cmd+Shift+A to focus, Esc to return)';

interface Props {
  mode: BarMode;
  /** Why the bar is limited. Shown under the input, or as the input's placeholder when hintInline is set. */
  hint: string | null;
  /** Show the hint as the input's placeholder instead of a row under it, so the bar's height does not change. */
  hintInline: boolean;
  hasPendingApproval: boolean;
  /** The agent is mid-turn. The button becomes Stop, and the field's border shows a light while it lasts. */
  busy: boolean;
  /** An integration is running: the input takes no text and sends nothing until it answers. */
  locked: boolean;
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
  const { mode, hint, hintInline, hasPendingApproval, busy, focusToken } = props;
  const [text, setText] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const inputRef = useRef<HTMLTextAreaElement>(null);
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
    if (message === null || props.locked) {
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

  const send = sendButtonFor(busy, text, props.locked);

  /** The send button: Stop while busy, otherwise the same as Enter. Focus stays in the input either way. */
  function onSendClick() {
    if (send.stop) {
      props.onInterrupt();
      inputRef.current?.focus();
      return;
    }
    if (!sending && !props.locked) {
      void submit().then(() => inputRef.current?.focus());
    }
  }

  function onKeyDown(e: ReactKeyboardEvent<HTMLTextAreaElement>) {
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
        if (!sending && !props.locked) {
          void submit();
        }
        return;
      case 'history_prev': {
        // In a multi-line text, Up moves the caret until it is on the first line.
        if (!historyKeyTakesLine('up', text, e.currentTarget.selectionStart)) {
          return;
        }
        e.preventDefault();
        const older = history.current.back(text);
        if (older !== null) {
          setText(older);
        }
        return;
      }
      case 'history_next': {
        // In a multi-line text, Down moves the caret until it is on the last line.
        if (!historyKeyTakesLine('down', text, e.currentTarget.selectionEnd)) {
          return;
        }
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
  const placeholder = inlineHint ?? DEFAULT_PLACEHOLDER;

  return (
    <div className={`bar bar-${mode}`}>
      <div className={busy ? 'bar-field is-busy' : 'bar-field'}>
        {/*
         * Sizes the field. It holds the text the textarea shows (or the placeholder, when empty) in the same box, so the
         * row is as tall as the text, up to the max-height. The trailing space keeps a final newline on its own line.
         * Hidden, and not read by assistive tech: the textarea is the control.
         */}
        <div className="bar-mirror" aria-hidden="true">
          {`${text === '' ? placeholder : text} `}
        </div>
        <textarea
          ref={inputRef}
          className="bar-input"
          rows={1}
          value={text}
          placeholder={placeholder}
          spellCheck={false}
          autoComplete="off"
          disabled={props.locked}
          onChange={(e) => {
            setText(e.currentTarget.value);
            setError(null);
          }}
          onKeyDown={onKeyDown}
          onFocus={() => props.onFocusChange(true)}
          onBlur={() => props.onFocusChange(false)}
        />
      </div>
      {/* mousedown keeps focus in the input: a blur closes the bar on the alternate screen. */}
      <button
        type="button"
        className={send.stop ? 'bar-send is-stop' : 'bar-send'}
        aria-label={send.stop ? 'Stop' : 'Send'}
        title={send.stop ? 'Stop' : 'Send'}
        disabled={send.disabled}
        onMouseDown={(e) => e.preventDefault()}
        onClick={onSendClick}
      >
        {send.stop ? <StopIcon /> : <SendIcon />}
      </button>
      {error !== null && <div className="bar-error">{error}</div>}
      {error === null && hint !== null && !hintInline && <div className="bar-hint">{hint}</div>}
    </div>
  );
}

function SendIcon() {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true" focusable="false">
      <path d="M12 19V5M5 12l7-7 7 7" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

function StopIcon() {
  return (
    <svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true" focusable="false">
      <rect x="5" y="5" width="14" height="14" rx="2" fill="currentColor" />
    </svg>
  );
}

import { useEffect, useRef, useState } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent } from 'react';
import { barKeyIntent } from '../lib/inputState';
import type { BarMode } from '../lib/inputState';
import { clearScreenDecision, decideSubmit, History, isClearScreenKey, modeLabel } from '../lib/barInput';

interface Props {
  mode: BarMode;
  /** Why the bar is off or limited. Shown under the input. */
  hint: string | null;
  hasPendingApproval: boolean;
  /** Bumped by the parent to move focus into the input. */
  focusToken: number;
  /** Each resolves to an error message, or null on success. The text is kept on error. */
  onShell: (command: string) => Promise<string | null>;
  onAgent: (text: string) => Promise<string | null>;
  onInterrupt: () => void;
  /** Ctrl+L at the prompt: the shell clears the screen. The text in the bar is left alone. */
  onClearScreen: () => void;
  onApproveFirst: () => void;
  onDenyFirst: () => void;
  onClose: () => void;
}

export function InputBar(props: Props) {
  const { mode, hint, hasPendingApproval, focusToken } = props;
  const [text, setText] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const history = useRef(new History());

  // A new token moves focus. The effect reads the mode at that moment, when the input is rendered.
  useEffect(() => {
    if (focusToken > 0 && mode !== 'collapsed' && mode !== 'off') {
      inputRef.current?.focus();
    }
  }, [focusToken]);

  if (mode === 'collapsed') {
    return (
      <div className="bar bar-collapsed" title="Ctrl+Shift+A to message the agent">
        <span className="bar-hint">{hint}</span>
      </div>
    );
  }

  const off = mode === 'off';
  const label = modeLabel(text);
  const placeholder = off
    ? (hint ?? 'Input is off.')
    : mode === 'agent_only'
      ? 'Message the agent'
      : 'Type a command after ! or ask the agent';

  async function submit() {
    const decision = decideSubmit(mode, hint, text);
    if (decision.kind === 'none') {
      return;
    }
    if (decision.kind === 'reject') {
      setError(decision.reason);
      return;
    }
    setSending(true);
    const err = decision.kind === 'shell' ? await props.onShell(decision.command) : await props.onAgent(decision.text);
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
    if (isClearScreenKey(e.key, { ctrl: e.ctrlKey, shift: e.shiftKey, alt: e.altKey, meta: e.metaKey })) {
      if (clearScreenDecision(mode, sending) === 'send') {
        e.preventDefault();
        props.onClearScreen();
      }
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
        if (!sending && !off) {
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

  return (
    <div className={`bar bar-${mode}`}>
      <span className={`bar-mode mode-${off ? 'off' : label}`}>{off ? '-' : label}</span>
      <input
        ref={inputRef}
        className="bar-input"
        value={text}
        disabled={off}
        placeholder={placeholder}
        spellCheck={false}
        autoComplete="off"
        onChange={(e) => {
          setText(e.currentTarget.value);
          setError(null);
        }}
        onKeyDown={onKeyDown}
      />
      {error !== null && <div className="bar-error">{error}</div>}
      {!off && error === null && hint !== null && <div className="bar-hint">{hint}</div>}
    </div>
  );
}

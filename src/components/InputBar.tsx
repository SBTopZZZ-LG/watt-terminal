import { useEffect, useRef, useState } from 'react';
import type { ChangeEvent, ClipboardEvent as ReactClipboardEvent, DragEvent as ReactDragEvent, KeyboardEvent as ReactKeyboardEvent } from 'react';
import { barKeyIntent } from '../lib/inputState';
import type { BarMode } from '../lib/inputState';
import { History, historyKeyTakesLine, messageFor, sendButtonFor } from '../lib/barInput';
import { admitRefusal, countRefusal, filesFromPaste, loadOf, prepareFile } from '../lib/attachments';
import type { Prepared } from '../lib/attachments';
import { carriesFiles } from '../lib/dropGuard';
import { showsVisionWarning } from '../lib/modelVision';
import { visibleEscapes } from '../lib/visibleEscapes';
import type { AttachmentInput } from '../lib/types';
import { AttachmentChips } from './AttachmentChips';
import type { ChipItem } from './AttachmentChips';

const DEFAULT_PLACEHOLDER = 'Ask the agent…';
/** The shortcut hint, kept as the input's tooltip so the placeholder stays one line at every panel width. */
const SHORTCUT_HINT = 'Ctrl/Cmd+Shift+A to focus, Esc to return';

/** An attachment on the bar. `input` is null while the file is still being prepared. */
interface Draft {
  key: number;
  name: string;
  /** The size: the processed size for an image, the file's for a text document. 0 while preparing. */
  bytes: number;
  input: AttachmentInput | null;
}

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
  /** Resolves to an error message, or null on success. The text and the attachments are kept on error. */
  onAgent: (text: string, attachments: AttachmentInput[]) => Promise<string | null>;
  /** Whether the selected model takes images (Model.vision): false shows a note under the attachments. Null or true: no note. */
  modelVision?: boolean | null;
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
  const [drafts, setDrafts] = useState<Draft[]>([]);
  // The same list, for the steps that finish outside a render (a file's preparation). Kept in step by commit.
  const draftsRef = useRef<Draft[]>([]);
  const draftKey = useRef(0);
  // Files are prepared one after another, in the order they were given, so a file that does not fit is the last one given.
  const queue = useRef<Promise<void>>(Promise.resolve());
  const [dragging, setDragging] = useState(false);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
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

  /** Replaces the attachments: the state for the render, and the ref for the steps that run outside it. */
  function commit(next: Draft[]) {
    draftsRef.current = next;
    setDrafts(next);
  }

  /** A rejection goes to the error row. Several are listed one per line. */
  function reject(message: string) {
    setError((prev) => (prev === null ? message : `${prev}\n${message}`));
  }

  /** Takes files from the paperclip, a paste or a drop. Each is prepared in turn; a file that cannot be attached is reported. */
  function addFiles(files: File[]) {
    if (props.locked || files.length === 0) {
      return;
    }
    setError(null);
    for (const file of files) {
      const full = countRefusal(draftsRef.current.length);
      if (full !== null) {
        reject(`${visibleEscapes(file.name || 'file')}: ${full}`);
        continue;
      }
      draftKey.current += 1;
      const key = draftKey.current;
      commit([...draftsRef.current, { key, name: file.name, bytes: 0, input: null }]);
      queue.current = queue.current
        .then(() => prepareFile(file))
        .then((result) => settle(key, result))
        .catch(() => undefined);
    }
  }

  /** A file has been prepared. It joins the message if it fits; otherwise it is removed, and the reason is shown. */
  function settle(key: number, result: Prepared) {
    const current = draftsRef.current;
    const self = current.find((d) => d.key === key);
    if (self === undefined) {
      // Sent or cleared while it was being prepared.
      return;
    }
    if (!result.ok) {
      commit(current.filter((d) => d.key !== key));
      reject(result.message);
      return;
    }
    const others = current.flatMap((d) => (d.key !== key && d.input !== null ? [{ kind: d.input.kind, bytes: d.bytes }] : []));
    const refusal = admitRefusal(loadOf(others), { kind: result.input.kind, bytes: result.bytes });
    if (refusal !== null) {
      commit(current.filter((d) => d.key !== key));
      reject(`${visibleEscapes(self.name)}: ${refusal}`);
      return;
    }
    commit(current.map((d) => (d.key === key ? { key, name: d.name, bytes: result.bytes, input: result.input } : d)));
  }

  function removeDraft(key: number) {
    commit(draftsRef.current.filter((d) => d.key !== key));
    setError(null);
    inputRef.current?.focus();
  }

  async function submit() {
    const current = draftsRef.current;
    const message = messageFor(text);
    const ready = current.flatMap((d) => (d.input === null ? [] : [d.input]));
    const processing = current.some((d) => d.input === null);
    if (props.locked || processing || (message === null && ready.length === 0)) {
      return;
    }
    const sent = new Set(current.filter((d) => d.input !== null).map((d) => d.key));
    setSending(true);
    const err = await props.onAgent(message ?? '', ready);
    setSending(false);
    if (err) {
      setError(err);
      return;
    }
    history.current.record(text);
    setText('');
    commit(draftsRef.current.filter((d) => !sent.has(d.key)));
    setError(null);
  }

  const readyCount = drafts.filter((d) => d.input !== null).length;
  const processing = drafts.some((d) => d.input === null);
  const send = sendButtonFor(busy, text, props.locked, { ready: readyCount, processing });

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

  /** Files in the paste win over the text that comes with them. A paste with no files is pasted as text. */
  function onPaste(e: ReactClipboardEvent<HTMLTextAreaElement>) {
    const files = filesFromPaste(e.clipboardData);
    if (files.length === 0) {
      return;
    }
    e.preventDefault();
    addFiles(files);
  }

  function onPick(e: ChangeEvent<HTMLInputElement>) {
    const files = e.currentTarget.files ? Array.from(e.currentTarget.files) : [];
    // Reset, so that picking the same file again fires the change again.
    e.currentTarget.value = '';
    addFiles(files);
    inputRef.current?.focus();
  }

  function onDragOver(e: ReactDragEvent<HTMLDivElement>) {
    if (props.locked || !carriesFiles(e.dataTransfer)) {
      return;
    }
    e.preventDefault();
    e.dataTransfer.dropEffect = 'copy';
    setDragging(true);
  }

  function onDragLeave(e: ReactDragEvent<HTMLDivElement>) {
    if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
      setDragging(false);
    }
  }

  function onDrop(e: ReactDragEvent<HTMLDivElement>) {
    setDragging(false);
    if (props.locked || !carriesFiles(e.dataTransfer)) {
      return;
    }
    e.preventDefault();
    addFiles(Array.from(e.dataTransfer.files));
  }

  const chips: ChipItem[] = drafts.map((d) => ({
    key: d.key,
    name: visibleEscapes(d.name),
    kind: d.input?.kind === 'image' ? 'image' : 'file',
    bytes: d.input === null ? null : d.bytes,
    thumb: d.input?.thumb,
    processing: d.input === null,
  }));

  function onKeyDown(e: ReactKeyboardEvent<HTMLTextAreaElement>) {
    if (e.nativeEvent.isComposing) {
      return;
    }
    const intent = barKeyIntent(
      e.key,
      { ctrl: e.ctrlKey, shift: e.shiftKey },
      // Staged attachments are not an empty input: Enter sends them rather than approving a pending call.
      { inputEmpty: text === '' && drafts.length === 0, hasPendingApproval },
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
    <div
      className={dragging ? `bar bar-${mode} is-dragging` : `bar bar-${mode}`}
      onDragEnter={onDragOver}
      onDragOver={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDrop}
    >
      {drafts.length > 0 && <AttachmentChips items={chips} onRemove={removeDraft} />}
      {showsVisionWarning(props.modelVision, drafts.filter((d) => d.input?.kind === 'image').length) && (
        <div className="bar-vision">This model may not accept images; they'd be left out.</div>
      )}
      {/* mousedown keeps focus in the input, as for Send. The picker opens on the hidden file input. */}
      <button
        type="button"
        className="bar-attach-button"
        aria-label="Attach files"
        title="Attach files"
        disabled={props.locked}
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => fileInputRef.current?.click()}
      >
        <ClipIcon />
      </button>
      <input ref={fileInputRef} type="file" multiple hidden onChange={onPick} />
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
          title={SHORTCUT_HINT}
          aria-label="Message the agent"
          spellCheck={false}
          autoComplete="off"
          disabled={props.locked}
          onChange={(e) => {
            setText(e.currentTarget.value);
            setError(null);
          }}
          onKeyDown={onKeyDown}
          onPaste={onPaste}
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

function ClipIcon() {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true" focusable="false">
      <path
        d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

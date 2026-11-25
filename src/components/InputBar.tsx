import { useEffect, useRef, useState } from 'react';
import type { ChangeEvent, ClipboardEvent as ReactClipboardEvent, DragEvent as ReactDragEvent, KeyboardEvent as ReactKeyboardEvent } from 'react';
import { barKeyIntent } from '../lib/inputState';
import type { BarMode } from '../lib/inputState';
import { agentPlaceholder, History, historyKeyTakesLine, messageFor, sendButtonFor } from '../lib/barInput';
import { queuedAttachmentNote, queueHeader, restoreText } from '../lib/messageQueue';
import type { QueuedMessage } from '../lib/messageQueue';
import { BYPASS_LABEL } from '../lib/approvalReasons';
import { acceptMenuItem, commandMenuItems, filterCommandMenu, menuAcceptRuns, menuKeyIntent, menuQuery, moveMenuIndex } from '../lib/commandMenu';
import type { CommandMenuItem } from '../lib/commandMenu';
import { admitRefusal, countRefusal, decodedSize, filesFromPaste, loadOf, prepareFile } from '../lib/attachments';
import type { Prepared } from '../lib/attachments';
import { carriesFiles } from '../lib/dropGuard';
import { shouldRestoreInputFocus } from '../lib/inputFocus';
import { showsVisionWarning } from '../lib/modelVision';
import { visibleEscapes } from '../lib/visibleEscapes';
import { IS_MAC, shortcutLabel } from '../lib/platform';
import type { AttachmentInput, CommandCatalog, QueueMode } from '../lib/types';
import { AttachmentChips } from './AttachmentChips';
import type { ChipItem } from './AttachmentChips';
import { CommandMenu, commandMenuItemId, COMMAND_MENU_ID } from './CommandMenu';
import { McpBar } from './McpBar';
import { clearDraftIfSame, draftOnKeyChange, loadDraft, NEW_DRAFT_KEY, readCache, saveDraft } from '../lib/conversationCache';
import { defaultSizeStorage } from '../lib/zoom';

/** The unsent text is saved this long after the last change (and at once on a key change, unmount, or page hide). */
const DRAFT_SAVE_MS = 400;

/** The shortcut hint, kept as the input's tooltip so the placeholder stays one line at every panel width. */
const SHORTCUT_HINT = `${shortcutLabel('agent')} to focus, Esc to return`;

/** No customs: the menu still shows the built-ins while the catalog loads. */
const EMPTY_CATALOG: CommandCatalog = { commands: [], project_dir: null, global_dir: null };

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
  /** Show the hint as the input's placeholder instead of a row under it. No current route sets it; kept for later use. */
  hintInline: boolean;
  hasPendingApproval: boolean;
  /** Enter may approve the pending card only when armed (500ms visible in the chat body). Escape denies regardless. */
  canApprove: boolean;
  /** The agent is mid-turn. The button becomes Stop, and the field's border shows a light while it lasts. */
  busy: boolean;
  /** An integration is running: the input takes no text and sends nothing until it answers. Read-only, not disabled, so it keeps focus. */
  locked: boolean;
  /** The agent's question is waiting for the user's answer. A blocker for taking focus back (see inputFocus.ts). */
  questionPending?: boolean;
  /** Bypass (approve-all): the agent runs commands without asking. Shown above the input in chat mode; Shift+Tab flips it too. */
  autoApprove: boolean;
  /** Bypass was switched on in another shell level, so it is off in the shell on top. The chip says manual until it is switched on here. */
  autoSuspended?: boolean;
  onToggleAutoApprove: () => void;
  /** Bumped by the parent to move focus into the input (or the restart button, when the shell exited). */
  focusToken: number;
  /** Reports when the input gains or loses focus. */
  onFocusChange: (focused: boolean) => void;
  /** Fetches the custom slash commands. Called fresh on every menu opening; built-ins show without it. */
  loadCommands: () => Promise<CommandCatalog>;
  /** The shell exited: starts a new session. */
  onRestart: () => void;
  restarting: boolean;
  /** Resolves to an error message, or null on success. The text and the attachments are kept on error. */
  onAgent: (text: string, attachments: AttachmentInput[]) => Promise<string | null>;
  /** Whether the selected model takes images (Model.vision): false shows a note under the attachments. Null or true: no note. */
  modelVision?: boolean | null;
  /** The messages queued while the agent works, in order. Shown above the field in chat mode, each removable. */
  queue: QueuedMessage[];
  queueMode: QueueMode;
  /** A send now is queued rather than sent (the agent is mid-turn, and no side command runs). Only sets the placeholder. */
  queueable: boolean;
  /** Takes a queued message back: returns it, or null when it is gone. Its text goes back into the input, selected. */
  onUnqueue: (id: number) => QueuedMessage | null;
  /**
   * Queued messages the app took back without a click (a Stop, or a restart): they come into the input as an unqueue
   * does. Bumping `restoreTick` runs it; `takeReturned` gives the messages.
   */
  restoreTick?: number;
  takeReturned?: () => QueuedMessage[];
  /** Follow-up prompts for the latest reply, as buttons under the field. Shown in chat mode only when the agent is idle. */
  suggestions: string[];
  /** A suggestion was clicked: it is sent as a plain message (not through the command parser). */
  onSuggestion: (text: string) => void;
  onInterrupt: () => void;
  onApproveFirst: () => void;
  onDenyFirst: () => void;
  /** Escape with nothing to deny: focus goes back to the terminal. */
  onClose: () => void;
  /** The conversation the unsent text belongs to (its id, or NEW_DRAFT_KEY before the id is known). A change swaps the draft. */
  draftKey?: string;
  /** Bumped when the live conversation was deleted: its text is dropped on the key change, not saved under the deleted id. */
  draftDiscard?: number;
}

export function InputBar(props: Props) {
  const { mode, hint, hintInline, hasPendingApproval, busy, focusToken } = props;
  const convKey = props.draftKey ?? NEW_DRAFT_KEY;
  const discardToken = props.draftDiscard ?? 0;
  const [text, setText] = useState(() => loadDraft(defaultSizeStorage(), convKey));
  // The latest text, for the steps that run outside a render. `shownKey` is the conversation the text belongs to.
  const textRef = useRef(text);
  const shownKey = useRef(convKey);
  // The text not saved yet, with its key, and the timer that saves it.
  const unsaved = useRef<{ key: string; text: string } | null>(null);
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const discardSeen = useRef(discardToken);
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
  // The autocomplete menu: the catalog (null until the first opening fetches it), the selected row, and whether
  // the user shut the menu for this text. The caret lands at the end after an accept, once the text commits.
  const [catalog, setCatalog] = useState<CommandCatalog | null>(null);
  const [menuIndex, setMenuIndex] = useState(0);
  const [menuDismissed, setMenuDismissed] = useState(false);
  const placeCaretEnd = useRef(false);
  // Bumped when a queued message comes back into the input: the input takes focus and its text is selected.
  const [restoreToken, setRestoreToken] = useState(0);

  /** Saves the unsaved text now (under the key it belongs to), and stops the pending save. */
  const flushDraft = () => {
    if (saveTimer.current !== null) {
      clearTimeout(saveTimer.current);
      saveTimer.current = null;
    }
    const pending = unsaved.current;
    unsaved.current = null;
    if (pending !== null) {
      saveDraft(defaultSizeStorage(), pending.key, pending.text);
    }
  };

  /** Sets the input's text: typed, or restored. It is saved for the conversation shown, shortly after. */
  const updateText = (next: string) => {
    textRef.current = next;
    setText(next);
    unsaved.current = { key: shownKey.current, text: next };
    if (saveTimer.current !== null) {
      clearTimeout(saveTimer.current);
    }
    saveTimer.current = setTimeout(flushDraft, DRAFT_SAVE_MS);
  };

  /**
   * The conversation changed: the text is saved under the one it belonged to, and the new one's draft is shown. No focus
   * is moved. Runs after the render that changed the key, so the text is still the old conversation's.
   */
  useEffect(() => {
    const discard = discardToken !== discardSeen.current;
    discardSeen.current = discardToken;
    const prevKey = shownKey.current;
    if (prevKey === convKey) {
      return;
    }
    const storage = defaultSizeStorage();
    const next = draftOnKeyChange(readCache(storage), prevKey, convKey, textRef.current, discard);
    if (saveTimer.current !== null) {
      clearTimeout(saveTimer.current);
      saveTimer.current = null;
    }
    unsaved.current = null;
    if (next.save !== null) {
      saveDraft(storage, next.save.key, next.save.text);
    }
    shownKey.current = convKey;
    textRef.current = next.load;
    setText(next.load);
  }, [convKey, discardToken]);

  // Saves what is still unsaved when the page goes away or the bar unmounts.
  useEffect(() => {
    window.addEventListener('pagehide', flushDraft);
    window.addEventListener('beforeunload', flushDraft);
    return () => {
      window.removeEventListener('pagehide', flushDraft);
      window.removeEventListener('beforeunload', flushDraft);
      flushDraft();
    };
  }, []);

  // The menu's rows: built-ins at once, customs when the catalog loads. Visible while the whole text is a slash
  // prefix, unless dismissed, and there is a row to show.
  const query = menuQuery(text);
  const menuItems = filterCommandMenu(commandMenuItems(catalog ?? EMPTY_CATALOG), query ?? '');
  const menuOpen = mode === 'chat' && !props.locked && query !== null && !menuDismissed && menuItems.length > 0;
  const selectedIndex = menuItems.length === 0 ? 0 : menuIndex % menuItems.length;

  // Fetches the customs once per slash session (the transition into a command token, not the menu
  // opening, which can shut when a filter has no matches). A failure leaves the built-ins: the menu stays
  // useful, and the next session tries again. Typing within a session neither retriggers nor cancels.
  const inCommandToken = mode === 'chat' && query !== null;
  const loadCommandsRef = useRef(props.loadCommands);
  loadCommandsRef.current = props.loadCommands;
  useEffect(() => {
    if (!inCommandToken) {
      return;
    }
    let live = true;
    loadCommandsRef
      .current()
      .then((c) => {
        if (live) {
          setCatalog(c);
        }
      })
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [inCommandToken]);

  // After an accept commits, the caret goes to the end of the new text. Focus is kept throughout.
  useEffect(() => {
    if (!placeCaretEnd.current) {
      return;
    }
    placeCaretEnd.current = false;
    const el = inputRef.current;
    if (el) {
      el.focus();
      el.setSelectionRange(text.length, text.length);
    }
  }, [text]);

  // A queued message came back: the input has focus and its whole text selected, so typing replaces it. Runs after the
  // text is committed (the click on its remove button did not move focus, see the button's mousedown).
  useEffect(() => {
    if (restoreToken === 0) {
      return;
    }
    const el = inputRef.current;
    if (el) {
      el.focus();
      el.select();
    }
  }, [restoreToken]);

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

  // Focus the bar had when the lock began. The lock itself never moves focus (the field is read-only), but if something
  // took it to the page body during the lock, the bar takes it back once the lock ends, unless a blocker is up.
  const focusedAtLock = useRef(false);
  useEffect(() => {
    if (props.locked) {
      focusedAtLock.current = document.activeElement === inputRef.current;
      return;
    }
    if (!focusedAtLock.current) {
      return;
    }
    focusedAtLock.current = false;
    const active = document.activeElement;
    if (
      shouldRestoreInputFocus({
        inputWasFocused: true,
        focusLost: active === null || active === document.body,
        dialogOpen: document.querySelector('[role="dialog"]') !== null,
        questionPending: props.questionPending ?? false,
      })
    ) {
      inputRef.current?.focus();
    }
  }, [props.locked]);

  // Bypass is on in this shell only when it was switched on here: a suspended one reads manual for the shell on top.
  const bypassOn = props.autoApprove && !props.autoSuspended;
  const bypassSuspended = props.autoApprove && props.autoSuspended === true;
  const approvalTip = bypassOn
    ? 'Bypass is ON: the agent runs every command without asking, even ones your deny rules or the built-in checks would block. It resets when Watt restarts. Click to turn off.'
    : bypassSuspended
      ? 'Bypass was turned on in another shell, so it is off in this one: every command here asks for your approval. Click to turn it on here.'
      : 'manual: each command asks for approval. Click (or Shift+Tab) to bypass approval for this session.';
  const toggleClass = bypassOn ? 'bar-approval-toggle is-bypass' : bypassSuspended ? 'bar-approval-toggle is-suspended' : 'bar-approval-toggle';

  /**
   * The approval chip. The chat bar always shows it. The collapsed and exited bars show it only while bypass is on here
   * (`inline`: one row with the rest of the bar), so a bypass left on can still be seen and turned off.
   */
  const approvalChip = (inline: boolean) => (
    <div className={inline ? 'bar-approval is-inline' : 'bar-approval'} data-tip={approvalTip}>
      <span className="bar-approval-label">approval:</span>{' '}
      <button
        type="button"
        className={toggleClass}
        aria-pressed={bypassOn}
        data-tip={approvalTip}
        disabled={props.locked}
        onMouseDown={(e) => e.preventDefault()}
        onClick={props.onToggleAutoApprove}
      >
        {bypassOn ? BYPASS_LABEL : 'manual'}
      </button>
    </div>
  );

  if (mode === 'exited') {
    return (
      <div className="bar bar-exited">
        <span className="bar-notice">{hint}</span>
        {bypassOn && approvalChip(true)}
        <button ref={restartRef} type="button" className="bar-restart" disabled={props.restarting} onClick={props.onRestart}>
          Start new session
        </button>
      </div>
    );
  }

  if (mode === 'collapsed') {
    return (
      <div className="bar bar-collapsed" title={`${shortcutLabel('agent')} to message the agent`}>
        <span className="bar-hint">{hint}</span>
        {bypassOn && approvalChip(true)}
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

  /**
   * Takes a queued message back. Its text goes into the input (before any text already there), selected, and its files
   * come back as drafts, so nothing is lost. Nothing is sent.
   */
  function unqueue(id: number) {
    const removed = props.onUnqueue(id);
    if (removed !== null) {
      restoreQueued([removed]);
    }
  }

  /** Puts queued messages back into the input: their texts (in order, before any text there), selected, and their files as drafts. */
  function restoreQueued(items: QueuedMessage[]) {
    const restored: Draft[] = items.flatMap((q) => q.attachments).map((input) => {
      draftKey.current += 1;
      return { key: draftKey.current, name: input.name, bytes: decodedSize(input.data_b64), input };
    });
    commit([...draftsRef.current, ...restored]);
    const text = items.map((q) => q.text).filter((t) => t !== '').join('\n\n');
    updateText(restoreText(text, textRef.current));
    setError(null);
    setRestoreToken((t) => t + 1);
  }

  // Queued messages the app took back (a Stop, a restart) come into the input here. Runs once per tick.
  const restoreSeen = useRef(props.restoreTick ?? 0);
  useEffect(() => {
    const tick = props.restoreTick ?? 0;
    if (tick === restoreSeen.current) {
      return;
    }
    restoreSeen.current = tick;
    const items = props.takeReturned?.() ?? [];
    if (items.length > 0) {
      restoreQueued(items);
    }
  }, [props.restoreTick]);

  /** Takes a menu row: the text becomes `/name` (plus a space when it takes arguments) and the menu shuts, so it does not reopen for the new text. */
  function acceptItem(item: CommandMenuItem) {
    placeCaretEnd.current = true;
    updateText(acceptMenuItem(item));
    setMenuIndex(0);
    setMenuDismissed(true);
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
    // The conversation and the text as they were sent: the await below may end in another conversation, or with more typed.
    const sentKey = shownKey.current;
    setSending(true);
    let err: string | null;
    try {
      err = await props.onAgent(message ?? '', ready);
    } finally {
      setSending(false);
    }
    // A local slash command returns null (it ran), so the bar clears. A refused send keeps its text and files, whether it
    // was a plain message or a slash-looking one (`/var/log/...`, a template), so the user can retry.
    if (err) {
      setError(err);
      return;
    }
    history.current.record(text);
    clearSent(sentKey, text);
    commit(draftsRef.current.filter((d) => !sent.has(d.key)));
    setError(null);
  }

  /**
   * After a send (or a slash command that ran): the input is cleared only while it still holds what was sent, in the same
   * conversation. Text typed meanwhile stays. The saved draft of the sent text is removed in either case.
   */
  function clearSent(key: string, sentText: string) {
    if (shownKey.current === key && textRef.current === sentText) {
      updateText('');
      flushDraft();
      return;
    }
    clearDraftIfSame(defaultSizeStorage(), key, sentText);
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
    if (props.locked) {
      e.preventDefault();
      return;
    }
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
    data: d.input?.kind === 'image' ? { mime: d.input.mime, b64: d.input.data_b64 } : undefined,
    processing: d.input === null,
  }));

  function onKeyDown(e: ReactKeyboardEvent<HTMLTextAreaElement>) {
    // Locked: no key acts here (the field used to be disabled, which took its keys too). Focus stays, the keys do nothing.
    if (props.locked || e.nativeEvent.isComposing) {
      return;
    }
    // The menu first: plain keys move, accept or dismiss it. A Ctrl/Meta/Alt chord is not the menu's: it falls
    // through to the bar's own handling (Ctrl-C still interrupts), as does Shift+Enter (a newline). Shift+Tab
    // toggles bypass (Ctrl+Shift+Tab stays none, so focus moves on). Escape dismisses the menu instead of
    // denying or closing.
    if (menuOpen && !e.ctrlKey && !e.metaKey && !e.altKey) {
      const intent = menuKeyIntent(e.key, { shift: e.shiftKey, ctrl: false, meta: false, alt: false });
      if (intent === 'next' || intent === 'prev') {
        e.preventDefault();
        setMenuIndex(moveMenuIndex(selectedIndex, intent === 'next' ? 1 : -1, menuItems.length));
        return;
      }
      if (intent === 'accept') {
        e.preventDefault();
        const item = menuItems[selectedIndex];
        if (item === undefined) {
          return;
        }
        // Enter on a fully typed command runs it; Tab (or Enter on a partial name) completes it.
        if (e.key === 'Enter' && menuAcceptRuns(text, item)) {
          if (!sending && !props.locked) {
            void submit();
          }
          return;
        }
        acceptItem(item);
        return;
      }
      if (intent === 'dismiss') {
        e.preventDefault();
        setMenuDismissed(true);
        return;
      }
    }
    const intent = barKeyIntent(
      e.key,
      { ctrl: e.ctrlKey, shift: e.shiftKey },
      // Staged attachments are not an empty input: Enter sends them rather than approving a pending call.
      {
        inputEmpty: text === '' && drafts.length === 0,
        hasPendingApproval,
        canApprove: props.canApprove,
        // Off macOS, Ctrl+C with text selected copies it (see inputState.ts). The selection is read from the field itself.
        hasSelection: e.currentTarget.selectionStart !== e.currentTarget.selectionEnd,
        mac: IS_MAC,
      },
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
      case 'toggle_approval':
        // Shift+Tab flips bypass without moving focus. Locked and non-chat modes ignore it.
        e.preventDefault();
        if (mode === 'chat' && !props.locked) {
          props.onToggleAutoApprove();
        }
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
          updateText(older);
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
          updateText(newer);
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
  const placeholder = inlineHint ?? agentPlaceholder(props.queueable && !props.locked);

  return (
    <div
      className={dragging ? `bar bar-${mode} is-dragging` : `bar bar-${mode}`}
      onDragEnter={onDragOver}
      onDragOver={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDrop}
    >
      {mode === 'chat' && approvalChip(false)}
      {mode === 'chat' && props.queue.length > 0 && (
        <div className="bar-queue">
          <span className="bar-queue-head">{queueHeader(props.queueMode)}</span>
          {props.queue.map((q) => (
            <div key={q.id} className="bar-queue-item">
              <span className="bar-queue-text" data-tip={q.text === '' ? 'Attachments only' : q.text} data-tip-clip="">
                {q.text === '' ? 'Attachments only' : q.text}
              </span>
              {q.attachments.length > 0 && <span className="bar-queue-note">{queuedAttachmentNote(q.attachments.length)}</span>}
              {/* mousedown keeps focus where it is; the click then puts the text in the input and selects it. */}
              <button
                type="button"
                className="bar-queue-remove"
                aria-label="Remove queued message"
                title="Remove queued message"
                disabled={props.locked}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => unqueue(q.id)}
              >
                ×
              </button>
            </div>
          ))}
        </div>
      )}
      {drafts.length > 0 && <AttachmentChips items={chips} onRemove={removeDraft} />}
      {showsVisionWarning(props.modelVision, drafts.filter((d) => d.input?.kind === 'image').length) && (
        <div className="bar-vision">This model may not accept images; they'd be left out.</div>
      )}
      {mode === 'chat' && <McpBar />}
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
          className={props.locked ? 'bar-input is-locked' : 'bar-input'}
          rows={1}
          value={text}
          placeholder={placeholder}
          title={SHORTCUT_HINT}
          aria-label="Message the agent"
          spellCheck={false}
          autoComplete="off"
          readOnly={props.locked}
          onChange={(e) => {
            updateText(e.currentTarget.value);
            setMenuIndex(0);
            setMenuDismissed(false);
            setError(null);
          }}
          onKeyDown={onKeyDown}
          onPaste={onPaste}
          onFocus={() => props.onFocusChange(true)}
          onBlur={() => props.onFocusChange(false)}
          aria-expanded={menuOpen}
          aria-controls={menuOpen ? COMMAND_MENU_ID : undefined}
          aria-activedescendant={menuOpen ? commandMenuItemId(selectedIndex) : undefined}
        />
      </div>
      {menuOpen && <CommandMenu items={menuItems} index={selectedIndex} onAccept={acceptItem} onHover={setMenuIndex} />}
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
      {mode === 'chat' && !busy && !props.locked && props.suggestions.length > 0 && (
        <div className="bar-suggest">
          <span className="bar-suggest-head">Suggested:</span>
          {props.suggestions.map((s) => (
            <button
              key={s}
              type="button"
              className="bar-suggest-chip"
              data-tip={s}
              data-tip-clip=""
              aria-label={s}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => props.onSuggestion(s)}
            >
              {s}
            </button>
          ))}
        </div>
      )}
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

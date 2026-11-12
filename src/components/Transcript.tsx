import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { MouseEvent, ReactNode, UIEvent, WheelEvent } from 'react';
import { copyText } from '../lib/clipboard';
import { previewJson } from '../lib/format';
import { isAtBottom, shouldFollowAfterSend } from '../lib/stickScroll';
import { approvalTitle } from '../lib/transcript';
import type { TranscriptItem, UserAttachment } from '../lib/transcript';
import { formatSize } from '../lib/attachments';
import { safeText } from '../lib/historyView';
import { askedBecause, decisionLabel, decisionTooltip } from '../lib/approvalReasons';
import { capDisplay, displayText, sendKeysDisplay } from '../lib/keysDisplay';
import { interactiveHeader } from '../lib/interactive';
import { isDeclinedIntegration } from '../lib/integrate';
import { groupTranscript, toolGroupSummary } from '../lib/toolGroups';
import type { GroupItem, InteractiveGroup, TranscriptEntry } from '../lib/toolGroups';
import { visibleEscapes } from '../lib/visibleEscapes';
import { Markdown } from './Markdown';
import { chatBodyShown } from '../lib/panelLayout';
import { settleTight } from '../lib/panelTight';
import type { Dock } from '../lib/panelLayout';
import wattMark from '../assets/watt-mark.png';

interface Props {
  items: TranscriptItem[];
  busy: boolean;
  /** The model is working and has not produced output yet. */
  thinking: boolean;
  open: boolean;
  onToggle: () => void;
  onApprove: (callId: string, approve: boolean) => void;
  onNewConversation: () => void;
  /** The dock the panel is in. With onDock, the header shows the four dock buttons, with this one marked. */
  dock?: Dock;
  onDock?: (dock: Dock) => void;
  /** Which view the panel body shows. History replaces the chat body (the header stays). Defaults to chat. */
  view?: 'chat' | 'history';
  /** With onToggleHistory, the header shows the History button, pressed while the history view is shown. */
  onToggleHistory?: () => void;
  /** The history view. Shown in place of the chat body while `view` is 'history' and the panel is open. */
  history?: ReactNode;
  /** A notice shown above the chat body (the settings banner). Shown with the chat view only, and it never moves the body's scroll. */
  banner?: ReactNode;
  /** A notice pinned above the chat body, above `banner` (the approve-all warning). Same placement and rules as `banner`. */
  warning?: ReactNode;
}

/** The agent conversation, docked above the input bar. Collapsible, scrollable, and capped by CSS. */
export function Transcript({
  items,
  busy,
  thinking,
  open,
  onToggle,
  onApprove,
  onNewConversation,
  dock,
  onDock,
  view = 'chat',
  onToggleHistory,
  history,
  banner = null,
  warning = null,
}: Props) {
  const bodyRef = useRef<HTMLDivElement>(null);
  const sectionRef = useRef<HTMLElement>(null);
  const entries = useMemo(() => groupTranscript(items), [items]);
  /** Whether the view follows new content. A ref, read in effects and handlers, so changing it does not re-render. */
  const following = useRef(true);
  /** The body the scroll state belongs to. A new body (the panel opened or remounted) starts following. */
  const shownBody = useRef<HTMLDivElement | null>(null);
  const lastItem = useRef<TranscriptItem | undefined>(undefined);
  /** The body's scrollTop as last seen, to tell the user scrolling up from content growing below. */
  const lastTop = useRef(0);
  const [showJump, setShowJump] = useState(false);
  /** The chat body is on screen. Switching to the history view and back remounts it, like closing and opening the panel. */
  const bodyShown = chatBodyShown(open, view);

  // The part above the body (the header and the banners) in px, published as --transcript-chrome. In a sized panel the transcript's
  // least size is this plus the body's floor (App.css), so the floor holds and the body is never clipped. With no body on screen
  // (the history view, the panel closed) there is no floor. The agent panel's tight flag is settled here too (panelTight.ts), since
  // the transcript is always on screen. Re-measured after each render, and a frame after a part above the body, or the panel, changes
  // size. The measure runs in that frame, so the style change does not trip the observer's loop limit.
  useLayoutEffect(() => {
    const section = sectionRef.current;
    if (!section) {
      return;
    }
    const agent = section.closest<HTMLElement>('.agent-panel');
    const settle = () => {
      const body = bodyRef.current;
      if (body && section.contains(body)) {
        const chrome = body.getBoundingClientRect().top - section.getBoundingClientRect().top;
        section.style.setProperty('--transcript-chrome', `${chrome}px`);
        section.style.removeProperty('--transcript-floor');
      } else {
        section.style.setProperty('--transcript-chrome', '0px');
        section.style.setProperty('--transcript-floor', '0px');
      }
      settleTight(agent);
    };
    settle();
    if (typeof ResizeObserver === 'undefined') {
      return;
    }
    const observer = new ResizeObserver(() => requestAnimationFrame(settle));
    if (agent) {
      observer.observe(agent);
      // The panel's other parts (the todo list, the context meter, the bar) change their own heights without a render: the meter's
      // breakdown opens on its own state. Their size changes settle the flag too.
      for (const child of Array.from(agent.children)) {
        observer.observe(child);
      }
    }
    for (const child of Array.from(section.children)) {
      if (child !== bodyRef.current) {
        observer.observe(child);
      }
    }
    return () => observer.disconnect();
  });

  // Keyed on the items array, not its length: a streamed delta replaces the last item, and the length stops changing at the cap.
  useLayoutEffect(() => {
    const prev = lastItem.current;
    const last = items[items.length - 1];
    lastItem.current = last;
    const body = bodyRef.current;
    if (!body) {
      shownBody.current = null;
      return;
    }
    if (body !== shownBody.current) {
      shownBody.current = body;
      following.current = true;
      setShowJump(false);
    }
    if (shouldFollowAfterSend(prev, last)) {
      following.current = true;
    }
    if (following.current) {
      body.scrollTop = body.scrollHeight;
      lastTop.current = body.scrollTop;
    }
  }, [items, bodyShown]);

  // A panel resize or dock change keeps the pin while following; when not following, being back at the bottom resumes it.
  useEffect(() => {
    const body = bodyRef.current;
    if (!body || typeof ResizeObserver === 'undefined') {
      return;
    }
    const observer = new ResizeObserver(() => {
      if (following.current) {
        body.scrollTop = body.scrollHeight;
        lastTop.current = body.scrollTop;
      } else if (isAtBottom(body)) {
        following.current = true;
        setShowJump(false);
      }
    });
    observer.observe(body);
    return () => observer.disconnect();
  }, [bodyShown]);

  const onScroll = (e: UIEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    const top = el.scrollTop;
    const movedUp = top < lastTop.current;
    lastTop.current = top;
    if (isAtBottom(el)) {
      following.current = true;
    } else if (movedUp) {
      // Only a scroll up stops following. Content growing below the view leaves the top where it was, and a streamed
      // chunk can grow it by more than the threshold before our own scroll-to-bottom event is handled.
      following.current = false;
    }
    setShowJump(!following.current);
  };

  // Catches a wheel up before the scroll it causes is handled, so a streamed delta in between cannot pull the view down.
  // Without overflow there is nothing to scroll back to, so following stays on.
  const onWheel = (e: WheelEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    if (e.deltaY < 0 && el.scrollHeight > el.clientHeight) {
      following.current = false;
      setShowJump(true);
    }
  };

  const jumpToLatest = (e: MouseEvent<HTMLButtonElement>) => {
    const body = bodyRef.current;
    if (!body) {
      return;
    }
    // The button unmounts on click. If it had keyboard focus, move focus to the body so it is not lost.
    const hadFocus = document.activeElement === e.currentTarget;
    following.current = true;
    body.scrollTop = body.scrollHeight;
    lastTop.current = body.scrollTop;
    setShowJump(false);
    if (hadFocus) {
      body.focus({ preventScroll: true });
    }
  };

  return (
    <section ref={sectionRef} className={`transcript${open ? ' open' : ''}`}>
      <div className="transcript-header">
        <button type="button" className="transcript-head" onClick={onToggle}>
          <img className="brand-mark" src={wattMark} width={212} height={256} alt="" aria-hidden="true" />
          <span>Agent</span>
          {busy && <span className="busy">working...</span>}
          {thinking && <span className="thinking">thinking...</span>}
          <span className="toggle">{open ? 'Hide' : 'Show'}</span>
        </button>
        {onDock && dock && <DockButtons dock={dock} onDock={onDock} />}
        {onToggleHistory && <HistoryButton pressed={view === 'history'} onClick={onToggleHistory} />}
        <button
          type="button"
          className="transcript-new"
          disabled={busy}
          onClick={onNewConversation}
          title={busy ? 'Wait for the agent to finish, or cancel it with Ctrl-C, first' : 'Clears the agent conversation'}
        >
          New conversation
        </button>
      </div>
      {open && view === 'history' && history}
      {/* Pinned: siblings above the body, so they never scroll away and the body's scroll and follow logic is untouched. */}
      {bodyShown && warning}
      {bodyShown && banner}
      {bodyShown && (
        <div className="transcript-body" ref={bodyRef} tabIndex={-1} onScroll={onScroll} onWheel={onWheel}>
          {items.length === 0 && <div className="empty">Ask the agent something, for example "what was the last command".</div>}
          {renderEntries(entries, onApprove)}
        </div>
      )}
      {bodyShown && showJump && <JumpToLatest busy={busy} onClick={jumpToLatest} />}
    </section>
  );
}

/** The floating control shown when the view is not at the bottom. It sits over the body, so it changes no layout. */
export function JumpToLatest({ busy, onClick }: { busy: boolean; onClick: (e: MouseEvent<HTMLButtonElement>) => void }) {
  const label = busy ? 'Follow the agent (resume auto-scroll)' : 'Scroll to bottom';
  return (
    <button type="button" className="jump-to-latest" aria-label={label} title={label} onClick={onClick}>
      <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
        <path d="M8 2.5v8.5" />
        <path d="M4.5 7.5L8 11l3.5-3.5" />
        <path d="M3 14h10" />
      </svg>
    </button>
  );
}

/** A run of tool calls, collapsed to one line. Uncontrolled, and keyed by its first item, so it stays open as it grows. */
function ToolGroup({ items, onApprove }: { items: GroupItem[]; onApprove: Props['onApprove'] }) {
  return (
    <details className="tool tool-group">
      <summary>{toolGroupSummary(items)}</summary>
      <div className="tool-group-items">
        {items.map((item) => (
          <Entry key={item.id} item={item} onApprove={onApprove} />
        ))}
      </div>
    </details>
  );
}

/** The transcript's entries as elements. Used for the body and, inside an interactive group, for the group's contents. */
function renderEntries(entries: TranscriptEntry[], onApprove: Props['onApprove']) {
  return entries.map((entry) => {
    switch (entry.kind) {
      case 'item':
        return <Entry key={entry.item.id} item={entry.item} onApprove={onApprove} />;
      case 'tools':
        return <ToolGroup key={entry.key} items={entry.items} onApprove={onApprove} />;
      case 'interactive':
        return <InteractiveGroupView key={entry.key} group={entry.group} onApprove={onApprove} />;
    }
  });
}

/**
 * A command the agent started and left running, and what the agent did for it. The wrapper has an accent rail while the
 * command runs; once it ends the rail goes and the header says how it ended. The header ticks once a second while running.
 */
function InteractiveGroupView({ group, onApprove }: { group: InteractiveGroup; onApprove: Props['onApprove'] }) {
  const running = group.ended === null;
  const now = useNow(running);
  const header = interactiveHeader(group, now);
  return (
    <section className={`interactive ${running ? 'interactive-running' : 'interactive-ended'}`}>
      <div className="interactive-header" role="group" aria-label={header.label}>
        <span className="interactive-state" aria-hidden="true">
          {header.state}
        </span>
        {header.command !== '' && (
          <span className="interactive-command" aria-hidden="true">
            {` · ${header.command}`}
          </span>
        )}
      </div>
      <div className="interactive-items">{renderEntries(group.entries, onApprove)}</div>
    </section>
  );
}

/** The current time, refreshed every second while `ticking`. Only the component that asks re-renders. */
function useNow(ticking: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!ticking) {
      return;
    }
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [ticking]);
  return now;
}

/** The send_keys rows' typed text is cut to this many characters in the summary. The full text is in the row's details. */
const SEND_KEYS_ROW_CHARS = 40;

/** A tool row's name. A send_keys row shows what it typed (decoded, capped) in place of the bare name. */
function toolName(item: Extract<TranscriptItem, { kind: 'tool' }>): string {
  if (item.name !== 'send_keys') {
    return item.name;
  }
  const typed = sendKeysDisplay(item.args);
  return typed === '' ? item.name : `send_keys "${capDisplay(typed, SEND_KEYS_ROW_CHARS)}"`;
}

/**
 * The attachments of a message the user sent, under its text: a 48px thumbnail for an image that has one, otherwise a chip with
 * the name and size (a document, or an image whose thumbnail was not stored). Names are shown as text only.
 */
function MessageAttachments({ items }: { items: UserAttachment[] }) {
  return (
    <div className="msg-attachments">
      {items.map((a, i) => {
        const name = safeText(a.name);
        if (a.kind === 'image' && a.thumb !== undefined) {
          return (
            <div key={i} className="msg-attach is-image">
              <img className="msg-attach-thumb" src={a.thumb} alt={name} title={name} />
            </div>
          );
        }
        return (
          <div key={i} className={a.kind === 'image' ? 'msg-attach is-placeholder' : 'msg-attach is-file'}>
            <span className="msg-attach-name" title={name}>
              {name}
            </span>
            <span className="msg-attach-size">{formatSize(a.bytes)}</span>
          </div>
        );
      })}
    </div>
  );
}

function Entry({ item, onApprove }: { item: TranscriptItem; onApprove: Props['onApprove'] }) {
  switch (item.kind) {
    case 'user':
      return (
        <div className="msg user">
          <span className="who">you</span>
          {item.text}
          {item.attachments !== undefined && <MessageAttachments items={item.attachments} />}
        </div>
      );
    case 'agent':
      return (
        <div className="msg agent">
          <Markdown text={item.text} />
          {!item.streaming && <CopyMarkdownButton text={item.text} />}
        </div>
      );
    case 'notice':
      return <div className={item.muted ? 'notice notice-muted' : 'notice'}>{item.text}</div>;
    // Closes an interactive group (see renderEntries); with no group open there is nothing to show.
    case 'record_end':
      return null;
    case 'tool': {
      // A declined integration request is its own status, not a failure: the user chose it.
      const declined = isDeclinedIntegration(item.name, item.decision);
      const status = declined ? 'declined' : item.status === 'running' ? 'running' : item.status === 'ok' ? 'ok' : 'failed';
      return (
        <details className={`tool tool-${status}`}>
          <summary>
            {toolName(item)}
            {item.decision !== null && !declined && (
              <span
                className={`decision decision-${item.decision}`}
                title={decisionTooltip(item.decision, item.reason) ?? undefined}
              >
                {` ${decisionLabel(item.decision, item.reason)}`}
              </span>
            )}
            : {status}
          </summary>
          <pre>{previewJson({ args: item.args, result: item.result })}</pre>
        </details>
      );
    }
    case 'approval': {
      const why = askedBecause(item.reason);
      return (
        <div className={`approval approval-${item.state}`}>
          <div className="approval-title">{approvalTitle(item.name)}</div>
          {item.location !== null && <div className="approval-location">on {item.location}</div>}
          {/* A send_keys card shows the decoded input (the text and the keys), escaped; the other cards show the command. */}
          {item.name === 'send_keys' ? (
            <code className="approval-text approval-keys">{displayText(item.display ?? item.text)}</code>
          ) : (
            <code className="approval-text">{visibleEscapes(item.text)}</code>
          )}
          {why !== null && <div className="approval-reason">{why}</div>}
          {item.state === 'pending' && (
            <div className="approval-actions">
              <button type="button" className="approve" onClick={() => onApprove(item.callId, true)}>
                Approve (Enter)
              </button>
              <button type="button" className="deny" onClick={() => onApprove(item.callId, false)}>
                Deny (Esc)
              </button>
            </div>
          )}
          {item.state === 'answering' && <div className="approval-note">{item.approve ? 'Approving...' : 'Denying...'}</div>}
          {item.state === 'resolved' && <div className="approval-note">Resolved</div>}
        </div>
      );
    }
  }
}

/** How long the check mark stays after a copy. */
const COPIED_MS = 1500;

/**
 * Copies a finished agent message as the model wrote it (its markdown source). Shown only once the message is complete.
 * The icon swaps to a check mark for a moment; the button keeps its size, so the swap does not move the layout.
 */
function CopyMarkdownButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(timer.current), []);

  const onClick = async () => {
    const ok = await copyText(text, { clipboard: navigator.clipboard, document });
    if (!ok) {
      return;
    }
    setCopied(true);
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setCopied(false), COPIED_MS);
  };

  return (
    <div className="msg-actions">
      <button
        type="button"
        className={`copy-md${copied ? ' copied' : ''}`}
        onClick={onClick}
        title="Copy as Markdown"
        aria-label="Copy as Markdown"
      >
        {copied ? <CheckIcon /> : <CopyIcon />}
      </button>
    </div>
  );
}

function CopyIcon() {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.25" aria-hidden="true">
      <rect x="5.5" y="5.5" width="8" height="8" rx="1" />
      <path d="M10.5 5.5V3.5a1 1 0 0 0-1-1h-6a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2" />
    </svg>
  );
}

function CheckIcon() {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
      <path d="M3 8.5l3 3 7-7" />
    </svg>
  );
}

/** Toggles the History view in the panel body. Pressed while that view is shown. */
function HistoryButton({ pressed, onClick }: { pressed: boolean; onClick: () => void }) {
  return (
    <button
      type="button"
      id="history-button"
      className="history-button"
      aria-label="Conversation history"
      aria-pressed={pressed}
      title="Conversation history"
      onClick={onClick}
    >
      <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.25" aria-hidden="true">
        <path d="M2.5 8a5.5 5.5 0 1 0 1.6-3.9" />
        <path d="M2.5 2.5v2.6h2.6" />
        <path d="M8 5v3.2l2.2 1.4" />
      </svg>
    </button>
  );
}

/** Where each dock button sends the panel, in the order the buttons show. */
const DOCK_BUTTONS: Dock[] = ['left', 'top', 'bottom', 'right'];

/** Four icon buttons. The current dock's button is pressed (aria-pressed), and drawn accented. */
function DockButtons({ dock, onDock }: { dock: Dock; onDock: (dock: Dock) => void }) {
  return (
    <div className="dock-buttons" role="group" aria-label="Agent panel position">
      {DOCK_BUTTONS.map((d) => (
        <button
          key={d}
          type="button"
          className="dock-button"
          aria-label={`Dock ${d}`}
          aria-pressed={d === dock}
          title={`Dock ${d}`}
          onClick={() => onDock(d)}
        >
          <DockIcon dock={d} />
        </button>
      ))}
    </div>
  );
}

/** A window frame with the side the panel docks to filled in. */
function DockIcon({ dock }: { dock: Dock }) {
  const fill: Record<Dock, string> = {
    left: 'M1.5 1.5h4v11h-4z',
    top: 'M1.5 1.5h11v4h-11z',
    bottom: 'M1.5 10.5h11v4h-11z',
    right: 'M10.5 1.5h4v11h-4z',
  };
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.25" aria-hidden="true">
      <rect x="1.5" y="1.5" width="13" height="13" rx="1" />
      <path d={fill[dock]} fill="currentColor" stroke="none" />
    </svg>
  );
}

import { useEffect, useMemo, useRef, useState } from 'react';
import { copyText } from '../lib/clipboard';
import { previewJson } from '../lib/format';
import { approvalTitle } from '../lib/transcript';
import type { TranscriptItem } from '../lib/transcript';
import { groupTranscript, toolGroupSummary } from '../lib/toolGroups';
import type { GroupItem } from '../lib/toolGroups';
import { visibleEscapes } from '../lib/visibleEscapes';
import { Markdown } from './Markdown';
import type { Dock } from '../lib/panelLayout';

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
}

/** The agent conversation, docked above the input bar. Collapsible, scrollable, and capped by CSS. */
export function Transcript({ items, busy, thinking, open, onToggle, onApprove, onNewConversation, dock, onDock }: Props) {
  const bodyRef = useRef<HTMLDivElement>(null);
  const entries = useMemo(() => groupTranscript(items), [items]);

  useEffect(() => {
    const body = bodyRef.current;
    if (body) {
      body.scrollTop = body.scrollHeight;
    }
  }, [items.length, open]);

  return (
    <section className={`transcript${open ? ' open' : ''}`}>
      <div className="transcript-header">
        <button type="button" className="transcript-head" onClick={onToggle}>
          <span>Agent</span>
          {busy && <span className="busy">working...</span>}
          {thinking && <span className="thinking">thinking...</span>}
          <span className="toggle">{open ? 'Hide' : 'Show'}</span>
        </button>
        {onDock && dock && <DockButtons dock={dock} onDock={onDock} />}
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
      {open && (
        <div className="transcript-body" ref={bodyRef}>
          {items.length === 0 && <div className="empty">Ask the agent something, for example "what was the last command".</div>}
          {entries.map((entry) =>
            entry.kind === 'item' ? (
              <Entry key={entry.item.id} item={entry.item} onApprove={onApprove} />
            ) : (
              <ToolGroup key={entry.key} items={entry.items} onApprove={onApprove} />
            ),
          )}
        </div>
      )}
    </section>
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

function Entry({ item, onApprove }: { item: TranscriptItem; onApprove: Props['onApprove'] }) {
  switch (item.kind) {
    case 'user':
      return (
        <div className="msg user">
          <span className="who">you</span>
          {item.text}
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
      return <div className="notice">{item.text}</div>;
    case 'tool': {
      const status = item.status === 'running' ? 'running' : item.status === 'ok' ? 'ok' : 'failed';
      return (
        <details className={`tool tool-${status}`}>
          <summary>
            {item.name}
            {item.decision !== null && <span className={`decision decision-${item.decision}`}> {item.decision}</span>}: {status}
          </summary>
          <pre>{previewJson({ args: item.args, result: item.result })}</pre>
        </details>
      );
    }
    case 'approval':
      return (
        <div className={`approval approval-${item.state}`}>
          <div className="approval-title">{approvalTitle(item.name)}</div>
          <code className="approval-text">{visibleEscapes(item.text)}</code>
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

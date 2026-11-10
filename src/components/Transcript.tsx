import { useEffect, useMemo, useRef } from 'react';
import { previewJson } from '../lib/format';
import { approvalTitle } from '../lib/transcript';
import type { TranscriptItem } from '../lib/transcript';
import { groupTranscript, toolGroupSummary } from '../lib/toolGroups';
import type { GroupItem } from '../lib/toolGroups';
import { visibleEscapes } from '../lib/visibleEscapes';

interface Props {
  items: TranscriptItem[];
  busy: boolean;
  /** The model is working and has not produced output yet. */
  thinking: boolean;
  open: boolean;
  onToggle: () => void;
  onApprove: (callId: string, approve: boolean) => void;
  onNewConversation: () => void;
}

/** The agent conversation, docked above the input bar. Collapsible, scrollable, and capped by CSS. */
export function Transcript({ items, busy, thinking, open, onToggle, onApprove, onNewConversation }: Props) {
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
      {items.map((item) => (
        <Entry key={item.id} item={item} onApprove={onApprove} />
      ))}
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
      return <div className="msg agent">{item.text}</div>;
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

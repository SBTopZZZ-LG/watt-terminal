import { useEffect, useRef } from 'react';
import { previewJson } from '../lib/format';
import type { TranscriptItem } from '../lib/transcript';

interface Props {
  items: TranscriptItem[];
  busy: boolean;
  open: boolean;
  onToggle: () => void;
  onApprove: (callId: string, approve: boolean) => void;
}

/** The agent conversation, docked above the input bar. Collapsible, scrollable, and capped by CSS. */
export function Transcript({ items, busy, open, onToggle, onApprove }: Props) {
  const bodyRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const body = bodyRef.current;
    if (body) {
      body.scrollTop = body.scrollHeight;
    }
  }, [items.length, open]);

  return (
    <section className={`transcript${open ? ' open' : ''}`}>
      <button type="button" className="transcript-head" onClick={onToggle}>
        <span>Agent</span>
        {busy && <span className="busy">working...</span>}
        <span className="toggle">{open ? 'Hide' : 'Show'}</span>
      </button>
      {open && (
        <div className="transcript-body" ref={bodyRef}>
          {items.length === 0 && <div className="empty">Ask the agent something, for example "what was the last command".</div>}
          {items.map((item) => (
            <Entry key={item.id} item={item} onApprove={onApprove} />
          ))}
        </div>
      )}
    </section>
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
            {item.name}: {status}
          </summary>
          <pre>{previewJson({ args: item.args, result: item.result })}</pre>
        </details>
      );
    }
    case 'approval':
      return (
        <div className={`approval approval-${item.state}`}>
          <div className="approval-title">The agent wants to run a command</div>
          <code className="approval-text">{item.text}</code>
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

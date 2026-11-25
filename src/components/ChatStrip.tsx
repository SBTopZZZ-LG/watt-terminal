import type { KeyboardEvent, MouseEvent } from 'react';
import type { ChatBadges } from '../lib/chats';
import { nextTabId } from '../lib/tabKeys';

export interface ChatStripItem {
  id: string;
  /** The chat's name ("Chat N", see chats.ts). */
  label: string;
  badges: ChatBadges;
}

export interface ChatStripProps {
  /** The open chats, in strip order. The strip is shown only with two or more. */
  chats: readonly ChatStripItem[];
  /** The shown chat. */
  activeId: string;
  onSelect: (id: string) => void;
  /** Asked for by the ×. The App refuses it for a busy chat (with a notice) and for the last chat. */
  onClose: (id: string) => void;
}

/** A click or a press on a chat or its × keeps the focus where it was. */
function keepFocus(e: MouseEvent): void {
  e.preventDefault();
}

/** What a chat's dots say, in words, for its accessible name. */
function stateWords(b: ChatBadges): string[] {
  const words: string[] = [];
  if (b.busy) {
    words.push('working');
  }
  if (b.approval) {
    words.push('needs approval');
  }
  if (b.question) {
    words.push('has a question');
  }
  return words;
}

/**
 * The chats, above the agent panel's transcript. Each pill names its chat and shows a dot for a turn running, an approval
 * waiting, or a question waiting. The shown one is marked. Arrow keys, Home and End move between them (the WAI-ARIA tabs
 * pattern, as TabStrip does). The × closes a chat; it is marked disabled while the chat runs.
 */
export function ChatStrip({ chats, activeId, onSelect, onClose }: ChatStripProps) {
  const ids = chats.map((c) => c.id);

  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>, id: string) => {
    const next = nextTabId(ids, id, e.key);
    if (next === null) {
      return;
    }
    e.preventDefault();
    onSelect(next);
    document.querySelector<HTMLElement>(`[data-chat-id="${next}"]`)?.focus();
  };

  return (
    <div className="chat-tabs" role="tablist" aria-label="Chats">
      {chats.map((c) => {
        const selected = c.id === activeId;
        const words = stateWords(c.badges);
        return (
          <div className="chat-tab-cell" key={c.id}>
            <button
              type="button"
              role="tab"
              aria-selected={selected}
              tabIndex={selected ? 0 : -1}
              data-chat-id={c.id}
              className={`chat-tab${selected ? ' is-active' : ''}`}
              aria-label={[c.label, ...words].join(', ')}
              onMouseDown={keepFocus}
              onClick={() => onSelect(c.id)}
              onKeyDown={(e) => onKeyDown(e, c.id)}
            >
              {c.badges.busy && <span className="chat-tab-dot is-busy" aria-hidden="true" />}
              {c.badges.approval && <span className="chat-tab-dot is-approval" aria-hidden="true" />}
              {c.badges.question && <span className="chat-tab-dot is-question" aria-hidden="true" />}
              <span className="chat-tab-label">{c.label}</span>
            </button>
            <button
              type="button"
              className="chat-tab-close"
              aria-label={`Close ${c.label}`}
              aria-disabled={c.badges.busy ? true : undefined}
              data-tip={c.badges.busy ? 'Stop its turn first' : 'Close chat'}
              onMouseDown={keepFocus}
              onClick={() => onClose(c.id)}
            >
              ×
            </button>
          </div>
        );
      })}
    </div>
  );
}

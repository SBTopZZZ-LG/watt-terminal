import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { ChatStrip } from './ChatStrip';
import type { ChatStripItem } from './ChatStrip';

const idle = { busy: false, approval: false, question: false };

const chats: ChatStripItem[] = [
  { id: 'main', label: 'Chat 1', badges: idle },
  { id: 'c2', label: 'Chat 2', badges: { busy: true, approval: true, question: false } },
];

const html = renderToStaticMarkup(<ChatStrip chats={chats} activeId="main" onSelect={() => undefined} onClose={() => undefined} />);

describe('ChatStrip markup', () => {
  it('is a tablist with one tab per chat, the shown one selected and in the tab order', () => {
    expect(html).toContain('role="tablist"');
    expect(html.match(/role="tab"/g)).toHaveLength(2);
    expect(html).toMatch(/aria-selected="true"[^>]*tabindex="0"[^>]*data-chat-id="main"|data-chat-id="main"[^>]*aria-selected="true"/);
    expect(html).toMatch(/data-chat-id="c2"[^>]*tabindex="-1"|tabindex="-1"[^>]*data-chat-id="c2"/);
  });

  it('shows a dot for each state, and names the states in the accessible name', () => {
    expect(html).toContain('chat-tab-dot is-busy');
    expect(html).toContain('chat-tab-dot is-approval');
    expect(html).not.toContain('chat-tab-dot is-question');
    expect(html).toContain('aria-label="Chat 2, working, needs approval"');
    expect(html).toContain('aria-label="Chat 1"');
  });

  it('marks the close of a busy chat disabled, and leaves an idle chat\'s close enabled', () => {
    expect(html).toContain('aria-label="Close Chat 2" aria-disabled="true"');
    expect(html).toContain('aria-label="Close Chat 1"');
    expect(html).not.toContain('aria-label="Close Chat 1" aria-disabled');
  });
});

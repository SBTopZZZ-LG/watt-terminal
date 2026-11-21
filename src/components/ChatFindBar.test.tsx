import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { ChatFindBar } from './ChatFindBar';

describe('ChatFindBar markup', () => {
  const html = renderToStaticMarkup(<ChatFindBar focusToken={0} onClose={() => undefined} />);

  it('is a search region with a labelled input', () => {
    expect(html).toContain('class="chat-find"');
    expect(html).toContain('role="search"');
    expect(html).toContain('aria-label="Find in chat"');
  });

  it('has previous, next, case and close controls, with the case toggle off', () => {
    expect(html).toContain('aria-label="Previous match"');
    expect(html).toContain('aria-label="Next match"');
    expect(html).toContain('aria-pressed="false"');
    expect(html).toContain('aria-label="Close find"');
  });

  it('shows no count before a query is typed', () => {
    expect(html).toContain('<span class="find-count" aria-live="polite"></span>');
  });
});

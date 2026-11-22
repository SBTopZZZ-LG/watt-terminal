import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { FindBar } from './FindBar';

describe('FindBar markup', () => {
  const html = renderToStaticMarkup(<FindBar focusToken={0} onClose={() => undefined} />);

  it('has previous, next, case and close controls, named, with styled tips and no native titles', () => {
    expect(html).toContain('aria-label="Previous match"');
    expect(html).toContain('data-tip="Previous (Shift+Enter)"');
    expect(html).toContain('aria-label="Next match"');
    expect(html).toContain('data-tip="Next (Enter)"');
    expect(html).toContain('aria-label="Match case"');
    expect(html).toContain('data-tip="Match case"');
    expect(html).toContain('aria-label="Close find"');
    expect(html).toContain('data-tip="Close (Esc)"');
    expect(html).not.toContain('title=');
  });
});

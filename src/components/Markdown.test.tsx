import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { Markdown } from './Markdown';

function html(text: string): string {
  return renderToStaticMarkup(<Markdown text={text} />);
}

describe('Markdown: untrusted model text', () => {
  it('does not render raw script or img HTML as elements', () => {
    const out = html('<script>alert(1)</script>\n\n<img src=x onerror=alert(1)>');
    expect(out).not.toContain('<script');
    expect(out).not.toContain('<img');
    // Shown as escaped text, so it is visible but inert.
    expect(out).toContain('&lt;script&gt;');
    expect(out).toContain('&lt;img src=x onerror=alert(1)&gt;');
  });

  it('renders a link as a non-navigating span that keeps the text and shows the URL in title', () => {
    const out = html('[x](https://evil.example)');
    expect(out).not.toContain('<a');
    expect(out).not.toContain('href');
    expect(out).toContain('>x</span>');
    expect(out).toContain('title="https://evil.example"');
  });

  it('renders an image as no img element and no remote URL, keeping the alt text', () => {
    const out = html('![alt text](https://evil.example/a.png)');
    expect(out).not.toContain('<img');
    expect(out).not.toContain('evil.example');
    expect(out).toContain('alt text');
  });
});

describe('Markdown: rendering', () => {
  it('gives a fenced block its language as data-lang', () => {
    const out = html('```sh\nls -la\n```');
    expect(out).toContain('<pre data-lang="sh">');
    expect(out).toContain('ls -la');
  });

  it('gives an unlabelled fence data-lang="text"', () => {
    expect(html('```\nplain\n```')).toContain('<pre data-lang="text">');
  });

  it('renders a GFM table inside the scroll wrapper', () => {
    const out = html('| a | b |\n|---|---|\n| 1 | 2 |');
    expect(out).toMatch(/<div class="md-table"><table>.*<\/table><\/div>/);
    expect(out).toContain('<td>2</td>');
  });

  it('renders GFM task items with a disabled, read-only checkbox and the done class on checked items', () => {
    const out = html('- [x] done\n- [ ] todo');
    expect(out).toContain('<ul class="tasks">');
    expect(out).toContain('<li class="done">');
    expect(out.match(/<input type="checkbox"[^>]*disabled=""/g)).toHaveLength(2);
    expect(out).toMatch(/<input type="checkbox" disabled="" readOnly="" checked=""\/> done/);
    expect(out).toMatch(/<input type="checkbox" disabled="" readOnly=""\/> todo/);
  });

  it('keeps plain text as plain text', () => {
    expect(html('Hello there.')).toBe('<div class="md"><p>Hello there.</p></div>');
  });

  it('does not throw on an unclosed fence and keeps the partial text', () => {
    let out = '';
    expect(() => {
      out = html('Here it is:\n\n```js\nconst x = 1');
    }).not.toThrow();
    expect(out).toContain('const x = 1');
    expect(out).toContain('data-lang="js"');
  });
});

import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { Sheet, SheetHostContext } from './Sheet';

const NOOP = () => undefined;

describe('Sheet', () => {
  it('is a modal dialog named by its title', () => {
    const html = renderToStaticMarkup(
      <Sheet title="Skill folder" onClose={NOOP}>
        <p>Body</p>
      </Sheet>,
    );
    expect(html).toContain('role="dialog"');
    expect(html).toContain('aria-modal="true"');
    const labelled = /aria-labelledby="([^"]+)"/.exec(html);
    expect(labelled).not.toBeNull();
    expect(html).toContain(`<h3 class="sheet-title" id="${labelled![1]}">Skill folder</h3>`);
  });

  it('has a close button named Close and the body inside the sheet', () => {
    const html = renderToStaticMarkup(
      <Sheet title="Skill folder" onClose={NOOP}>
        <p>Body text</p>
      </Sheet>,
    );
    expect(html).toContain('aria-label="Close"');
    expect(html).toContain('<div class="sheet-body"><p>Body text</p></div>');
  });

  it('pins the footer under the body only when one is given', () => {
    const withFoot = renderToStaticMarkup(
      <Sheet title="Add a provider" onClose={NOOP} footer={<button type="button">Save</button>}>
        <p>Body</p>
      </Sheet>,
    );
    expect(withFoot).toContain('<footer class="sheet-foot"><button type="button">Save</button></footer>');
    const without = renderToStaticMarkup(
      <Sheet title="Add a provider" onClose={NOOP}>
        <p>Body</p>
      </Sheet>,
    );
    expect(without).not.toContain('sheet-foot');
  });

  it('is narrow by default and wide on request', () => {
    const narrow = renderToStaticMarkup(
      <Sheet title="T" onClose={NOOP}>
        <p>x</p>
      </Sheet>,
    );
    expect(narrow).toContain('class="sheet"');
    expect(narrow).not.toContain('sheet-wide');
    const wide = renderToStaticMarkup(
      <Sheet title="T" onClose={NOOP} wide>
        <p>x</p>
      </Sheet>,
    );
    expect(wide).toContain('class="sheet sheet-wide"');
  });

  it('draws in place when there is no host (a render without the settings card)', () => {
    const html = renderToStaticMarkup(
      <SheetHostContext.Provider value={null}>
        <Sheet title="T" onClose={NOOP}>
          <p>x</p>
        </Sheet>
      </SheetHostContext.Provider>,
    );
    expect(html.startsWith('<div class="sheet-backdrop">')).toBe(true);
  });
});

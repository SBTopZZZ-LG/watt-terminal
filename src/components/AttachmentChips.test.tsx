import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { AttachmentChips } from './AttachmentChips';
import type { ChipItem } from './AttachmentChips';

const NOOP = () => undefined;
const render = (items: ChipItem[]) => renderToStaticMarkup(<AttachmentChips items={items} onRemove={NOOP} />);

const THUMB = 'data:image/jpeg;base64,/9j/AA==';

describe('AttachmentChips', () => {
  it('renders an empty row for no items', () => {
    expect(render([])).toBe('<div class="bar-attachments"></div>');
  });

  it('an image is a thumbnail with its name as alt, and a remove button named after it', () => {
    const html = render([{ key: 1, name: 'shot.png', kind: 'image', bytes: 900, thumb: THUMB, processing: false }]);
    expect(html).toContain(`<img class="bar-attach-thumb" src="${THUMB}" alt="shot.png"`);
    expect(html).toContain('aria-label="Remove shot.png"');
    expect(html).toContain('is-image');
  });

  it('a file is a chip with its name and its size, and a remove button', () => {
    const html = render([{ key: 2, name: 'notes.md', kind: 'file', bytes: 12 * 1024, processing: false }]);
    expect(html).toContain('bar-attach-name');
    expect(html).toContain('notes.md');
    expect(html).toContain('12 KB');
    expect(html).toContain('aria-label="Remove notes.md"');
    expect(html).toContain('is-file');
  });

  it('an item still being prepared shows "processing..." and no remove button', () => {
    const html = render([{ key: 3, name: 'big.jpg', kind: 'file', bytes: null, processing: true }]);
    expect(html).toContain('processing...');
    expect(html).toContain('is-processing');
    expect(html).not.toContain('Remove');
  });

  it('an image without a thumbnail is shown as a file chip', () => {
    const html = render([{ key: 4, name: 'odd.png', kind: 'image', bytes: 2048, processing: false }]);
    expect(html).not.toContain('<img');
    expect(html).toContain('2 KB');
  });

  it('a name is text, never markup', () => {
    const html = render([{ key: 5, name: '<img src=x onerror=alert(1)>', kind: 'file', bytes: 1, processing: false }]);
    expect(html).not.toContain('<img src=x');
    expect(html).toContain('&lt;img src=x onerror=alert(1)&gt;');
  });
});

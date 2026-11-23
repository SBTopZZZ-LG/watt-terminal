import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { ImageViewer, ImageViewerProvider } from './ImageViewer';

const NOOP = () => undefined;
const THUMB = 'data:image/jpeg;base64,/9j/AA==';
const FULL = 'data:image/png;base64,iVBORw0K';
const SHA = 'a'.repeat(64);

// null host: a static render has no body, so the viewer is drawn where it is.
const render = (image: Parameters<typeof ImageViewer>[0]['image']) => renderToStaticMarkup(<ImageViewer image={image} onClose={NOOP} host={null} />);

describe('ImageViewer', () => {
  it('is a modal dialog named after the image, with a close button and the file name as the caption', () => {
    const html = render({ name: 'shot.png', full: FULL });
    expect(html).toContain('role="dialog"');
    expect(html).toContain('aria-modal="true"');
    expect(html).toContain('aria-label="Image: shot.png"');
    expect(html).toContain('aria-label="Close image"');
    expect(html).toContain('<div class="image-viewer-caption">shot.png</div>');
  });

  it('shows a composer image at once: the full data URL, with no loading line', () => {
    const html = render({ name: 'shot.png', thumb: THUMB, full: FULL });
    expect(html).toContain(`src="${FULL}"`);
    expect(html).not.toContain(THUMB);
    expect(html).not.toContain('role="status"');
    expect(html).not.toContain('is-loading');
  });

  it('a sent image shows its thumbnail, dimmed, with a loading status until the full image arrives', () => {
    const html = render({ name: 'shot.png', thumb: THUMB, sha256: SHA, mime: 'image/png' });
    expect(html).toContain(`src="${THUMB}"`);
    expect(html).toContain('class="image-viewer-img is-loading"');
    expect(html).toContain('role="status"');
    expect(html).toContain('Loading…');
  });

  it('the image is not draggable and carries the file name as its alt text', () => {
    const html = render({ name: 'shot.png', full: FULL });
    expect(html).toContain('alt="shot.png"');
    expect(html).toContain('draggable="false"');
  });

  it('there is no zoom control: only the close button is a button', () => {
    const html = render({ name: 'shot.png', full: FULL });
    expect(html.match(/<button/g)).toHaveLength(1);
    expect(html).not.toMatch(/zoom/i);
  });

  it('with no thumbnail and a stored address, only the loading line shows (no image yet)', () => {
    const html = render({ name: 'p.jpg', sha256: SHA, mime: 'image/jpeg' });
    expect(html).toContain('Loading…');
    expect(html).not.toContain('<img');
  });
});

describe('ImageViewerProvider', () => {
  it('renders its children and nothing of the viewer until an image is opened', () => {
    expect(renderToStaticMarkup(<ImageViewerProvider>{<span>app</span>}</ImageViewerProvider>)).toBe('<span>app</span>');
  });
});

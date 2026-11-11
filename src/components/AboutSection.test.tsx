import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import pkg from '../../package.json';
import { AboutSection } from './AboutSection';

const html = renderToStaticMarkup(<AboutSection />);
const images = html.match(/<img[^>]*>/g) ?? [];

describe('AboutSection', () => {
  it('shows the app name and the version from package.json (the vite define reaches the tests)', () => {
    expect(html).toContain('Watt Terminal');
    expect(html).toContain(`Version ${pkg.version}`);
    expect(html).not.toContain('Version unknown');
  });

  it('has the mark and the wordmark, both decorative: empty alt and aria-hidden', () => {
    expect(images).toHaveLength(2);
    expect(images[0]).toMatch(/watt-mark/);
    expect(images[1]).toMatch(/watt-wordmark/);
    for (const img of images) {
      expect(img).toContain('alt=""');
      expect(img).toContain('aria-hidden="true"');
    }
  });

  it('gives both images their size, so nothing shifts when they load', () => {
    expect(images[0]).toContain('width="212"');
    expect(images[0]).toContain('height="256"');
    expect(images[1]).toContain('width="480"');
    expect(images[1]).toContain('height="155"');
  });

  it('has no tagline (the tagline is for the README only)', () => {
    expect(html).not.toContain('Whatever Agentic');
  });
});

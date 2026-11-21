import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { TIP_ID, TipBox, Tooltip } from './Tooltip';

describe('Tooltip', () => {
  it('renders nothing until something is hovered or focused', () => {
    expect(renderToStaticMarkup(<Tooltip />)).toBe('');
  });
});

describe('TipBox', () => {
  it('is a role=tooltip with the shared id, hidden until it is placed', () => {
    const html = renderToStaticMarkup(<TipBox text="Full text" pos={null} />);
    expect(html).toBe(`<div id="${TIP_ID}" role="tooltip" class="tooltip" style="left:0;top:0;visibility:hidden">Full text</div>`);
  });

  it('is placed at its coordinates, with its side as a class', () => {
    const html = renderToStaticMarkup(<TipBox text="Below" pos={{ x: 12, y: 40, side: 'below' }} />);
    expect(html).toContain('class="tooltip tooltip-below"');
    expect(html).toContain('style="left:12px;top:40px"');
    expect(html).not.toContain('visibility');
    expect(renderToStaticMarkup(<TipBox text="Above" pos={{ x: 0, y: 0, side: 'above' }} />)).toContain('tooltip-above');
  });

  it('escapes its text (markup in a command or a path is shown as text)', () => {
    const html = renderToStaticMarkup(<TipBox text={'<img src=x onerror=alert(1)>'} pos={null} />);
    expect(html).toContain('&lt;img src=x onerror=alert(1)&gt;');
    expect(html).not.toContain('<img');
  });
});

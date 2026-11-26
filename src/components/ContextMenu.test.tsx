import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { CONTEXT_MENU_ID, ContextMenu, ContextMenuBox, contextMenuItemId } from './ContextMenu';
import type { MenuItem } from '../lib/contextMenu';

const noop = () => undefined;

describe('ContextMenu', () => {
  it('renders nothing until something is right-clicked or the Menu key is pressed', () => {
    const html = renderToStaticMarkup(
      <ContextMenu terminal={{ state: 'prompt', agentBusy: false, pasteRoutesToPty: true }} onClearTerminal={noop} onFindTerminal={noop} onFindChat={noop} devtools={false} onRestartTab={noop} chatId="main" />,
    );
    expect(html).toBe('');
  });
});

describe('ContextMenuBox', () => {
  const items: MenuItem[] = [
    { id: 'copy', label: 'Copy', shortcut: 'Ctrl+C', disabled: true },
    { id: 'paste', label: 'Paste', shortcut: 'Ctrl+V' },
    { id: 'separator', label: '', separator: true },
    { id: 'clear', label: 'Clear terminal' },
  ];

  it('is a role=menu with role=menuitem rows and a role=separator divider', () => {
    const html = renderToStaticMarkup(<ContextMenuBox items={items} index={-1} pos={{ x: 10, y: 20, side: 'below' }} onActivate={noop} onHover={noop} />);
    expect(html).toContain(`id="${CONTEXT_MENU_ID}"`);
    expect(html).toContain('role="menu"');
    expect(html.match(/role="menuitem"/g)).toHaveLength(3);
    expect(html).toContain('role="separator"');
    expect(html).toContain('style="left:10px;top:20px"');
  });

  it('marks a disabled item with aria-disabled and the dimmed class', () => {
    const html = renderToStaticMarkup(<ContextMenuBox items={items} index={-1} pos={{ x: 0, y: 0, side: 'below' }} onActivate={noop} onHover={noop} />);
    expect(html).toContain('aria-disabled="true" class="context-menu-item is-disabled"');
    expect(html.match(/aria-disabled/g)).toHaveLength(1);
  });

  it('shows the shortcut hint beside its label', () => {
    const html = renderToStaticMarkup(<ContextMenuBox items={items} index={-1} pos={{ x: 0, y: 0, side: 'below' }} onActivate={noop} onHover={noop} />);
    expect(html).toContain('<span class="context-menu-shortcut">Ctrl+V</span>');
    expect(html).not.toContain('context-menu-shortcut">undefined');
  });

  it('points aria-activedescendant at the highlighted item, and marks it', () => {
    const html = renderToStaticMarkup(<ContextMenuBox items={items} index={1} pos={{ x: 0, y: 0, side: 'below' }} onActivate={noop} onHover={noop} />);
    expect(html).toContain(`aria-activedescendant="${contextMenuItemId(1)}"`);
    expect(html).toContain(`id="${contextMenuItemId(1)}" role="menuitem"`);
    expect(html).toContain('is-highlighted');
  });

  it('is hidden until it is placed, so it cannot flash in the wrong place', () => {
    const html = renderToStaticMarkup(<ContextMenuBox items={items} index={-1} pos={null} onActivate={noop} onHover={noop} />);
    expect(html).toContain('style="left:0;top:0;visibility:hidden"');
  });

  it('escapes its labels as text', () => {
    const html = renderToStaticMarkup(
      <ContextMenuBox items={[{ id: 'copy-link', label: '<img src=x>' }]} index={-1} pos={null} onActivate={noop} onHover={noop} />,
    );
    expect(html).toContain('&lt;img src=x&gt;');
    expect(html).not.toContain('<img');
  });
});

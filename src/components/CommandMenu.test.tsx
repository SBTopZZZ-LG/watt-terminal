import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { CommandMenu } from './CommandMenu';
import type { CommandMenuItem } from '../lib/commandMenu';

const NOOP = () => undefined;

const ITEMS: CommandMenuItem[] = [
  { name: 'plan', description: 'Ask for a plan first.', hint: '[task]', source: null },
  { name: 'new', description: 'Start a new conversation.', hint: null, source: null },
  { name: 'deploy', description: 'Deploy it.', hint: '[env]', source: 'project' },
  { name: 'fixup', description: 'Fix it.', hint: null, source: 'global' },
];

const html = (index = 0, items: CommandMenuItem[] = ITEMS) =>
  renderToStaticMarkup(<CommandMenu items={items} index={index} onAccept={NOOP} onHover={NOOP} />);

describe('CommandMenu', () => {
  it('is a listbox of options with stable ids', () => {
    const out = html();
    expect(out).toContain('id="command-menu"');
    expect(out).toContain('role="listbox"');
    expect(out.match(/role="option"/g)).toHaveLength(4);
    expect(out).toContain('id="command-menu-item-0"');
    expect(out).toContain('id="command-menu-item-3"');
  });

  it('marks only the selected row', () => {
    const out = html(2);
    expect(out.match(/aria-selected="true"/g)).toHaveLength(1);
    expect(out.match(/aria-selected="false"/g)).toHaveLength(3);
    expect(out.match(/class="command-menu-item is-selected"/g)).toHaveLength(1);
    expect(out).toContain('id="command-menu-item-2" role="option" aria-selected="true"');
  });

  it('shows the name, hint and description of every row', () => {
    const out = html();
    expect(out).toContain('<span class="command-menu-name">/plan</span>');
    expect(out).toContain('<span class="command-menu-hint">[task]</span>');
    expect(out).toContain('<span class="command-menu-description">Ask for a plan first.</span>');
    // No hint, no hint span.
    expect(out.match(/command-menu-hint/g)).toHaveLength(2);
  });

  it('badges only the custom rows with their source', () => {
    const out = html();
    expect(out).toContain('<span class="command-menu-source">project</span>');
    expect(out).toContain('<span class="command-menu-source">global</span>');
    expect(out.match(/command-menu-source/g)).toHaveLength(2);
  });

  it('shows command text as plain text: markup is escaped', () => {
    const out = html(0, [{ name: 'evil', description: '<img src=x>', hint: '<arg>', source: null }]);
    expect(out).not.toContain('<img');
    expect(out).toContain('&lt;img');
    expect(out).toContain('&lt;arg&gt;');
  });
});

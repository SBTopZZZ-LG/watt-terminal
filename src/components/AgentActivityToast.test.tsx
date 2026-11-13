import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { AgentActivityToast } from './AgentActivityToast';
import type { Dock } from '../lib/panelLayout';
import { DOCKS } from '../lib/panelLayout';

describe('AgentActivityToast', () => {
  it('is a button with the dock class, a live region, and the agent title', () => {
    const html = renderToStaticMarkup(<AgentActivityToast dock="right" text="Thinking…" onOpen={() => {}} />);
    expect(html).toContain('class="agent-activity dock-right"');
    expect(html).toContain('aria-live="polite"');
    expect(html).toContain('title="Show the agent"');
    expect(html).toContain('Agent is working');
    expect(html).toContain('Thinking…');
    expect(html).toContain('agent-activity-spinner');
    expect(html.startsWith('<button')).toBe(true);
  });

  it('carries the dock class for every dock', () => {
    for (const dock of DOCKS) {
      const html = renderToStaticMarkup(<AgentActivityToast dock={dock as Dock} text="Working…" onOpen={() => {}} />);
      expect(html).toContain(`class="agent-activity dock-${dock}"`);
    }
  });

  it('shows the activity text', () => {
    const html = renderToStaticMarkup(<AgentActivityToast dock="bottom" text="run_command · List files" onOpen={() => {}} />);
    expect(html).toContain('run_command · List files');
  });
});

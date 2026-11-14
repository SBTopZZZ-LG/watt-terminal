import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { defaultLayout, DOCKS, panelView } from '../lib/panelLayout';
import type { Container, Dock, PanelLayout } from '../lib/panelLayout';
import { AgentPanel, Workspace } from './AgentPanel';
import { PanelDivider } from './PanelDivider';

const BIG: Container = { width: 1600, height: 1000 };

function layoutFor(dock: Dock, open = true): PanelLayout {
  return { ...defaultLayout(), dock, open };
}

describe('Workspace: the React order does not depend on the dock', () => {
  it('puts the terminal, then the divider, then the panel, for every dock', () => {
    for (const dock of DOCKS) {
      const html = renderToStaticMarkup(
        <Workspace
          dock={dock}
          terminal={<i data-part="terminal" />}
          divider={<b data-part="divider" />}
          panel={<u data-part="panel" />}
        />,
      );
      const t = html.indexOf('data-part="terminal"');
      const d = html.indexOf('data-part="divider"');
      const p = html.indexOf('data-part="panel"');
      expect(t).toBeGreaterThan(-1);
      expect(t).toBeLessThan(d);
      expect(d).toBeLessThan(p);
      expect(html).toContain(`class="workspace dock-${dock}"`);
    }
  });

  it('keeps the terminal first when the divider is hidden', () => {
    const html = renderToStaticMarkup(
      <Workspace dock="left" terminal={<i data-part="terminal" />} divider={null} panel={<u data-part="panel" />} />,
    );
    expect(html.indexOf('data-part="terminal"')).toBeLessThan(html.indexOf('data-part="panel"'));
  });
});

describe('AgentPanel', () => {
  it('shows the rail button only on a closed side panel, and keeps its children in place', () => {
    for (const dock of DOCKS) {
      for (const open of [true, false]) {
        const view = panelView(layoutFor(dock, open), BIG);
        const html = renderToStaticMarkup(
          <AgentPanel view={view} onExpand={() => {}}>
            <span data-part="child">kept</span>
          </AgentPanel>,
        );
        expect(html).toContain('data-part="child"');
        expect(html.includes('class="agent-rail"')).toBe(view.rail);
      }
    }
  });

  it('gives the panel its class and inline size', () => {
    const side = renderToStaticMarkup(
      <AgentPanel view={panelView(layoutFor('right'), BIG)} onExpand={() => {}}>
        <span />
      </AgentPanel>,
    );
    expect(side).toContain('class="agent-panel dock-right is-sized"');
    expect(side).toContain('width:380px');

    const top = renderToStaticMarkup(
      <AgentPanel view={panelView(layoutFor('top'), BIG)} onExpand={() => {}}>
        <span />
      </AgentPanel>,
    );
    expect(top).toContain('class="agent-panel dock-top"');
    expect(top).not.toContain('height:');
  });

  it('marks the rail button with its label', () => {
    const html = renderToStaticMarkup(
      <AgentPanel view={panelView(layoutFor('left', false), BIG)} onExpand={() => {}}>
        <span />
      </AgentPanel>,
    );
    expect(html).toContain('aria-label="Show the agent panel"');
    expect(html).toContain('agent-rail-label');
  });

  it('shows a busy dot on the rail button while the agent works', () => {
    const busy = renderToStaticMarkup(
      <AgentPanel view={panelView(layoutFor('left', false), BIG)} onExpand={() => {}} busy>
        <span />
      </AgentPanel>,
    );
    expect(busy).toContain('agent-rail-busy');
    expect(busy).toContain('aria-label="Agent is working"');

    const idle = renderToStaticMarkup(
      <AgentPanel view={panelView(layoutFor('left', false), BIG)} onExpand={() => {}}>
        <span />
      </AgentPanel>,
    );
    expect(idle).not.toContain('agent-rail-busy');
  });

  it('shows an approval dot on the rail button while a card waits, in addition to the busy dot', () => {
    const both = renderToStaticMarkup(
      <AgentPanel view={panelView(layoutFor('left', false), BIG)} onExpand={() => {}} busy approvalPending>
        <span />
      </AgentPanel>,
    );
    expect(both).toContain('agent-rail-approval');
    expect(both).toContain('aria-label="Approval needed"');
    expect(both).toContain('agent-rail-busy');

    const only = renderToStaticMarkup(
      <AgentPanel view={panelView(layoutFor('left', false), BIG)} onExpand={() => {}} approvalPending>
        <span />
      </AgentPanel>,
    );
    expect(only).toContain('agent-rail-approval');
    expect(only).not.toContain('agent-rail-busy');

    const neither = renderToStaticMarkup(
      <AgentPanel view={panelView(layoutFor('left', false), BIG)} onExpand={() => {}}>
        <span />
      </AgentPanel>,
    );
    expect(neither).not.toContain('agent-rail-approval');
  });
});

describe('PanelDivider', () => {
  const noop = () => {};
  const render = (dock: Dock, valueNow: number) =>
    renderToStaticMarkup(
      <PanelDivider
        dock={dock}
        container={BIG}
        valueNow={valueNow}
        measure={() => valueNow}
        onResize={noop}
        onReset={noop}
        onSettle={noop}
      />,
    );

  it('is a focusable separator, vertical for side docks and horizontal for top and bottom', () => {
    const side = render('left', 380);
    expect(side).toContain('role="separator"');
    expect(side).toContain('tabindex="0"');
    expect(side).toContain('aria-orientation="vertical"');
    expect(side).not.toContain('is-horizontal');

    const bottom = render('bottom', 200);
    expect(bottom).toContain('aria-orientation="horizontal"');
    expect(bottom).toContain('is-horizontal');
  });

  it('reports the value and the bounds of the panel', () => {
    const html = render('right', 380.4);
    expect(html).toContain('aria-valuenow="380"');
    expect(html).toContain('aria-valuemin="240"');
    expect(html).toContain(`aria-valuemax="${1600 - 326}"`);
    const vertical = render('top', 200);
    expect(vertical).toContain('aria-valuemin="140"');
    expect(vertical).toContain(`aria-valuemax="${1000 - 126}"`);
  });
});

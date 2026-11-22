import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { McpSection, McpToolList } from './McpSection';
import type { McpSectionProps } from './McpSection';
import { McpServerForm } from './McpServerForm';
import { draftFromServer, presetDraft, blankDraft } from '../lib/mcpForm';
import type { McpServerConfig, McpServerView, McpView } from '../lib/types';

const NOOP = () => undefined;
const NOOP_ASYNC = () => Promise.resolve();

const FETCH: McpServerConfig = {
  name: 'fetch',
  enabled: true,
  approval: 'ask',
  transport: { type: 'stdio', command: 'uvx', args: ['mcp-server-fetch'], env: [] },
};

const CONTEXT7: McpServerConfig = {
  name: 'context7',
  enabled: true,
  approval: 'auto',
  tools: { 'resolve-library-id': 'never' },
  transport: { type: 'http', url: 'https://mcp.context7.com/mcp?api_key=url-secret-77', headers: ['Authorization'] },
};

function server(over: Partial<McpServerView> & { config: McpServerConfig }): McpServerView {
  return { state: { kind: 'connected', tools: 0 }, tools: [], secrets_present: { env: [], headers: [] }, ...over };
}

const VIEW: McpView = {
  servers: [
    server({
      config: FETCH,
      state: { kind: 'connected', tools: 1 },
      tools: [{ name: 'fetch', description: 'Fetch a URL', override: null, effective: 'ask' }],
    }),
    server({
      config: CONTEXT7,
      state: { kind: 'failed', message: 'connection refused' },
      secrets_present: { env: [], headers: ['Authorization'] },
    }),
  ],
};

const BASE: McpSectionProps = {
  view: VIEW,
  loadError: null,
  actionError: null,
  busy: false,
  panel: null,
  logs: null,
  askRemove: null,
  onToggleEnabled: NOOP,
  onServerApproval: NOOP,
  onToolApproval: NOOP,
  onRestart: NOOP,
  onToggleLogs: NOOP,
  onAskRemove: NOOP,
  onRemove: NOOP,
  onEdit: NOOP,
  onOpenAdd: NOOP,
  onClosePanel: NOOP,
  onSave: NOOP_ASYNC,
  onImport: () => Promise.resolve({ view: VIEW, added: [], warnings: [] }),
};

const render = (over: Partial<McpSectionProps> = {}) => renderToStaticMarkup(<McpSection {...BASE} {...over} />);

/** The opening tag of the element holding `needle`: searches back from the needle to the nearest `<tag`. */
function tagHolding(html: string, tag: string, needle: string): string {
  const at = html.indexOf(needle);
  expect(at, `${needle} is in the markup`).toBeGreaterThan(-1);
  const start = html.lastIndexOf(`<${tag}`, at);
  return html.slice(start, html.indexOf('>', start) + 1);
}

describe('McpSection: the list', () => {
  it('shows the loading line before the first read, and the load error when the read fails', () => {
    expect(render({ view: null })).toContain('Loading MCP servers...');
    const failed = render({ view: null, loadError: 'backend is down' });
    expect(failed).toContain('backend is down');
    expect(failed).not.toContain('Loading MCP servers...');
  });

  it('says there are no servers, and offers Add server', () => {
    const html = render({ view: { servers: [] } });
    expect(html).toContain('No MCP servers yet. Add one below.');
    expect(html).toContain('>Add server<');
  });

  it('shows a connected server with its tool count and transport, and its state tag', () => {
    const html = render();
    expect(html).toContain('connected · 1 tool');
    expect(html).toContain('uvx mcp-server-fetch');
    expect(html).toContain('settings-tag-ok');
  });

  it('shows a failed server with its message, and drops the query from its URL', () => {
    const html = render();
    expect(html).toContain('failed');
    expect(html).toContain('connection refused');
    expect(html).toContain('https://mcp.context7.com/mcp');
    expect(html).not.toContain('url-secret-77');
    expect(html).not.toContain('api_key');
  });

  it('shows an auth message from a server that needs sign-in', () => {
    const html = render({
      view: { servers: [server({ config: CONTEXT7, state: { kind: 'needs_auth', message: 'sign in first' } })] },
    });
    expect(html).toContain('needs auth');
    expect(html).toContain('sign in first');
  });

  it('a disabled server shows as disabled, and its Restart is off', () => {
    const html = render({
      view: { servers: [server({ config: { ...FETCH, enabled: false }, state: { kind: 'disabled' } })] },
    });
    expect(html).toContain('disabled');
    expect(tagHolding(html, 'button', '>Restart<')).toContain('disabled=""');
  });

  it('while a change is in flight, the controls that change a server are off', () => {
    const html = render({ busy: true });
    expect(tagHolding(html, 'button', '>Add server<')).toContain('disabled=""');
    expect(tagHolding(html, 'button', '>Remove<')).toContain('disabled=""');
  });

  it('the server approval select offers the three levels', () => {
    const html = render();
    expect(html).toContain('Ask each time');
    expect(html).toContain('Auto-approve');
    expect(html).toContain('Never (hide tools)');
  });

  it('shows the errors of the last action', () => {
    const html = render({ actionError: 'could not restart' });
    expect(html).toContain('could not restart');
    expect(html).toContain('role="alert"');
  });

  it('asks before removing, naming the server and its stored values', () => {
    const html = render({ askRemove: 'fetch' });
    expect(html).toContain('Remove fetch and its stored values?');
    expect(html).toContain('Yes, remove');
  });
});

describe('McpSection: logs', () => {
  it('shows the open server log, and says so when it has no lines', () => {
    const shown = render({ logs: { name: 'fetch', lines: ['started', 'ready'], error: null } });
    expect(tagHolding(shown, 'pre', 'started')).toContain('settings-logs');
    expect(shown).toContain('started\nready');
    expect(render({ logs: { name: 'fetch', lines: [], error: null } })).toContain('(no output)');
  });

  it('a log for one server does not show under another', () => {
    const html = render({ logs: { name: 'context7', lines: ['only here'], error: null } });
    expect(html).toContain('only here');
    expect(html.split('settings-logs').length).toBe(2);
  });

  it('shows a log load error in place of the lines', () => {
    expect(render({ logs: { name: 'fetch', lines: null, error: 'no such server' } })).toContain('no such server');
  });
});

describe('McpSection: the add and edit panels', () => {
  it('the add panel opens on a custom server, with its preset list', () => {
    const html = render({ panel: { kind: 'add' } });
    expect(html).toContain('Add a server');
    expect(html).toContain('Fetch (uvx mcp-server-fetch)');
    expect(html).toContain('You.com (API key)');
    expect(html).toContain('Context7 (optional API key)');
    expect(html).toContain('Filesystem (npx, folders you list)');
    expect(html).toContain('>Add server<');
    expect(tagHolding(html, 'button', 'Form')).toContain('aria-selected="true"');
  });

  it('the edit panel is the server form, with the name fixed and the stored header marked stored, no value', () => {
    const html = render({ panel: { kind: 'edit', name: 'context7' } });
    expect(html).toContain('Edit context7');
    expect(html).toContain('stored');
    // The stored header's value box is empty and the name cannot change.
    expect(html).toContain('readOnly=""');
    expect(html).toContain('value="context7"');
    expect(tagHolding(html, 'input', 'aria-label="header value"')).toContain('value=""');
  });
});

describe('McpToolList', () => {
  it('lists each tool with its description, and its override select set to the server default', () => {
    const html = renderToStaticMarkup(
      <McpToolList
        name="context7"
        tools={[
          { name: 'resolve-library-id', description: 'Find a library', override: 'never', effective: 'never' },
          { name: 'query-docs', description: 'Query the docs', override: null, effective: 'auto' },
        ]}
        busy={false}
        onToolApproval={NOOP}
      />,
    );
    expect(html).toContain('resolve-library-id');
    expect(html).toContain('Find a library');
    expect(html).toContain('MCP default (Auto-approve)');
    expect(html).toContain('>Never (hide tools)<');
  });

  it('says so when a server lists no tools', () => {
    const html = renderToStaticMarkup(<McpToolList name="fetch" tools={[]} busy={false} onToolApproval={NOOP} />);
    expect(html).toContain('No tools listed.');
  });
});

describe('McpServerForm: the secret rows', () => {
  it('a new Authorization row takes a bare key, with the Bearer note in its placeholder', () => {
    const html = renderToStaticMarkup(
      <McpServerForm initial={presetDraft('you-key')} takenNames={[]} busy={false} onSave={NOOP_ASYNC} onCancel={NOOP} />,
    );
    expect(html).toContain('API key (sent as Bearer)');
    expect(html).toContain('value="you-com"');
    expect(html).toContain('https://api.you.com/mcp');
    // A password box, never a visible value.
    expect(html).toContain('type="password"');
  });

  it('an optional stored header shows "Leave empty to keep" and a Remove button, and no value', () => {
    const draft = draftFromServer(
      server({ config: CONTEXT7, secrets_present: { env: [], headers: ['Authorization'] } }),
    );
    const html = renderToStaticMarkup(
      <McpServerForm initial={draft} original={CONTEXT7} takenNames={['fetch']} busy={false} onSave={NOOP_ASYNC} onCancel={NOOP} />,
    );
    expect(html).toContain('Leave empty to keep');
    expect(html).toContain('>Remove<');
    expect(html).toContain('>stored<');
    expect(html).not.toContain('removed on save');
  });

  it('a new server form starts blank on the stdio transport', () => {
    const html = renderToStaticMarkup(
      <McpServerForm initial={blankDraft()} takenNames={[]} busy={false} onSave={NOOP_ASYNC} onCancel={NOOP} />,
    );
    expect(html).toContain('Command (stdio)');
    expect(html).toContain('Add server');
  });

  it('the filesystem preset asks for folders, not arguments', () => {
    const html = renderToStaticMarkup(
      <McpServerForm initial={presetDraft('filesystem')} takenNames={[]} busy={false} onSave={NOOP_ASYNC} onCancel={NOOP} />,
    );
    expect(html).toContain('Folders the server may read, one per line');
    expect(html).not.toContain('Arguments, one per line');
  });
});

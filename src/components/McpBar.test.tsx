import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { McpLoader, McpPanel } from './McpBar';
import type { McpServerStatus } from '../lib/types';

const NOOP = () => undefined;
const server = (name: string, state: McpServerStatus['state'], enabled = true, tools = 0): McpServerStatus => ({ id: name, name, enabled, state, tools });

const SERVERS: McpServerStatus[] = [
  server('fetch', { kind: 'connected', tools: 3 }, true, 3),
  server('docs', { kind: 'failed', message: 'spawn ENOENT' }),
  server('ctx', { kind: 'disabled' }, false),
];

const panel = (over: Partial<Parameters<typeof McpPanel>[0]> = {}) =>
  renderToStaticMarkup(<McpPanel id="p1" open servers={SERVERS} pending={null} error={null} onToggle={NOOP} {...over} />);

/** The opening tag of the switch for one server. */
const switchFor = (out: string, name: string) => new RegExp(`<button[^>]*aria-label="${name}"[^>]*>`).exec(out)?.[0] ?? '';

describe('McpLoader', () => {
  it('renders nothing when nothing is starting', () => {
    expect(renderToStaticMarkup(<McpLoader starting={false} servers={SERVERS} />)).toBe('');
  });

  it('renders a status row with the spinner and the count while starting', () => {
    const two = [server('fetch', { kind: 'connected', tools: 1 }), server('docs', { kind: 'connecting' })];
    const out = renderToStaticMarkup(<McpLoader starting servers={two} />);
    expect(out).toContain('role="status"');
    expect(out).toContain('aria-live="polite"');
    expect(out).toContain('braille-spinner');
    expect(out).toContain('Starting MCP servers… (1/2 ready)');
  });

  it('shows no count for a single enabled server', () => {
    const out = renderToStaticMarkup(<McpLoader starting servers={[server('fetch', { kind: 'connecting' })]} />);
    expect(out).toContain('Starting MCP servers…');
    expect(out).not.toContain('ready');
  });
});

describe('McpPanel', () => {
  it('is in the DOM but hidden while closed', () => {
    expect(panel({ open: false })).toMatch(/class="mcp-panel" role="region"[^>]*hidden=""/);
  });

  it('is a labelled region that a button controls by id', () => {
    const out = panel();
    expect(out).toContain('id="p1"');
    expect(out).toContain('aria-label="MCP servers"');
    expect(out).not.toMatch(/hidden=""/);
  });

  it('has one row per server with its name, state and dot', () => {
    const out = panel();
    expect(out.match(/class="mcp-row"/g)).toHaveLength(3);
    expect(out).toContain('>fetch<');
    expect(out).toContain('3 tools');
    expect(out).toContain('failed');
    expect(out).toContain('data-tone="fail"');
    expect(out).toContain('data-tone="ok"');
  });

  it('gives each switch role=switch and its state in aria-checked', () => {
    const out = panel();
    expect(switchFor(out, 'fetch')).toMatch(/role="switch"/);
    expect(switchFor(out, 'fetch')).toContain('aria-checked="true"');
    expect(switchFor(out, 'docs')).toContain('aria-checked="true"');
    expect(switchFor(out, 'ctx')).toContain('aria-checked="false"');
    expect(switchFor(out, 'ctx')).toContain('data-tip="Hidden from the agent"');
    expect(switchFor(out, 'fetch')).toContain('data-tip="The agent can use this server"');
  });

  it('puts a failure message in the state tooltip, not in the row text', () => {
    const out = panel();
    expect(out).toContain('title="spawn ENOENT"');
  });

  it('disables every switch while one is being saved', () => {
    const out = panel({ pending: 'fetch' });
    expect(switchFor(out, 'fetch')).toContain('disabled=""');
    expect(switchFor(out, 'ctx')).toContain('disabled=""');
  });

  it('shows a save error as an alert', () => {
    const out = panel({ error: 'docs: the server could not be changed' });
    expect(out).toMatch(/role="alert"[^>]*>docs: the server could not be changed</);
  });

  it('shows the empty state when no server is configured', () => {
    const out = panel({ servers: [] });
    expect(out).toContain('No MCP servers yet.');
    expect(out).not.toContain('role="switch"');
  });

  it('names the place to add servers and says when a change takes effect', () => {
    const out = panel();
    expect(out).toContain('Manage servers in Settings &gt; MCP.');
    expect(out).toContain('Changes take effect on the agent');
  });
});

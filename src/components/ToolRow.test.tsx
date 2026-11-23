import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { TranscriptItem } from '../lib/transcript';
import { ToolRow } from './ToolRow';

type ToolItem = Extract<TranscriptItem, { kind: 'tool' }>;

function row(overrides: Partial<ToolItem>): ToolItem {
  return { id: 1, kind: 'tool', callId: 'c1', name: 'run_command', args: {}, status: 'ok', result: {}, decision: 'read', reason: null, startedAt: 1, ...overrides };
}

/** React escapes quotes in text as &quot;; the JSON panel's text is checked as JSON. */
function unescapeQuotes(html: string): string {
  return html.replace(/&quot;/g, '"');
}

/** The opening tag of each tabpanel, in order: the first is the Input / Output panel, the second the JSON panel. */
function panelTags(html: string): string[] {
  return html.match(/<div role="tabpanel"[^>]*>/g) ?? [];
}

describe('ToolRow: tabs', () => {
  it('has a tab list with two tabs, and the first (Input / Output) is selected', () => {
    const html = renderToStaticMarkup(<ToolRow item={row({ args: { command: 'ls' }, result: {} })} />);
    expect(html).toContain('role="tablist"');
    const tabs = html.match(/<button[^>]*role="tab"[^>]*>[^<]*<\/button>/g) ?? [];
    expect(tabs).toHaveLength(2);
    expect(tabs[0]).toContain('aria-selected="true"');
    expect(tabs[0]).toContain('>Input / Output<');
    expect(tabs[1]).toContain('aria-selected="false"');
    expect(tabs[1]).toContain('>JSON<');
    expect(tabs[1]).toContain('tabindex="-1"');
  });

  it('shows the Input / Output panel by default and hides the JSON panel', () => {
    const html = renderToStaticMarkup(<ToolRow item={row({ args: { command: 'ls' }, result: {} })} />);
    const [io, json] = panelTags(html);
    expect(io).not.toContain('hidden');
    expect(json).toContain('hidden=""');
  });

  it('each tab controls its panel', () => {
    const html = renderToStaticMarkup(<ToolRow item={row({ args: {}, result: {} })} />);
    const [io, json] = panelTags(html);
    const ioId = /id="([^"]+-panel-io)"/.exec(io)?.[1];
    const jsonId = /id="([^"]+-panel-json)"/.exec(json)?.[1];
    expect(ioId).toBeDefined();
    expect(jsonId).toBeDefined();
    expect(html).toContain(`aria-controls="${ioId}"`);
    expect(html).toContain(`aria-controls="${jsonId}"`);
  });
});

describe('ToolRow: the panels', () => {
  it('Input / Output shows the command as typed, and the output with its exit status', () => {
    const html = renderToStaticMarkup(
      <ToolRow
        item={row({
          args: { command: 'cd x\nls -la' },
          result: { outcome: 'finished', record: { id: 4, exit_code: 0, status: 'complete', output: 'a.txt' } },
        })}
      />,
    );
    expect(html).toContain('<div class="tool-io-label">Input</div><pre>cd x\nls -la</pre>');
    expect(html).toContain('<div class="tool-io-label">Output</div><pre>a.txt\nexit 0 · complete</pre>');
  });

  it('the JSON panel holds the arguments and the raw result', () => {
    const html = renderToStaticMarkup(<ToolRow item={row({ args: { command: 'ls' }, result: { outcome: 'not_run', reason: 'cancelled' } })} />);
    const json = unescapeQuotes(html.slice(html.indexOf('-panel-json')));
    expect(json).toContain('"command": "ls"');
    expect(json).toContain('"outcome": "not_run"');
    expect(json).not.toContain('Restored from history');
  });

  it('a running row with no result yet says it is waiting for output', () => {
    const html = renderToStaticMarkup(<ToolRow item={row({ status: 'running', result: null, args: { command: 'sleep 9' } })} />);
    expect(html).toContain('<pre data-empty="">…waiting for output</pre>');
  });

  it('a placeholder pane is marked data-empty, and a pane with text is not', () => {
    const empty = renderToStaticMarkup(<ToolRow item={row({ args: {}, result: {} })} />);
    expect(empty).toContain('<pre data-empty="">(none)</pre>');
    expect(empty).toContain('<pre data-empty="">(no output)</pre>');
    const full = renderToStaticMarkup(<ToolRow item={row({ args: { command: 'ls' }, result: 'a.txt' })} />);
    expect(full).toContain('<pre>ls</pre>');
    expect(full).toContain('<pre>a.txt</pre>');
    expect(full).not.toContain('data-empty=""');
  });

  it('a failed row shows its reason as the output', () => {
    const html = renderToStaticMarkup(<ToolRow item={row({ status: 'failed', decision: null, result: 'error: no shell', args: { command: 'ls' } })} />);
    expect(html).toContain('<pre>error: no shell</pre>');
    expect(html).toContain(': failed</span>');
  });

  it('a restored row says its result is the text the model saw, in the JSON panel', () => {
    const html = renderToStaticMarkup(<ToolRow item={row({ startedAt: undefined, result: 'waited 1.0s: the screen changed', name: 'wait', args: { seconds: 5 } })} />);
    expect(html).toContain('Restored from history: the result is the text the model saw.');
    expect(unescapeQuotes(html)).toContain('"result": "waited 1.0s: the screen changed"');
  });

  it('a live string result (not restored) has no restore note', () => {
    const html = renderToStaticMarkup(<ToolRow item={row({ name: 'request_integration', result: 'Integrated ssh ubuntu@lidia (zsh).', args: {} })} />);
    expect(html).not.toContain('Restored from history');
  });
});

describe('ToolRow: the summary', () => {
  it('a wait row shows the time it waited, not the bare status', () => {
    const html = renderToStaticMarkup(<ToolRow item={row({ name: 'wait', args: { seconds: 5 }, result: { reason: 'screen_changed', waited_ms: 4900, state: 'running', screen: { rows: [] } } })} />);
    expect(html).toContain('<span class="tool-line">wait<span class="decision decision-read"> read</span>: waited 4.9s</span>');
    expect(html).not.toContain(': ok</span>');
  });

  it('a running wait shows the seconds it was asked for', () => {
    const html = renderToStaticMarkup(<ToolRow item={row({ name: 'wait', args: { seconds: 5 }, status: 'running', result: null })} />);
    expect(html).toContain(': waiting 5s</span>');
  });

  it('a restored wait shows its time from the text the model saw', () => {
    const html = renderToStaticMarkup(<ToolRow item={row({ name: 'wait', args: { seconds: 5 }, startedAt: undefined, result: 'waited 4.2s: the command finished with exit status 0' })} />);
    expect(html).toContain(': waited 4.2s</span>');
  });

  it('other rows keep the status word', () => {
    const html = renderToStaticMarkup(<ToolRow item={row({ name: 'get_cwd', args: {}, result: {} })} />);
    expect(html).toContain(': ok</span>');
  });
});

describe('ToolRow: an unavailable call', () => {
  it('its decision reads unavailable, and the error shows in the row without opening it', () => {
    const message = 'unknown tool `ghost`: no tool with this name is available right now.';
    const html = renderToStaticMarkup(
      <ToolRow item={row({ name: 'ghost', status: 'failed', decision: 'unavailable', args: {}, result: { error: message } })} />,
    );
    expect(html).toContain('<span class="decision decision-unavailable"> unavailable</span>');
    expect(html).toContain(`data-tip="${message}"`);
    expect(html).toContain(`>${message}</span>`);
  });

  it('a row with no error text shows no error line', () => {
    const html = renderToStaticMarkup(<ToolRow item={row({ name: 'get_cwd', args: {}, result: {} })} />);
    expect(html).not.toContain('decision-unavailable');
  });
});

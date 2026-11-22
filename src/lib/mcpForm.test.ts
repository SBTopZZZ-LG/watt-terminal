import { describe, expect, it } from 'vitest';
import {
  blankDraft,
  buildServerInput,
  defaultOptionLabel,
  draftFromServer,
  headerValueToSend,
  isLoopbackHost,
  mcpToolLabel,
  overrideFromValue,
  overrideValue,
  presetDraft,
  stateMessage,
  stateTag,
  transportSummary,
  urlForDisplay,
  urlProblem,
} from './mcpForm';
import type { McpFormDraft, SecretRow } from './mcpForm';
import type { McpServerConfig, McpServerView } from './types';

const row = (over: Partial<SecretRow> = {}): SecretRow => ({ name: 'API_KEY', value: '', stored: false, remove: false, optional: false, ...over });

function built(d: McpFormDraft, original?: McpServerConfig, taken: readonly string[] = []) {
  const r = buildServerInput(d, original, taken);
  if (!r.ok) {
    throw new Error(`expected a valid draft, got ${r.problems.join(' | ')}`);
  }
  return r.input;
}

function problemsOf(d: McpFormDraft, original?: McpServerConfig, taken: readonly string[] = []): string[] {
  const r = buildServerInput(d, original, taken);
  return r.ok ? [] : r.problems;
}

describe('presets', () => {
  it('fetch is a bare uvx stdio server with no secrets', () => {
    expect(built(presetDraft('fetch'))).toEqual({
      config: { name: 'fetch', enabled: true, approval: 'ask', transport: { type: 'stdio', command: 'uvx', args: ['mcp-server-fetch'], env: [] } },
      env_values: {},
      header_values: {},
    });
  });

  it('You.com free is the keyless URL', () => {
    const input = built(presetDraft('you-free'));
    expect(input.config.transport).toEqual({ type: 'http', url: 'https://api.you.com/mcp/free', headers: [] });
    expect(input.config.name).toBe('you-com');
  });

  it('You.com with a key sends the bare key as a Bearer Authorization header', () => {
    const draft = { ...presetDraft('you-key'), headers: [row({ name: 'Authorization', value: '  ydc-key-1  ' })] };
    const input = built(draft);
    expect(input.config.transport).toEqual({ type: 'http', url: 'https://api.you.com/mcp', headers: ['Authorization'] });
    expect(input.header_values).toEqual({ Authorization: 'Bearer ydc-key-1' });
  });

  it('Context7 with no key leaves its optional header out of the server', () => {
    const input = built(presetDraft('context7'));
    expect(input.config.transport).toEqual({ type: 'http', url: 'https://mcp.context7.com/mcp', headers: [] });
    expect(input.header_values).toEqual({});
  });

  it('Context7 with a key declares the Authorization header and sends Bearer', () => {
    const draft = { ...presetDraft('context7'), headers: [row({ name: 'Authorization', value: 'ctx7-key', optional: true })] };
    expect(built(draft).header_values).toEqual({ Authorization: 'Bearer ctx7-key' });
  });

  it('filesystem takes its folders one per line and needs at least one', () => {
    const draft = { ...presetDraft('filesystem'), folders: ' /home/a \n\n/srv/b\r\n' };
    expect(built(draft).config.transport).toEqual({
      type: 'stdio',
      command: 'npx',
      args: ['-y', '@modelcontextprotocol/server-filesystem', '/home/a', '/srv/b'],
      env: [],
    });
    expect(problemsOf({ ...presetDraft('filesystem'), folders: ' \n ' })).toEqual([
      'Enter at least one folder for the filesystem server, one per line.',
    ]);
  });

  it('an empty name is refused with the name problem', () => {
    expect(problemsOf({ ...presetDraft('custom'), name: '', command: 'npx' })).toEqual([
      'Name: 1 to 32 characters, a-z, 0-9, "-" or "_".',
    ]);
  });
});

describe('name and transport checks', () => {
  it('accepts 1 to 32 characters of a-z, 0-9, - and _ only', () => {
    const ok = (name: string) => problemsOf({ ...presetDraft('fetch'), name }).length === 0;
    expect(ok('a')).toBe(true);
    expect(ok('x'.repeat(32))).toBe(true);
    expect(ok('you_com-2')).toBe(true);
    for (const bad of ['', 'Fetch', 'has space', 'dot.name', 'héllo', 'x'.repeat(33)]) {
      expect(ok(bad), bad).toBe(false);
    }
  });

  it('a new server may not take the name of a saved one', () => {
    expect(problemsOf({ ...presetDraft('fetch'), isNew: true }, undefined, ['fetch'])).toEqual([
      'A server named fetch already exists. Pick another name.',
    ]);
    expect(problemsOf({ ...presetDraft('fetch'), isNew: false }, undefined, ['fetch'])).toEqual([]);
  });

  it('a stdio server needs a command, and no control characters in it', () => {
    expect(problemsOf({ ...blankDraft(), name: 'ok', command: '   ' })).toEqual(['Command is required.']);
    expect(problemsOf({ ...blankDraft(), name: 'ok', command: 'npx\u0007' })).toEqual([
      'Command must not contain control characters.',
    ]);
  });

  it('env names must be valid and unique', () => {
    const problems = problemsOf({
      ...blankDraft(),
      name: 'ok',
      command: 'npx',
      env: [row({ name: 'GOOD_1', value: 'a' }), row({ name: '1BAD', value: 'b' }), row({ name: 'GOOD_1', value: 'c' })],
    });
    expect(problems).toEqual([
      'Variable name 1BAD must be letters, digits and "_", not starting with a digit.',
      'Variable GOOD_1 is listed more than once.',
    ]);
  });

  it('a value typed into a row with no name is refused', () => {
    expect(problemsOf({ ...blankDraft(), name: 'ok', command: 'npx', env: [row({ name: '', value: 'secret-x' })] })).toEqual([
      'Give every variable value a name.',
    ]);
  });

  it('plain http is allowed only for loopback hosts', () => {
    for (const ok of ['https://example.com/mcp', 'http://localhost:3000/mcp', 'http://api.localhost/mcp', 'http://127.0.0.1/mcp', 'http://[::1]/mcp']) {
      expect(urlProblem(ok), ok).toBeNull();
    }
    for (const bad of ['http://example.com/mcp', 'http://192.168.1.5/mcp', 'http://localhost.example.com/mcp']) {
      expect(urlProblem(bad), bad).toEqual('URL must use https unless the host is localhost.');
    }
    expect(urlProblem('')).toBe('URL is required.');
    expect(urlProblem('ftp://example.com/mcp')).toBe('URL must be an http or https URL.');
    expect(urlProblem('https://')).toBe('URL is not valid.');
    expect(urlProblem('not a url')).toBe('URL must not contain whitespace or control characters.');
  });

  it('a rejected URL is not echoed back in the problem', () => {
    const problem = urlProblem('http://example.com/mcp?key=sekrit-value-123') ?? '';
    expect(problem).not.toContain('sekrit');
    expect(problem).not.toContain('example.com');
  });

  it('isLoopbackHost covers localhost, *.localhost, 127/8 and ::1 only', () => {
    expect(isLoopbackHost('127.5.6.7')).toBe(true);
    expect(isLoopbackHost('[::1]')).toBe(true);
    expect(isLoopbackHost('localhost.example.com')).toBe(false);
    expect(isLoopbackHost('10.0.0.1')).toBe(false);
  });

  it('header names are tokens and unique ignoring case', () => {
    const problems = problemsOf({
      ...blankDraft(),
      name: 'ok',
      transport: 'http',
      url: 'https://example.com/mcp',
      headers: [row({ name: 'Authorization', value: 'a' }), row({ name: 'authorization', value: 'b' }), row({ name: 'bad header', value: 'c' })],
    });
    expect(problems).toEqual(['Header authorization is listed more than once.', 'Header name bad header is not a valid HTTP header name.']);
  });

  it('a required header with no value and nothing stored is refused', () => {
    expect(problemsOf({ ...presetDraft('you-key') })).toEqual(['Enter a value for header Authorization.']);
  });
});

describe('secret values: what is sent', () => {
  const stored = (over: Partial<SecretRow> = {}) => row({ stored: true, ...over });

  it('an untouched stored env value is omitted, so the backend keeps it', () => {
    const d = { ...blankDraft(), name: 'ok', command: 'npx', env: [stored({ name: 'API_KEY' })] };
    const input = built(d);
    expect(input.config.transport).toEqual({ type: 'stdio', command: 'npx', args: [], env: ['API_KEY'] });
    expect(input.env_values).toEqual({});
  });

  it('a removed env value is sent as an empty string, and the name stays declared', () => {
    const d = { ...blankDraft(), name: 'ok', command: 'npx', env: [stored({ name: 'API_KEY', remove: true })] };
    const input = built(d);
    expect(input.config.transport).toEqual({ type: 'stdio', command: 'npx', args: [], env: ['API_KEY'] });
    expect(input.env_values).toEqual({ API_KEY: '' });
  });

  it('a typed value replaces the stored one', () => {
    const d = { ...blankDraft(), name: 'ok', command: 'npx', env: [stored({ name: 'API_KEY', value: 'new-value' })] };
    expect(built(d).env_values).toEqual({ API_KEY: 'new-value' });
  });

  it('a removed header is dropped from the server and nothing is sent for it', () => {
    const d = {
      ...blankDraft(),
      name: 'ok',
      transport: 'http' as const,
      url: 'https://example.com/mcp',
      headers: [stored({ name: 'Authorization', remove: true }), row({ name: 'X-Other', value: 'v' })],
    };
    const input = built(d);
    expect(input.config.transport).toEqual({ type: 'http', url: 'https://example.com/mcp', headers: ['X-Other'] });
    expect(input.header_values).toEqual({ 'X-Other': 'v' });
  });

  it('a stored header that is kept and not retyped stays declared without a value', () => {
    const d = { ...blankDraft(), name: 'ok', transport: 'http' as const, url: 'https://example.com/mcp', headers: [stored({ name: 'Authorization' })] };
    const input = built(d);
    expect(input.config.transport).toEqual({ type: 'http', url: 'https://example.com/mcp', headers: ['Authorization'] });
    expect(input.header_values).toEqual({});
  });

  it('values are only sent for declared names', () => {
    const d = { ...blankDraft(), name: 'ok', command: 'npx', env: [row({ name: 'A_1', value: 'x' })], headers: [row({ name: 'Not-Sent', value: 'y' })] };
    const input = built(d);
    expect(Object.keys(input.env_values)).toEqual(['A_1']);
    expect(Object.keys(input.header_values)).toEqual([]);
  });

  it('secret values are trimmed and never appear in a problem list', () => {
    const d = { ...blankDraft(), name: 'Bad Name', command: 'npx', env: [row({ name: 'API_KEY', value: 'tok-secret-99' })] };
    expect(problemsOf(d).join('\n')).not.toContain('tok-secret-99');
  });

  it('an Authorization value with a space is sent as typed, so a scheme can be given', () => {
    expect(headerValueToSend('Authorization', 'Basic abc')).toBe('Basic abc');
    expect(headerValueToSend('authorization', 'abc')).toBe('Bearer abc');
    expect(headerValueToSend('X-Api-Key', 'abc')).toBe('abc');
  });

  it('an edit keeps the saved tool overrides and timeout, which the form does not show', () => {
    const original: McpServerConfig = {
      name: 'fetch',
      enabled: true,
      approval: 'ask',
      tools: { fetch: 'auto' },
      timeout_secs: 90,
      transport: { type: 'stdio', command: 'uvx', args: [], env: [] },
    };
    const input = built({ ...draftFromServer(view(original, { env: [], headers: [] })), approval: 'auto' }, original);
    expect(input.config.tools).toEqual({ fetch: 'auto' });
    expect(input.config.timeout_secs).toBe(90);
    expect(input.config.approval).toBe('auto');
  });
});

describe('draftFromServer', () => {
  it('marks stored names as stored, with no values, and keeps the name fixed', () => {
    const server = view(
      { name: 'ctx', enabled: false, approval: 'never', transport: { type: 'http', url: 'https://example.com/mcp', headers: ['Authorization'] } },
      { env: [], headers: ['Authorization'] },
    );
    const d = draftFromServer(server);
    expect(d).toMatchObject({ isNew: false, name: 'ctx', enabled: false, approval: 'never', transport: 'http', url: 'https://example.com/mcp' });
    expect(d.headers).toEqual([{ name: 'Authorization', value: '', stored: true, remove: false, optional: false }]);
  });
});

describe('display helpers', () => {
  it('urlForDisplay keeps origin and path, and drops the query and user:password', () => {
    expect(urlForDisplay('https://user:pw@example.com/mcp/v1?key=sekrit')).toBe('https://example.com/mcp/v1');
    expect(urlForDisplay('not a url?key=sekrit')).toBe('(invalid URL)');
  });

  it('transportSummary never shows header or env values, only names are in the config', () => {
    expect(transportSummary({ type: 'http', url: 'https://api.you.com/mcp?k=1', headers: ['Authorization'] })).toBe('https://api.you.com/mcp');
    expect(transportSummary({ type: 'stdio', command: 'npx', args: ['-y', 'pkg'], env: ['API_KEY'] })).toBe('npx -y pkg');
  });

  it('stateTag maps every state to its text and tone', () => {
    expect(stateTag({ kind: 'disabled' })).toEqual({ label: 'disabled', tone: 'muted' });
    expect(stateTag({ kind: 'connecting' })).toEqual({ label: 'connecting', tone: 'accent' });
    expect(stateTag({ kind: 'connected', tools: 1 })).toEqual({ label: 'connected · 1 tool', tone: 'ok' });
    expect(stateTag({ kind: 'connected', tools: 3 })).toEqual({ label: 'connected · 3 tools', tone: 'ok' });
    expect(stateTag({ kind: 'failed', message: 'x' })).toEqual({ label: 'failed', tone: 'warn' });
    expect(stateTag({ kind: 'needs_auth', message: 'x' })).toEqual({ label: 'needs auth', tone: 'warn' });
  });

  it('stateMessage shows only the failure and auth messages', () => {
    expect(stateMessage({ kind: 'failed', message: 'spawn failed' })).toBe('spawn failed');
    expect(stateMessage({ kind: 'needs_auth', message: 'sign in' })).toBe('sign in');
    expect(stateMessage({ kind: 'connected', tools: 2 })).toBeNull();
  });

  it('default option text shows the level the tool gets now, and the override select round-trips', () => {
    expect(defaultOptionLabel('auto')).toBe('MCP default (Auto-approve)');
    expect(defaultOptionLabel('never')).toBe('MCP default (Never (hide tools))');
    expect(overrideValue(null)).toBe('');
    expect(overrideValue('ask')).toBe('ask');
    expect(overrideFromValue('')).toBeNull();
    expect(overrideFromValue('never')).toBe('never');
  });
});

describe('mcpToolLabel', () => {
  it('splits mcp__server__tool into its server and tool', () => {
    expect(mcpToolLabel('mcp__fetch__fetch')).toEqual({ server: 'fetch', tool: 'fetch' });
    expect(mcpToolLabel('mcp__you-com__search')).toEqual({ server: 'you-com', tool: 'search' });
  });

  it('keeps any later __ in the tool name (the split is at the first __ after the prefix)', () => {
    expect(mcpToolLabel('mcp__ctx__a__b')).toEqual({ server: 'ctx', tool: 'a__b' });
  });

  it('a server slug that holds __ splits at its first __ (the chosen reading of that ambiguity)', () => {
    expect(mcpToolLabel('mcp__a__b__c')).toEqual({ server: 'a', tool: 'b__c' });
  });

  it('other names, and names with no tool or no server, give null', () => {
    for (const name of ['run_command', 'skill', 'send_keys', 'mcp__', 'mcp__server', 'mcp__server__', 'mcp____tool', 'xmcp__a__b']) {
      expect(mcpToolLabel(name), name).toBeNull();
    }
  });
});

/** A server view for the draft tests: the config and the stored names, with no live state or tools. */
function view(config: McpServerConfig, secrets: { env: string[]; headers: string[] }): McpServerView {
  return { config, state: { kind: 'disabled' }, tools: [], secrets_present: secrets };
}

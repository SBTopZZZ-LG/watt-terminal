// The MCP settings form: the presets, the draft of one server, its checks and the save input it builds. Pure, so the rules
// are tested without the DOM. The backend (src-tauri/src/core/mcp/config.rs) stays the authority; the checks here mirror its
// basics and it still refuses what they miss. Values typed into a secret row are only ever placed in the save input.

import { capDisplay } from './keysDisplay';
import type { McpLevel, McpServerConfig, McpServerInput, McpServerView, McpStateView, McpTransport } from './types';

/** A server name: 1 to 32 characters of a-z, 0-9, '-' or '_' (config.rs valid_server_name). */
const NAME_RE = /^[a-z0-9_-]{1,32}$/;
/** An env variable name (config.rs valid_env_name). */
const ENV_NAME_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;
/** An HTTP header name: RFC 7230 tchar (config.rs is_token_byte). */
const HEADER_NAME_RE = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
const CONTROL_RE = /[\u0000-\u001f\u007f]/;

export const NAME_PROBLEM = 'Name: 1 to 32 characters, a-z, 0-9, "-" or "_".';

/** The package the filesystem preset runs. Its folders are its arguments after `-y` and the package name. */
export const FILESYSTEM_PACKAGE = '@modelcontextprotocol/server-filesystem';

/** One secret row: an env variable or a header. `value` is what is typed now; `stored` says a value is already saved. */
export interface SecretRow {
  name: string;
  value: string;
  /** A value is stored for this name (Edit). The value itself is never read back. */
  stored: boolean;
  /**
   * Remove the stored value on save. An env variable keeps its name (the process environment may still supply it); a
   * header is dropped from the server. Only meaningful for a stored row.
   */
  remove: boolean;
  /** An optional header with no value is left out of the server, not refused. */
  optional: boolean;
}

/** The form's state for one server. Both transports' fields are kept, so switching the transport loses nothing typed. */
export interface McpFormDraft {
  /** A new server (its name is typed and must be free) or an edit (the name is fixed). */
  isNew: boolean;
  name: string;
  transport: 'stdio' | 'http';
  command: string;
  /** Arguments, one per line. */
  args: string;
  /** The filesystem preset's folders, one per line. Null for every other server (then `args` is used). */
  folders: string | null;
  cwd: string;
  url: string;
  env: SecretRow[];
  headers: SecretRow[];
  approval: McpLevel;
  enabled: boolean;
}

export type McpPresetKey = 'fetch' | 'you-free' | 'you-key' | 'context7' | 'filesystem' | 'custom';

/** The presets in the add panel, in order. Each one is a draft (`presetDraft`), so it saves through the same checks. */
export const MCP_PRESETS: readonly { key: McpPresetKey; label: string }[] = [
  { key: 'fetch', label: 'Fetch (uvx mcp-server-fetch)' },
  { key: 'you-free', label: 'You.com (free, no key)' },
  { key: 'you-key', label: 'You.com (API key)' },
  { key: 'context7', label: 'Context7 (optional API key)' },
  { key: 'filesystem', label: 'Filesystem (npx, folders you list)' },
  { key: 'custom', label: 'Custom' },
];

/** A draft with nothing filled in: a new stdio server, approval Ask, enabled. */
export function blankDraft(): McpFormDraft {
  return {
    isNew: true,
    name: '',
    transport: 'stdio',
    command: '',
    args: '',
    folders: null,
    cwd: '',
    url: '',
    env: [],
    headers: [],
    approval: 'ask',
    enabled: true,
  };
}

function authorizationRow(optional: boolean): SecretRow {
  return { name: 'Authorization', value: '', stored: false, remove: false, optional };
}

/**
 * The draft a preset starts from. The keyed You.com URL and the Context7 URL are the ones in each provider's docs
 * (You.com: docs.you.com MCP server page; Context7: upstash/context7 README). Keys go in an `Authorization` header, which
 * the form takes as the bare key and sends as `Bearer <key>`.
 */
export function presetDraft(key: McpPresetKey): McpFormDraft {
  const base = blankDraft();
  switch (key) {
    case 'fetch':
      return { ...base, name: 'fetch', transport: 'stdio', command: 'uvx', args: 'mcp-server-fetch' };
    case 'you-free':
      return { ...base, name: 'you-com', transport: 'http', url: 'https://api.you.com/mcp/free' };
    case 'you-key':
      return { ...base, name: 'you-com', transport: 'http', url: 'https://api.you.com/mcp', headers: [authorizationRow(false)] };
    case 'context7':
      return { ...base, name: 'context7', transport: 'http', url: 'https://mcp.context7.com/mcp', headers: [authorizationRow(true)] };
    case 'filesystem':
      return { ...base, name: 'filesystem', transport: 'stdio', command: 'npx', folders: '' };
    case 'custom':
      return base;
  }
}

/** The draft an Edit starts from: the saved config, with the stored secret names marked as stored and no values. */
export function draftFromServer(server: McpServerView): McpFormDraft {
  const config = server.config;
  const transport = config.transport;
  const base = { ...blankDraft(), isNew: false, name: config.name, approval: config.approval, enabled: config.enabled };
  if (transport.type === 'stdio') {
    return {
      ...base,
      transport: 'stdio',
      command: transport.command,
      args: transport.args.join('\n'),
      cwd: transport.cwd ?? '',
      env: transport.env.map((name) => storedRow(name, server.secrets_present.env.includes(name), false)),
    };
  }
  return {
    ...base,
    transport: 'http',
    url: transport.url,
    headers: transport.headers.map((name) => storedRow(name, server.secrets_present.headers.includes(name), false)),
  };
}

function storedRow(name: string, stored: boolean, optional: boolean): SecretRow {
  return { name, value: '', stored, remove: false, optional };
}

/** Lines of text as a list: each trimmed, empty lines dropped. */
export function splitLines(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line !== '');
}

/** The outcome of checking a draft: the save input, or the problems to show (each a whole sentence). */
export type McpBuild = { ok: true; input: McpServerInput } | { ok: false; problems: string[] };

/** A `Bearer` prefix for an Authorization value typed as a bare key (no space in it). Other values are sent as typed. */
export function headerValueToSend(name: string, typed: string): string {
  return name.toLowerCase() === 'authorization' && !/\s/.test(typed) ? `Bearer ${typed}` : typed;
}

/** Whether a URL's host is a loopback address (localhost, *.localhost, 127.0.0.0/8, ::1). */
export function isLoopbackHost(host: string): boolean {
  return host === 'localhost' || host.endsWith('.localhost') || /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host) || host === '[::1]';
}

/**
 * Why a URL cannot be saved, or null. Plain http is allowed only for a loopback host, as the backend requires. The URL is
 * not echoed: it may carry a key in its query.
 */
export function urlProblem(url: string): string | null {
  if (url === '') {
    return 'URL is required.';
  }
  if (/\s/.test(url) || CONTROL_RE.test(url)) {
    return 'URL must not contain whitespace or control characters.';
  }
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return 'URL is not valid.';
  }
  if (parsed.hostname === '') {
    return 'URL must have a host.';
  }
  if (parsed.protocol === 'https:') {
    return null;
  }
  if (parsed.protocol === 'http:') {
    return isLoopbackHost(parsed.hostname) ? null : 'URL must use https unless the host is localhost.';
  }
  return 'URL must be an http or https URL.';
}

/**
 * Checks a draft and builds the save input. `original` is the saved config of an edit: its per-tool overrides and timeout
 * are kept, since the form does not show them. `takenNames` are the names of the saved servers: a new server may not take one
 * (the backend would replace it). A secret value is sent only for a name the server declares: a stored name that is not
 * touched is left out (the backend keeps it), a typed value replaces it, and a removed env value is sent as "" (the backend
 * drops it). A removed header is dropped from the server, so its name is not declared and nothing is sent for it.
 */
export function buildServerInput(d: McpFormDraft, original?: McpServerConfig, takenNames: readonly string[] = []): McpBuild {
  const problems: string[] = [];
  const name = d.name.trim();
  if (!NAME_RE.test(name)) {
    problems.push(NAME_PROBLEM);
  } else if (d.isNew && takenNames.includes(name)) {
    problems.push(`A server named ${name} already exists. Pick another name.`);
  }

  let transport: McpTransport;
  let envValues: Record<string, string> = {};
  let headerValues: Record<string, string> = {};
  if (d.transport === 'stdio') {
    const command = d.command.trim();
    if (command === '') {
      problems.push('Command is required.');
    } else if (CONTROL_RE.test(command)) {
      problems.push('Command must not contain control characters.');
    }
    const cwd = d.cwd.trim();
    if (CONTROL_RE.test(cwd)) {
      problems.push('Working folder must not contain control characters.');
    }
    const args = d.folders !== null ? folderArgs(d.folders, problems) : splitLines(d.args);
    if (args.some((arg) => CONTROL_RE.test(arg))) {
      problems.push('Arguments must not contain control characters.');
    }
    const env = envRows(d.env, problems);
    envValues = env.values;
    transport = { type: 'stdio', command, args, ...(cwd === '' ? {} : { cwd }), env: env.names };
  } else {
    const url = d.url.trim();
    const problem = urlProblem(url);
    if (problem !== null) {
      problems.push(problem);
    }
    const headers = headerRows(d.headers, problems);
    headerValues = headers.values;
    transport = { type: 'http', url, headers: headers.names };
  }

  if (problems.length > 0) {
    return { ok: false, problems };
  }
  const config: McpServerConfig = { name, enabled: d.enabled, approval: d.approval, transport };
  if (original?.tools !== undefined && Object.keys(original.tools).length > 0) {
    config.tools = original.tools;
  }
  if (original?.timeout_secs !== undefined) {
    config.timeout_secs = original.timeout_secs;
  }
  return { ok: true, input: { config, env_values: envValues, header_values: headerValues } };
}

function folderArgs(folders: string, problems: string[]): string[] {
  const dirs = splitLines(folders);
  if (dirs.length === 0) {
    problems.push('Enter at least one folder for the filesystem server, one per line.');
  }
  return ['-y', FILESYSTEM_PACKAGE, ...dirs];
}

function envRows(rows: readonly SecretRow[], problems: string[]): { names: string[]; values: Record<string, string> } {
  const names: string[] = [];
  const values: Record<string, string> = {};
  for (const row of rows) {
    const name = row.name.trim();
    const typed = row.value.trim();
    if (name === '') {
      if (typed !== '') {
        problems.push('Give every variable value a name.');
      }
      continue;
    }
    if (!ENV_NAME_RE.test(name)) {
      problems.push(`Variable name ${name} must be letters, digits and "_", not starting with a digit.`);
      continue;
    }
    if (names.includes(name)) {
      problems.push(`Variable ${name} is listed more than once.`);
      continue;
    }
    names.push(name);
    if (row.remove && row.stored) {
      values[name] = '';
    } else if (typed !== '') {
      values[name] = typed;
    }
  }
  return { names, values };
}

function headerRows(rows: readonly SecretRow[], problems: string[]): { names: string[]; values: Record<string, string> } {
  const names: string[] = [];
  const values: Record<string, string> = {};
  for (const row of rows) {
    const name = row.name.trim();
    const typed = row.value.trim();
    if (name === '') {
      if (typed !== '') {
        problems.push('Give every header value a name.');
      }
      continue;
    }
    if (!HEADER_NAME_RE.test(name)) {
      problems.push(`Header name ${name} is not a valid HTTP header name.`);
      continue;
    }
    if (names.some((other) => other.toLowerCase() === name.toLowerCase())) {
      problems.push(`Header ${name} is listed more than once.`);
      continue;
    }
    if (row.remove && row.stored) {
      continue;
    }
    if (typed !== '') {
      names.push(name);
      values[name] = headerValueToSend(name, typed);
    } else if (row.stored) {
      names.push(name);
    } else if (!row.optional) {
      names.push(name);
      problems.push(`Enter a value for header ${name}.`);
    }
  }
  return { names, values };
}

/** The level's name in the UI. The words are the user's, the values are the backend's. */
export const LEVEL_LABEL: Record<McpLevel, string> = {
  ask: 'Ask each time',
  auto: 'Auto-approve',
  never: 'Never (hide tools)',
};

/** The option text of a tool's "use the server's level" choice, showing the level the tool gets now. */
export function defaultOptionLabel(effective: McpLevel): string {
  return `MCP default (${LEVEL_LABEL[effective]})`;
}

/** The tag on a server row: its text and the colour class it takes. */
export interface McpStateTag {
  label: string;
  tone: 'ok' | 'muted' | 'accent' | 'warn';
}

export function stateTag(state: McpStateView): McpStateTag {
  switch (state.kind) {
    case 'disabled':
      return { label: 'disabled', tone: 'muted' };
    case 'connecting':
      return { label: 'connecting', tone: 'accent' };
    case 'connected':
      return { label: `connected · ${state.tools} ${state.tools === 1 ? 'tool' : 'tools'}`, tone: 'ok' };
    case 'failed':
      return { label: 'failed', tone: 'warn' };
    case 'needs_auth':
      return { label: 'needs auth', tone: 'warn' };
  }
}

/** The message a failed or unauthorised server shows under its row, or null. As the backend reports it. */
export function stateMessage(state: McpStateView): string | null {
  return state.kind === 'failed' || state.kind === 'needs_auth' ? state.message : null;
}

/**
 * A URL as the settings show it: origin and path only. The query (where a key may sit) and any user:password are dropped.
 * An unparsable URL shows as a placeholder, never as its text.
 */
export function urlForDisplay(url: string): string {
  try {
    const parsed = new URL(url);
    return `${parsed.origin}${parsed.pathname}`;
  } catch {
    return '(invalid URL)';
  }
}

/** One line for a server's transport. Header and env values are never in the config, so they cannot show here. */
export function transportSummary(transport: McpTransport): string {
  if (transport.type === 'http') {
    return urlForDisplay(transport.url);
  }
  return capDisplay([transport.command, ...transport.args].join(' '), 100);
}

/** An MCP tool's name as the model sees it, split into its server and its own tool name. */
export interface McpToolLabel {
  server: string;
  tool: string;
}

const MCP_PREFIX = 'mcp__';

/**
 * `mcp__<server>__<tool>` split at the first `__` after the prefix, so the tool keeps any `__` it has. Server slugs may hold
 * `_`, so a server whose name contains `__` splits at its first `__`: that name is ambiguous, and this is the chosen reading.
 * Any other name (a built-in tool, a skill) gives null.
 */
export function mcpToolLabel(name: string): McpToolLabel | null {
  if (!name.startsWith(MCP_PREFIX)) {
    return null;
  }
  const rest = name.slice(MCP_PREFIX.length);
  const at = rest.indexOf('__');
  if (at <= 0 || at + 2 >= rest.length) {
    return null;
  }
  return { server: rest.slice(0, at), tool: rest.slice(at + 2) };
}

/** The tool's override select value: '' for the server's level, else the level. */
export function overrideValue(level: McpLevel | null): string {
  return level ?? '';
}

/** The select's value back to a level: '' clears the override. */
export function overrideFromValue(value: string): McpLevel | null {
  return value === '' ? null : (value as McpLevel);
}

// The AI provider settings form: the editable draft and the pure rules around it. Nothing here calls the backend or the DOM.
//
// The Rust side (src-tauri/src/core/settings) stays authoritative. validateDraft mirrors its rules lightly, so the form can
// point at a field before a save is attempted. A refusal from the backend is still shown as it is.
//
// Secrets: the typed API key and the typed header values live only in the draft while the form is open. They are never
// filled in from the store, and buildSaveInput is the only place they leave the draft.

import type {
  FetchSummary,
  HeaderEntry,
  Model,
  ModelRef,
  ModelSource,
  ModelVariant,
  Provider,
  ProviderPreset,
  SaveProviderInput,
  SecretsPresent,
  Settings,
  VariantSource,
} from './types';

/** The largest value the backend stores for a token count or a context limit (Rust u32). */
const U32_MAX = 4294967295;
const MAX_ID_CHARS = 40;
const MAX_NAME_CHARS = 80;
const MAX_MODEL_ID_CHARS = 200;
const MAX_HEADER_VALUE_CHARS = 500;
const CHAT_SUFFIX = '/chat/completions';
const LOCAL_HOSTS = ['localhost', '127.0.0.1', '[::1]'];

const ID_RE = /^[a-z0-9][a-z0-9_-]{0,39}$/;
const VARIANT_ID_RE = /^[a-z0-9][a-z0-9_-]{0,31}$/;
const MAX_VARIANT_LABEL_CHARS = 80;
const MAX_VARIANTS_PER_MODEL = 16;
const MAX_VARIANT_BODY_BYTES = 4096;
const MAX_VARIANT_DEPTH = 8;
/** Body keys a variant must never carry. The backend skips them again if one slips through. */
const PROTECTED_BODY_KEYS = new Set([
  'model',
  'messages',
  'stream',
  'tools',
  'stream_options',
  'max_tokens',
  'max_completion_tokens',
  'models',
]);
/** RFC 7230 tchar: the characters a header name may use. */
const TOKEN_RE = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
/** Header names that look like they carry a credential. Such a row's Secret toggle is ticked until the user touches it. */
const CREDENTIAL_NAME_RE = /key|token|secret|auth|password|cookie/i;
/** Control characters (Rust char::is_control): C0, DEL and C1. */
const CONTROL_RE = /[\u0000-\u001f\u007f-\u009f]/;
const WHITESPACE_RE = /\s/;

export const ID_HELP = "Use 1 to 40 characters: a-z, 0-9, '-' or '_', starting with a letter or digit.";

/** One header row. A secret header's value is only what the user typed; the stored value is never read back. */
export interface HeaderDraft {
  /** Stable React key. Not sent. */
  key: string;
  name: string;
  /** Plain headers: the value as saved or typed. Secret headers: what the user typed, normally "". */
  value: string;
  secret: boolean;
  /** Secret headers: the user asked to remove the stored value. */
  clearStored: boolean;
  /** The name this header had when it was saved as a secret header. Null for a new row, a preset row or a plain header. */
  savedName: string | null;
  /**
   * The user has set this row's Secret toggle by hand. Until then, a credential-looking name ticks it (see renameHeader).
   * UI state only: it is never sent, and buildSaveInput ignores it.
   */
  secretTouched: boolean;
}

/** One model row. `context_limit` is the typed override, as text: blank means none. */
export interface ModelDraft {
  id: string;
  source: ModelSource;
  context_limit: string;
  /** What the provider reported when the list was fetched. Shown, never edited. */
  provider_context: number | null;
  variants: VariantDraft[];
}

/** One variant row. `body` is the typed JSON text; the parsed object is what is saved. */
export interface VariantDraft {
  /** Stable React key. Not sent. */
  key: string;
  id: string;
  label: string;
  body: string;
  source: VariantSource;
}

/** The editable state of one provider. Numbers are text while typing and parsed in buildSaveInput. */
export interface ProviderDraft {
  /** A provider not saved yet. Its id is editable until the first save. */
  isNew: boolean;
  /** Set once the user types the id. Until then the id follows the name. */
  idEdited: boolean;
  id: string;
  name: string;
  chat_url: string;
  models_url: string;
  stream: boolean;
  max_tokens: string;
  headers: HeaderDraft[];
  models: ModelDraft[];
  /** The typed API key. Never filled from the store. */
  apiKey: string;
  /** The user asked to remove the stored key. */
  clearKey: boolean;
  /** The user asked to replace the stored key, so the input is shown. */
  keyEditing: boolean;
  /** Which secrets the store has for this provider, as the backend reported them. Presence only. */
  storedKey: boolean;
  storedHeaders: string[];
}

/** Field problems keyed the way the editor shows them. Empty strings are never stored: a missing key means no problem. */
export interface DraftProblems {
  id?: string;
  name?: string;
  chat_url?: string;
  models_url?: string;
  max_tokens?: string;
  /** Keyed by header row key. */
  headers: Record<string, string>;
  /** Keyed by model id. */
  models: Record<string, string>;
  /** Keyed by variant row key. */
  variants: Record<string, string>;
}

export interface ModelOption {
  key: string;
  ref: ModelRef;
  /** "Provider · model", as the default-model select shows it. */
  label: string;
}

let headerKeySeq = 0;
const nextHeaderKey = () => `h${++headerKeySeq}`;
let variantKeySeq = 0;
const nextVariantKey = () => `v${++variantKeySeq}`;

/** An empty header row, with a key no other row has. */
export function newHeaderDraft(): HeaderDraft {
  return { key: nextHeaderKey(), name: '', value: '', secret: false, clearStored: false, savedName: null, secretTouched: false };
}

/** An empty variant row of the given source, with a key no other row has. The body starts as blank text. */
export function newVariantDraft(source: VariantSource): VariantDraft {
  return { key: nextVariantKey(), id: '', label: '', body: '', source };
}

/** Whether a header name looks like it carries a credential (key, token, secret, auth, password or cookie). */
export function isCredentialName(name: string): boolean {
  return CREDENTIAL_NAME_RE.test(name);
}

/**
 * The header row after its name is typed. An untouched Secret toggle is ticked when the name looks like a credential, and is
 * never unticked: a name that stops matching leaves a ticked row ticked.
 */
export function renameHeader(h: HeaderDraft, name: string): HeaderDraft {
  return { ...h, name, secret: h.secret || (!h.secretTouched && isCredentialName(name)) };
}

/** The row after its Secret toggle is set by hand. From then on the name no longer ticks it. */
export function toggleSecret(h: HeaderDraft, secret: boolean): HeaderDraft {
  return { ...h, secret, clearStored: false, secretTouched: true };
}

/** A secret header with a row showing a credential-looking name that the user has not decided about: the note under it. */
export function isAutoTicked(h: HeaderDraft): boolean {
  return h.secret && !h.secretTouched && isCredentialName(h.name);
}

/** Text that is a positive whole number that fits the backend's u32, or null when blank or not one. */
export function parsePositiveInt(text: string): number | null {
  const t = text.trim();
  if (!/^\d+$/.test(t)) {
    return null;
  }
  const n = Number(t);
  return n >= 1 && n <= U32_MAX ? n : null;
}

/** What a commit of the global context limit does: refuse the text, leave the saved limit alone, or send `value`. */
export type LimitCommit = { kind: 'invalid' } | { kind: 'unchanged' } | { kind: 'send'; value: number | null };

/** Reads the typed global context limit against the saved one. Blank means no limit (null). */
export function limitCommit(text: string, saved: number | null): LimitCommit {
  if (text.trim() !== '' && parsePositiveInt(text) === null) {
    return { kind: 'invalid' };
  }
  const value = parsePositiveInt(text);
  return value === saved ? { kind: 'unchanged' } : { kind: 'send', value };
}

/** A slug for a provider id from its name: lower case, a-z 0-9 and '-', at most 40 characters, unique among takenIds. */
export function slugify(name: string, takenIds: readonly string[]): string {
  const base = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+/, '')
    .slice(0, MAX_ID_CHARS)
    .replace(/-+$/, '');
  if (base === '') {
    return '';
  }
  const taken = new Set(takenIds);
  if (!taken.has(base)) {
    return base;
  }
  for (let n = 2; ; n += 1) {
    const suffix = `-${n}`;
    const candidate = `${base.slice(0, MAX_ID_CHARS - suffix.length).replace(/-+$/, '')}${suffix}`;
    if (!taken.has(candidate)) {
      return candidate;
    }
  }
}

/** A row from a saved header. Saved rows count as touched, so their Secret toggle is never ticked for them. */
function headerDraft(h: HeaderEntry): HeaderDraft {
  return {
    key: nextHeaderKey(),
    name: h.name,
    value: h.value,
    secret: h.secret,
    clearStored: false,
    savedName: h.secret ? h.name : null,
    secretTouched: true,
  };
}

/** A saved variant as an editable row. The body is stringified with a 2-space indent; the save parses it back. */
function variantDraft(v: ModelVariant): VariantDraft {
  return { key: nextVariantKey(), id: v.id, label: v.label, body: JSON.stringify(v.body, null, 2), source: v.source };
}
/** A row from a preset's header. Untouched, so a credential-looking name ticks its Secret toggle. */
function presetHeaderDraft(h: HeaderEntry): HeaderDraft {
  return { ...newHeaderDraft(), name: h.name, value: h.value, secret: h.secret || isCredentialName(h.name) };
}

/** A saved provider as an editable draft. `present` says which of its secrets are stored; the draft never holds them. */
export function draftFromProvider(provider: Provider, present?: SecretsPresent): ProviderDraft {
  return {
    isNew: false,
    idEdited: true,
    id: provider.id,
    name: provider.name,
    chat_url: provider.chat_url,
    models_url: provider.models_url ?? '',
    stream: provider.stream,
    max_tokens: provider.max_tokens === null ? '' : String(provider.max_tokens),
    headers: provider.headers.map(headerDraft),
    models: provider.models.map((m) => ({
      id: m.id,
      source: m.source,
      context_limit: m.context_limit === null ? '' : String(m.context_limit),
      provider_context: m.provider_context,
      variants: (m.variants ?? []).map(variantDraft),
    })),
    apiKey: '',
    clearKey: false,
    keyEditing: false,
    storedKey: present?.api_key ?? false,
    storedHeaders: present?.headers ?? [],
  };
}

/** A new provider starting from a preset. The id is worked out from the name, unique among existingIds. */
export function draftFromPreset(preset: ProviderPreset, existingIds: readonly string[]): ProviderDraft {
  const p = preset.provider;
  return {
    isNew: true,
    idEdited: false,
    id: slugify(p.name, existingIds),
    name: p.name,
    chat_url: p.chat_url,
    models_url: p.models_url ?? '',
    stream: p.stream,
    max_tokens: p.max_tokens === null ? '' : String(p.max_tokens),
    headers: p.headers.map(presetHeaderDraft),
    models: [],
    apiKey: '',
    clearKey: false,
    keyEditing: false,
    storedKey: false,
    storedHeaders: [],
  };
}

/** A blank new provider, for "Custom provider". */
export function draftNew(): ProviderDraft {
  return {
    isNew: true,
    idEdited: false,
    id: '',
    name: '',
    chat_url: '',
    models_url: '',
    stream: true,
    max_tokens: '',
    headers: [],
    models: [],
    apiKey: '',
    clearKey: false,
    keyEditing: false,
    storedKey: false,
    storedHeaders: [],
  };
}

/** The name as typed. While the id has not been typed by hand, a new provider's id follows the name. */
export function withName(draft: ProviderDraft, name: string, takenIds: readonly string[]): ProviderDraft {
  if (!draft.isNew || draft.idEdited) {
    return { ...draft, name };
  }
  return { ...draft, name, id: slugify(name, takenIds) };
}

/** The id as typed, for a new provider. */
export function withId(draft: ProviderDraft, id: string): ProviderDraft {
  return { ...draft, id, idEdited: true };
}

/** What the save sends for the API key: a typed key wins; a requested removal sends ""; otherwise null (keep). */
function apiKeyField(draft: ProviderDraft): string | null {
  const typed = draft.apiKey.trim();
  if (typed !== '') {
    return typed;
  }
  return draft.clearKey ? '' : null;
}

/**
 * A variant body text back as an object. Invalid JSON, and JSON that is not an object (an array, a string, a number,
 * null), is `{}`: the save is blocked by validation, which reports the problem.
 */
export function parseVariantBody(text: string): Record<string, unknown> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return {};
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    return {};
  }
  return parsed as Record<string, unknown>;
}

/** How deep a parsed JSON value nests: a scalar is 0, an object or array is 1 over its deepest member. Mirrors the backend. */
function jsonDepth(value: unknown): number {
  if (Array.isArray(value)) {
    return 1 + value.map(jsonDepth).reduce((a, b) => Math.max(a, b), 0);
  }
  if (typeof value === 'object' && value !== null) {
    return 1 + Object.values(value).map(jsonDepth).reduce((a, b) => Math.max(a, b), 0);
  }
  return 0;
}

/** The UTF-8 byte length of text, as the backend's serialized-bytes cap counts it. */
function utf8Length(text: string): number {
  return new TextEncoder().encode(text).length;
}

/**
 * The save request for a draft. Untouched secrets are not sent: api_key is null unless the key was typed or removed, and
 * header_secrets only has the secret headers the user typed a value for or asked to clear. A secret header's entry in
 * `provider.headers` always has value "". The models list is sent whole, since the backend replaces it.
 */
export function buildSaveInput(draft: ProviderDraft): SaveProviderInput {
  const headers: HeaderEntry[] = draft.headers.map((h) => {
    const entry: HeaderEntry = { name: h.name.trim(), value: h.secret ? '' : h.value.trim(), secret: h.secret };
    // A saved secret header renamed with no new value keeps its stored value: the backend moves it from the old name.
    if (h.secret && !h.clearStored && h.value.trim() === '' && h.savedName !== null && h.name.trim().toLowerCase() !== h.savedName.toLowerCase()) {
      entry.previous_name = h.savedName;
    }
    return entry;
  });
  const header_secrets: Record<string, string> = {};
  for (const h of draft.headers) {
    if (!h.secret) {
      continue;
    }
    const typed = h.value.trim();
    if (typed !== '') {
      header_secrets[h.name.trim()] = typed;
    } else if (h.clearStored) {
      header_secrets[h.name.trim()] = '';
    }
  }
  const models: Model[] = draft.models.map((m) => {
    const variants: ModelVariant[] = m.variants.map((v) => ({
      id: v.id.trim(),
      label: v.label.trim(),
      body: parseVariantBody(v.body),
      source: v.source,
    }));
    return {
      id: m.id.trim(),
      context_limit: parsePositiveInt(m.context_limit),
      provider_context: m.provider_context,
      source: m.source,
      // Left out when empty, so a file from before variants round-trips byte-identical.
      ...(variants.length > 0 ? { variants } : {}),
    };
  });
  const modelsUrl = draft.models_url.trim();
  return {
    provider: {
      id: draft.id.trim(),
      name: draft.name.trim(),
      chat_url: draft.chat_url.trim(),
      models_url: modelsUrl === '' ? null : modelsUrl,
      headers,
      stream: draft.stream,
      max_tokens: parsePositiveInt(draft.max_tokens),
      models,
    },
    api_key: apiKeyField(draft),
    header_secrets,
  };
}

/**
 * Whether the draft differs from its baseline: what is saved, or for a provider not saved yet, the draft it started from (a
 * blank one or a preset). With no baseline (null) the draft counts as unsaved.
 */
export function isDirty(draft: ProviderDraft, saved: ProviderDraft | null): boolean {
  if (saved === null) {
    return true;
  }
  return JSON.stringify(buildSaveInput(draft)) !== JSON.stringify(buildSaveInput(saved));
}

/** Why a URL cannot be saved, as the rest of the sentence after its label; null when it can. Mirrors Rust validate_url. */
export function urlProblem(url: string, label: string): string | null {
  const u = url.trim();
  if (u === '') {
    return `${label} is required.`;
  }
  if (CONTROL_RE.test(u) || WHITESPACE_RE.test(u)) {
    return `${label} must not contain spaces or control characters.`;
  }
  let parsed: URL;
  try {
    parsed = new URL(u);
  } catch {
    return `${label} is not a valid URL.`;
  }
  if (parsed.protocol === 'http:') {
    if (!LOCAL_HOSTS.includes(parsed.hostname)) {
      return `${label} uses plain http. Only localhost, 127.0.0.1 and [::1] may.`;
    }
  } else if (parsed.protocol !== 'https:') {
    return `${label} must use http or https.`;
  }
  if (parsed.username !== '' || parsed.password !== '') {
    return `${label} must not contain a user name or password.`;
  }
  if (u.includes('#')) {
    return `${label} must not contain a fragment.`;
  }
  if (parsed.hostname === '') {
    return `${label} must have a host.`;
  }
  return null;
}

/** Whether a chat URL is plain http to a local address, which may run without a key. Mirrors Rust allows_empty_key. */
export function isLocalHttp(url: string): boolean {
  try {
    const parsed = new URL(url.trim());
    return parsed.protocol === 'http:' && LOCAL_HOSTS.includes(parsed.hostname);
  } catch {
    return false;
  }
}

/** The model-list URL the backend will use, or null when there is none. Mirrors Rust models_url_for. */
export function modelsUrlFor(draft: Pick<ProviderDraft, 'models_url' | 'chat_url'>): string | null {
  const explicit = draft.models_url.trim();
  if (explicit !== '') {
    return explicit;
  }
  const chat = draft.chat_url.trim();
  return chat.endsWith(CHAT_SUFFIX) ? `${chat.slice(0, -CHAT_SUFFIX.length)}/models` : null;
}

/** The host of a chat URL, for the provider list. The URL itself when it does not parse. */
export function chatHost(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

/** The host and port of a URL, or null when it is missing, empty, does not parse or has no host. Never throws. */
export function hostOf(url: string | null): string | null {
  if (url === null || url.trim() === '') {
    return null;
  }
  try {
    const host = new URL(url.trim()).host;
    return host === '' ? null : host;
  } catch {
    return null;
  }
}

/**
 * The warning shown while a saved provider's URL is edited to another host. The saved key and secret headers would then be
 * sent there, so the notice asks the user to re-enter them. Only for a saved provider that has a stored key or secret header
 * (hasStoredSecrets). Null for a new provider, and when no host changed. It never blocks a save.
 */
export function hostChangeNotice(saved: ProviderDraft | null, draft: ProviderDraft, hasStoredSecrets: boolean): string | null {
  if (saved === null || draft.isNew || !hasStoredSecrets) {
    return null;
  }
  const changed: string[] = [];
  const chatNow = hostOf(draft.chat_url);
  if (chatNow !== null && hostOf(saved.chat_url) !== null && chatNow !== hostOf(saved.chat_url)) {
    changed.push(chatNow);
  }
  // The model list URL in use: the explicit one, or the one taken from the chat URL.
  const modelsNow = hostOf(modelsUrlFor(draft));
  if (modelsNow !== null && hostOf(modelsUrlFor(saved)) !== null && modelsNow !== hostOf(modelsUrlFor(saved)) && !changed.includes(modelsNow)) {
    changed.push(modelsNow);
  }
  if (changed.length === 0) {
    return null;
  }
  return `The saved API key and secret headers will be sent to ${changed.join(' and ')}. Re-enter them if that host is not the one you trust.`;
}

/** Field problems for a draft. `takenIds` are the ids of the other providers. Mirrors the rules in Rust settings validate(). */
export function validateDraft(draft: ProviderDraft, takenIds: readonly string[]): DraftProblems {
  const problems: DraftProblems = { headers: {}, models: {}, variants: {} };

  const id = draft.id.trim();
  if (!ID_RE.test(id)) {
    problems.id = ID_HELP;
  } else if (takenIds.includes(id)) {
    problems.id = `The id "${id}" is already used by another provider.`;
  }

  const name = draft.name.trim();
  if (name === '') {
    problems.name = 'Name is required.';
  } else if ([...draft.name].length > MAX_NAME_CHARS) {
    problems.name = `Name must be at most ${MAX_NAME_CHARS} characters.`;
  } else if (CONTROL_RE.test(draft.name)) {
    problems.name = 'Name must not contain control characters.';
  }

  const chatProblem = urlProblem(draft.chat_url, 'Chat URL');
  if (chatProblem !== null) {
    problems.chat_url = chatProblem;
  }
  if (draft.models_url.trim() !== '') {
    const modelsProblem = urlProblem(draft.models_url, 'Model list URL');
    if (modelsProblem !== null) {
      problems.models_url = modelsProblem;
    }
  }

  if (draft.max_tokens.trim() !== '' && parsePositiveInt(draft.max_tokens) === null) {
    problems.max_tokens = 'Max output tokens must be a whole number of at least 1, or blank.';
  }

  const seenHeaders = new Set<string>();
  for (const h of draft.headers) {
    const name = h.name.trim();
    const lower = name.toLowerCase();
    if (name === '') {
      problems.headers[h.key] = 'Header name is required.';
    } else if (!TOKEN_RE.test(name)) {
      problems.headers[h.key] = `"${name}" is not a valid HTTP header name.`;
    } else if (seenHeaders.has(lower)) {
      problems.headers[h.key] = `Header "${name}" is listed more than once.`;
    } else if (lower === 'authorization' && !h.secret) {
      problems.headers[h.key] = 'Authorization must be a secret header, so its value stays out of settings.json.';
    } else if (CONTROL_RE.test(h.value)) {
      problems.headers[h.key] = 'Header value must not contain control characters.';
    } else if (!h.secret && [...h.value].length > MAX_HEADER_VALUE_CHARS) {
      problems.headers[h.key] = `Header value must be at most ${MAX_HEADER_VALUE_CHARS} characters.`;
    }
    seenHeaders.add(lower);
  }

  const seenModels = new Set<string>();
  for (const m of draft.models) {
    const id = m.id.trim();
    let message: string | undefined;
    if (id === '') {
      message = 'Model id is required.';
    } else if ([...id].length > MAX_MODEL_ID_CHARS) {
      message = `Model id must be at most ${MAX_MODEL_ID_CHARS} characters.`;
    } else if (CONTROL_RE.test(id)) {
      message = 'Model id must not contain control characters.';
    } else if (seenModels.has(id)) {
      message = `Model "${id}" is listed more than once.`;
    } else if (m.context_limit.trim() !== '' && parsePositiveInt(m.context_limit) === null) {
      message = 'Context limit must be a whole number of at least 1, or blank.';
    }
    seenModels.add(id);
    if (message !== undefined) {
      problems.models[m.id] = message;
    }
    if (m.variants.length > MAX_VARIANTS_PER_MODEL) {
      const cap = `Model "${id}" has ${m.variants.length} variants, at most ${MAX_VARIANTS_PER_MODEL} are allowed.`;
      problems.models[m.id] = problems.models[m.id] === undefined ? cap : `${problems.models[m.id]} ${cap}`;
    }
    const seenManual = new Set<string>();
    for (const v of m.variants) {
      const variantMessage = validateVariantDraft(v, seenManual);
      if (variantMessage !== undefined) {
        problems.variants[v.key] = variantMessage;
      }
    }
  }

  return problems;
}

/**
 * The problem with one variant row, or undefined when it checks out. Mirrors the backend's `validate_variant`: the id
 * slug, a short label without control characters, a non-empty JSON object body with no protected keys that serializes
 * small and nests shallow. Duplicate ids are an error only among manual variants: a manual and an auto may share an
 * id, and the manual wins. One string per row; a row with several problems reports the first.
 */
function validateVariantDraft(v: VariantDraft, seenManual: Set<string>): string | undefined {
  const id = v.id.trim();
  if (!VARIANT_ID_RE.test(id)) {
    return 'Variant id must be 1 to 32 characters of a-z, 0-9, \'-\' or \'_\', starting with a letter or digit.';
  }
  if (v.label.trim() === '') {
    return 'Variant label is required.';
  }
  if ([...v.label].length > MAX_VARIANT_LABEL_CHARS) {
    return `Variant label must be at most ${MAX_VARIANT_LABEL_CHARS} characters.`;
  }
  if (CONTROL_RE.test(v.label)) {
    return 'Variant label must not contain control characters.';
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(v.body);
  } catch {
    return 'Variant body must be a JSON object.';
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    return 'Variant body must be a JSON object.';
  }
  const body = parsed as Record<string, unknown>;
  if (Object.keys(body).length === 0) {
    return 'Variant body must be a non-empty JSON object.';
  }
  for (const key of Object.keys(body)) {
    if (PROTECTED_BODY_KEYS.has(key)) {
      return `Variant body must not set ${JSON.stringify(key)}.`;
    }
  }
  if (utf8Length(JSON.stringify(body)) > MAX_VARIANT_BODY_BYTES) {
    return `Variant body must serialize to at most ${MAX_VARIANT_BODY_BYTES} bytes.`;
  }
  const depth = Object.values(body).map(jsonDepth).reduce((a, b) => Math.max(a, b), 0) + 1;
  if (depth > MAX_VARIANT_DEPTH) {
    return `Variant body must nest at most ${MAX_VARIANT_DEPTH} levels deep.`;
  }
  if (v.source === 'manual') {
    if (seenManual.has(id)) {
      return `Variant "${id}" is listed more than once.`;
    }
    seenManual.add(id);
  }
  return undefined;
}

/** Whether validateDraft found anything. Save stays disabled while it did. */
export function hasProblems(p: DraftProblems): boolean {
  return (
    p.id !== undefined ||
    p.name !== undefined ||
    p.chat_url !== undefined ||
    p.models_url !== undefined ||
    p.max_tokens !== undefined ||
    Object.keys(p.headers).length > 0 ||
    Object.keys(p.models).length > 0 ||
    Object.keys(p.variants).length > 0
  );
}

/** The result of a model-list fetch, as one line. */
export function fetchSummaryText(s: FetchSummary): string {
  return `+${s.added} new, ${s.updated} updated, ${s.removed} removed. ${s.total} models in the list.`;
}

/** A stable string for a model reference, for select values. Provider ids cannot contain a quote or a comma, but JSON keeps it unambiguous anyway. The variant is never part of it: the model select names the base model. */
export function modelRefKey(ref: ModelRef): string {
  return JSON.stringify([ref.provider_id, ref.model_id]);
}

/**
 * A model's variants with manual and auto entries sharing an id merged: the manual one wins. First wins within one
 * source. Mirrors the backend's `Model::variant` lookup.
 */
export function effectiveVariants(model: Model): ModelVariant[] {
  const variants = model.variants ?? [];
  const out: ModelVariant[] = [];
  const seen = new Set<string>();
  for (const v of variants) {
    if (v.source === 'manual' && !seen.has(v.id)) {
      seen.add(v.id);
      out.push(v);
    }
  }
  for (const v of variants) {
    if (v.source !== 'manual' && !seen.has(v.id)) {
      seen.add(v.id);
      out.push(v);
    }
  }
  return out;
}

/** One variant option of the default-model variant select: its id, and its label (manual ones are tagged). */
export interface VariantOption {
  id: string;
  label: string;
}

/**
 * The variant options for a default-model ref: the effective variants of its model, with manual ones tagged. Empty
 * when there is no default, or its model is gone.
 */
export function variantOptions(settings: Settings, ref: ModelRef | null): VariantOption[] {
  if (ref === null) {
    return [];
  }
  const model = settings.providers.find((p) => p.id === ref.provider_id)?.models.find((m) => m.id === ref.model_id);
  if (model === undefined) {
    return [];
  }
  return effectiveVariants(model).map((v) => ({ id: v.id, label: v.source === 'manual' ? `${v.label} (manual)` : v.label }));
}

/** Every model of every provider, in provider order, for the default-model select. */
export function modelOptions(settings: Settings): ModelOption[] {
  return settings.providers.flatMap((provider) =>
    provider.models.map((model) => {
      const ref: ModelRef = { provider_id: provider.id, model_id: model.id };
      return { key: modelRefKey(ref), ref, label: `${provider.name} · ${model.id}` };
    }),
  );
}

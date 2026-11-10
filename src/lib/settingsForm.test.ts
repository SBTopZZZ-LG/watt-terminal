import { describe, expect, it } from 'vitest';
import {
  buildSaveInput,
  chatHost,
  draftFromPreset,
  draftFromProvider,
  draftNew,
  fetchSummaryText,
  hasProblems,
  isDirty,
  isLocalHttp,
  modelOptions,
  modelRefKey,
  limitCommit,
  modelsUrlFor,
  newHeaderDraft,
  parsePositiveInt,
  slugify,
  urlProblem,
  validateDraft,
  withId,
  withName,
} from './settingsForm';
import type { ProviderDraft } from './settingsForm';
import type { Provider, ProviderPreset, Settings } from './types';

const SAVED: Provider = {
  id: 'acme',
  name: 'Acme',
  chat_url: 'https://api.acme.test/v1/chat/completions',
  models_url: null,
  headers: [
    { name: 'X-Team', value: 'blue', secret: false },
    { name: 'X-Key', value: '', secret: true },
  ],
  stream: true,
  max_tokens: 512,
  models: [
    { id: 'big/one', context_limit: 8000, provider_context: 128000, source: 'fetched' },
    { id: 'mini', context_limit: null, provider_context: null, source: 'custom' },
  ],
};

const PRESENT = { api_key: true, headers: ['X-Key'] };

/** A saved provider's draft, as the editor starts from. */
function saved(over: Partial<Provider> = {}, present = PRESENT): ProviderDraft {
  return draftFromProvider({ ...SAVED, ...over }, present);
}

describe('parsePositiveInt', () => {
  it('reads whole numbers of at least 1, trimmed', () => {
    expect(parsePositiveInt('42')).toBe(42);
    expect(parsePositiveInt('  7 ')).toBe(7);
    expect(parsePositiveInt('007')).toBe(7);
    expect(parsePositiveInt('4294967295')).toBe(4294967295);
  });

  it('rejects blank, zero, negative, fractional, non-numeric and values over u32', () => {
    for (const text of ['', '   ', '0', '-1', '1.5', '1e3', 'abc', '12px', '4294967296']) {
      expect(parsePositiveInt(text), text).toBeNull();
    }
  });
});

describe('limitCommit', () => {
  it('refuses text that is neither blank nor a whole number of at least 1', () => {
    for (const text of ['0', '-3', 'abc', '1.5']) {
      expect(limitCommit(text, null), text).toEqual({ kind: 'invalid' });
    }
  });

  it('leaves the saved limit alone when the text matches it, blank included', () => {
    expect(limitCommit('12000', 12000)).toEqual({ kind: 'unchanged' });
    expect(limitCommit(' 12000 ', 12000)).toEqual({ kind: 'unchanged' });
    expect(limitCommit('', null)).toEqual({ kind: 'unchanged' });
  });

  it('sends the new value, or null when the text is cleared', () => {
    expect(limitCommit('15000', 12000)).toEqual({ kind: 'send', value: 15000 });
    expect(limitCommit('7', null)).toEqual({ kind: 'send', value: 7 });
    expect(limitCommit('', 12000)).toEqual({ kind: 'send', value: null });
  });
});

describe('slugify', () => {
  it('lower-cases the name and joins runs of other characters with a dash', () => {
    expect(slugify('OpenCode Go', [])).toBe('opencode-go');
    expect(slugify('  My  Local_LLM!! ', [])).toBe('my-local-llm');
  });

  it('gives an empty id when the name has no letters or digits', () => {
    expect(slugify('', [])).toBe('');
    expect(slugify('!!!', [])).toBe('');
  });

  it('keeps the id unique among the taken ids', () => {
    expect(slugify('Acme', ['acme'])).toBe('acme-2');
    expect(slugify('Acme', ['acme', 'acme-2'])).toBe('acme-3');
    expect(slugify('Acme', ['other'])).toBe('acme');
  });

  it('is at most 40 characters, with the unique suffix inside that limit and no trailing dash', () => {
    const long = 'a'.repeat(60);
    const first = slugify(long, []);
    expect(first).toHaveLength(40);
    const second = slugify(long, [first]);
    expect(second).toHaveLength(40);
    expect(second.endsWith('-2')).toBe(true);
    expect(second).not.toMatch(/--/);
    expect(slugify('x'.repeat(39) + ' y', [])).toBe('x'.repeat(39));
  });
});

describe('preset and new drafts', () => {
  const preset: ProviderPreset = {
    key: 'openrouter',
    label: 'OpenRouter',
    provider: {
      id: 'openrouter',
      name: 'OpenRouter',
      chat_url: 'https://openrouter.ai/api/v1/chat/completions',
      models_url: 'https://openrouter.ai/api/v1/models',
      headers: [{ name: 'X-Title', value: '{session_id}', secret: false }],
      stream: true,
      max_tokens: null,
      models: [],
    },
    needs_key: true,
    note: '',
  };

  it('a preset becomes a new draft with its URLs and a unique id from the name', () => {
    const d = draftFromPreset(preset, ['openrouter']);
    expect(d.isNew).toBe(true);
    expect(d.id).toBe('openrouter-2');
    expect(d.name).toBe('OpenRouter');
    expect(d.models_url).toBe('https://openrouter.ai/api/v1/models');
    expect(d.headers).toHaveLength(1);
    expect(d.headers[0].value).toBe('{session_id}');
    expect(d.apiKey).toBe('');
    expect(d.storedKey).toBe(false);
  });

  it('a blank new provider has no id until a name is typed', () => {
    const d = draftNew();
    expect(d.id).toBe('');
    expect(withName(d, 'Local Box', []).id).toBe('local-box');
  });

  it('a new provider id follows the name until the id is typed by hand', () => {
    let d = draftNew();
    d = withName(d, 'Local', []);
    expect(d.id).toBe('local');
    d = withName(d, 'Local Box', ['local']);
    expect(d.id).toBe('local-box');
    d = withId(d, 'mybox');
    d = withName(d, 'Something else', []);
    expect(d.id).toBe('mybox');
    expect(d.name).toBe('Something else');
  });

  it('a saved provider keeps its id when its name changes', () => {
    const d = withName(saved(), 'Renamed', []);
    expect(d.id).toBe('acme');
    expect(d.name).toBe('Renamed');
  });

  it('an untouched new draft, blank or from a preset, is clean against the draft it started from', () => {
    const fromPreset = draftFromPreset(preset, ['openrouter']);
    expect(isDirty(fromPreset, fromPreset)).toBe(false);
    const blank = draftNew();
    expect(isDirty(blank, blank)).toBe(false);
  });

  it('any edit to an untouched new draft makes it dirty: name, URL, typed key, header, model', () => {
    const start = draftFromPreset(preset, ['openrouter']);
    expect(isDirty(withName(start, 'OpenRouter Two', ['openrouter']), start)).toBe(true);
    expect(isDirty({ ...start, chat_url: 'https://openrouter.ai/api/v2/chat/completions' }, start)).toBe(true);
    expect(isDirty({ ...start, apiKey: 'sk-live' }, start)).toBe(true);
    expect(isDirty({ ...start, headers: [...start.headers, newHeaderDraft()] }, start)).toBe(true);
    expect(isDirty({ ...start, models: [{ id: 'gpt', source: 'custom', context_limit: '', provider_context: null }] }, start)).toBe(
      true,
    );
  });

  it('typing a name and typing it back is clean again', () => {
    const start = draftFromPreset(preset, ['openrouter']);
    const typed = withName(start, 'Other', ['openrouter']);
    expect(isDirty(typed, start)).toBe(true);
    expect(isDirty(withName(typed, 'OpenRouter', ['openrouter']), start)).toBe(false);
  });
});

describe('draftFromProvider and the save round trip', () => {
  it('an untouched draft sends no secrets: api_key null, no header secret', () => {
    const input = buildSaveInput(saved());
    expect(input.api_key).toBeNull();
    expect(input.header_secrets).toEqual({});
  });

  it('the stored indicators come from the presence flags, and the draft holds no secret value', () => {
    const d = saved();
    expect(d.storedKey).toBe(true);
    expect(d.storedHeaders).toEqual(['X-Key']);
    expect(d.apiKey).toBe('');
    expect(d.headers.find((h) => h.name === 'X-Key')?.value).toBe('');
  });

  it('an untouched round trip gives back the saved provider, with secret header values as ""', () => {
    const input = buildSaveInput(saved());
    expect(input.provider).toEqual(SAVED);
  });

  it('the models list is always sent whole, with source, provider context and overrides kept', () => {
    const d = saved();
    d.models[0].context_limit = ' 4000 ';
    const input = buildSaveInput(d);
    expect(input.provider.models).toEqual([
      { id: 'big/one', context_limit: 4000, provider_context: 128000, source: 'fetched' },
      { id: 'mini', context_limit: null, provider_context: null, source: 'custom' },
    ]);
  });
});

describe('buildSaveInput: API key', () => {
  it('a typed key is sent trimmed', () => {
    const d = { ...saved(), apiKey: '  sk-live-123  ' };
    expect(buildSaveInput(d).api_key).toBe('sk-live-123');
  });

  it('a key of spaces only is not a removal: it is treated as untouched', () => {
    const d = { ...saved(), apiKey: '   ' };
    expect(buildSaveInput(d).api_key).toBeNull();
  });

  it('a requested removal sends ""', () => {
    const d = { ...saved(), clearKey: true };
    expect(buildSaveInput(d).api_key).toBe('');
  });

  it('a typed key wins over a removal request', () => {
    const d = { ...saved(), clearKey: true, apiKey: 'sk-new' };
    expect(buildSaveInput(d).api_key).toBe('sk-new');
  });
});

describe('buildSaveInput: headers and secret headers', () => {
  it('a secret header is sent with value "", and only a typed value goes to header_secrets', () => {
    const d = saved();
    const secret = d.headers.find((h) => h.name === 'X-Key')!;
    secret.value = '  tok-9  ';
    const input = buildSaveInput(d);
    expect(input.header_secrets).toEqual({ 'X-Key': 'tok-9' });
    const sent = input.provider.headers.find((h) => h.name === 'X-Key')!;
    expect(sent).toEqual({ name: 'X-Key', value: '', secret: true });
  });

  it('a secret header with nothing typed is absent from header_secrets, which keeps the stored value', () => {
    const input = buildSaveInput(saved());
    expect(input.header_secrets).toEqual({});
  });

  it('a stored secret header asked to clear sends "" in header_secrets', () => {
    const d = saved();
    d.headers.find((h) => h.name === 'X-Key')!.clearStored = true;
    expect(buildSaveInput(d).header_secrets).toEqual({ 'X-Key': '' });
  });

  it('a typed value wins over a clear request for the same header', () => {
    const d = saved();
    const h = d.headers.find((x) => x.name === 'X-Key')!;
    h.clearStored = true;
    h.value = 'fresh';
    expect(buildSaveInput(d).header_secrets).toEqual({ 'X-Key': 'fresh' });
  });

  it('a plain header value is sent as typed, trimmed, and never goes to header_secrets', () => {
    const d = saved();
    d.headers.find((h) => h.name === 'X-Team')!.value = ' green ';
    d.headers.find((h) => h.name === 'X-Team')!.clearStored = true;
    const input = buildSaveInput(d);
    expect(input.provider.headers[0]).toEqual({ name: 'X-Team', value: 'green', secret: false });
    expect(input.header_secrets).toEqual({});
  });

  it('a header that is not secret any more sends no secret entry, and its stored value is dropped by the backend', () => {
    const d = saved();
    const h = d.headers.find((x) => x.name === 'X-Key')!;
    h.secret = false;
    h.value = 'plain-now';
    const input = buildSaveInput(d);
    expect(input.header_secrets).toEqual({});
    expect(input.provider.headers.find((x) => x.name === 'X-Key')).toEqual({ name: 'X-Key', value: 'plain-now', secret: false });
  });
});

describe('buildSaveInput: numbers, URLs and blanks', () => {
  it('blank token limits and a blank models URL are sent as null', () => {
    const d = { ...saved(), max_tokens: '  ', models_url: ' ' };
    const provider = buildSaveInput(d).provider;
    expect(provider.max_tokens).toBeNull();
    expect(provider.models_url).toBeNull();
  });

  it('numbers are parsed from text, and the URLs and name are trimmed', () => {
    const d = { ...saved(), max_tokens: '2048', chat_url: ' https://x.test/v1/chat/completions ', name: ' Acme ' };
    const provider = buildSaveInput(d).provider;
    expect(provider.max_tokens).toBe(2048);
    expect(provider.chat_url).toBe('https://x.test/v1/chat/completions');
    expect(provider.name).toBe('Acme');
  });
});

describe('isDirty', () => {
  it('with no baseline (null), a draft counts as unsaved', () => {
    expect(isDirty(draftNew(), null)).toBe(true);
  });

  it('an untouched saved provider is clean', () => {
    expect(isDirty(saved(), saved())).toBe(false);
  });

  it('typing a key, clearing a key, or changing a field is a change', () => {
    expect(isDirty({ ...saved(), apiKey: 'x' }, saved())).toBe(true);
    expect(isDirty({ ...saved(), clearKey: true }, saved())).toBe(true);
    expect(isDirty({ ...saved(), stream: false }, saved())).toBe(true);
    const d = saved();
    d.models.pop();
    expect(isDirty(d, saved())).toBe(true);
  });

  it('opening the key input or changing which secrets are stored does not make it dirty by itself', () => {
    expect(isDirty({ ...saved(), keyEditing: true }, saved())).toBe(false);
  });
});

describe('validateDraft', () => {
  const taken = ['other'];

  it('a complete draft has no problems', () => {
    const p = validateDraft(saved(), []);
    expect(hasProblems(p)).toBe(false);
  });

  it('an id must match the slug rule, and must not be taken by another provider', () => {
    expect(validateDraft(withId(draftNew(), 'Bad Id'), taken).id).toMatch(/Use 1 to 40 characters/);
    expect(validateDraft(withId(draftNew(), '-lead'), taken).id).toMatch(/Use 1 to 40/);
    expect(validateDraft(withId(draftNew(), 'other'), taken).id).toBe('The id "other" is already used by another provider.');
    expect(validateDraft(withId(draftNew(), 'fine_one-2'), taken).id).toBeUndefined();
  });

  it('a name is required, at most 80 characters, and without control characters', () => {
    expect(validateDraft({ ...saved(), name: '   ' }, []).name).toBe('Name is required.');
    expect(validateDraft({ ...saved(), name: 'n'.repeat(81) }, []).name).toBe('Name must be at most 80 characters.');
    expect(validateDraft({ ...saved(), name: 'a\u0007b' }, []).name).toBe('Name must not contain control characters.');
  });

  it('the chat URL must be https, or plain http to a local address', () => {
    expect(validateDraft({ ...saved(), chat_url: 'http://api.acme.test/x' }, []).chat_url).toMatch(/plain http/);
    expect(validateDraft({ ...saved(), chat_url: 'http://localhost:8080/v1/chat/completions' }, []).chat_url).toBeUndefined();
    expect(validateDraft({ ...saved(), chat_url: 'ftp://x.test/y' }, []).chat_url).toMatch(/http or https/);
    expect(validateDraft({ ...saved(), chat_url: 'not a url' }, []).chat_url).toMatch(/spaces or control/);
    expect(validateDraft({ ...saved(), chat_url: 'https://' }, []).chat_url).toMatch(/not a valid URL/);
    expect(validateDraft({ ...saved(), chat_url: '' }, []).chat_url).toBe('Chat URL is required.');
  });

  it('the models URL is optional, and checked when given', () => {
    expect(validateDraft({ ...saved(), models_url: '' }, []).models_url).toBeUndefined();
    expect(validateDraft({ ...saved(), models_url: 'https://u:p@x.test/models' }, []).models_url).toMatch(/user name/);
  });

  it('max output tokens must be blank or a positive whole number', () => {
    expect(validateDraft({ ...saved(), max_tokens: '0' }, []).max_tokens).toMatch(/whole number/);
    expect(validateDraft({ ...saved(), max_tokens: 'lots' }, []).max_tokens).toMatch(/whole number/);
    expect(validateDraft({ ...saved(), max_tokens: '' }, []).max_tokens).toBeUndefined();
  });

  it('header names are required, valid tokens and listed once, ignoring case', () => {
    const d = saved();
    d.headers.push({ key: 'n1', name: '', value: '', secret: false, clearStored: false });
    d.headers.push({ key: 'n2', name: 'Bad Name', value: '', secret: false, clearStored: false });
    d.headers.push({ key: 'n3', name: 'x-team', value: 'dup', secret: false, clearStored: false });
    const p = validateDraft(d, []);
    expect(p.headers.n1).toBe('Header name is required.');
    expect(p.headers.n2).toMatch(/not a valid HTTP header name/);
    expect(p.headers.n3).toBe('Header "x-team" is listed more than once.');
  });

  it('Authorization must be a secret header', () => {
    const d = saved();
    d.headers.push({ key: 'auth', name: 'Authorization', value: 'Bearer x', secret: false, clearStored: false });
    expect(validateDraft(d, []).headers.auth).toMatch(/must be a secret header/);
    d.headers[2].secret = true;
    expect(validateDraft(d, []).headers.auth).toBeUndefined();
  });

  it('a plain header value is limited to 500 characters and no control characters; a secret value has no length rule', () => {
    const d = saved();
    d.headers[0].value = 'v'.repeat(501);
    expect(validateDraft(d, []).headers[d.headers[0].key]).toMatch(/at most 500 characters/);
    d.headers[0].value = 'a\nb';
    expect(validateDraft(d, []).headers[d.headers[0].key]).toMatch(/control characters/);
    const e = saved();
    e.headers[1].value = 'z'.repeat(600);
    expect(validateDraft(e, []).headers[e.headers[1].key]).toBeUndefined();
  });

  it('model ids are listed once, and a context limit must be blank or a positive whole number', () => {
    const d = saved();
    d.models.push({ id: 'mini', source: 'custom', context_limit: '', provider_context: null });
    d.models[0].context_limit = '0';
    const p = validateDraft(d, []);
    expect(p.models.mini).toBe('Model "mini" is listed more than once.');
    expect(p.models['big/one']).toMatch(/whole number of at least 1/);
  });

  it('hasProblems reports any entry', () => {
    expect(hasProblems({ headers: {}, models: {} })).toBe(false);
    expect(hasProblems({ headers: {}, models: { m: 'x' } })).toBe(true);
    expect(hasProblems({ headers: { h: 'x' }, models: {} })).toBe(true);
  });
});

describe('URL helpers', () => {
  it('urlProblem is null for a good https address and a local http one', () => {
    expect(urlProblem('https://api.test/v1', 'Chat URL')).toBeNull();
    expect(urlProblem('http://127.0.0.1:9/v1', 'Chat URL')).toBeNull();
    expect(urlProblem('http://[::1]:9/v1', 'Chat URL')).toBeNull();
  });

  it('urlProblem rejects a fragment', () => {
    expect(urlProblem('https://api.test/v1#x', 'Chat URL')).toMatch(/fragment/);
  });

  it('isLocalHttp is true only for plain http to a local address', () => {
    expect(isLocalHttp('http://localhost:8080/v1/chat/completions')).toBe(true);
    expect(isLocalHttp('https://localhost:8443/v1')).toBe(false);
    expect(isLocalHttp('http://api.test/v1')).toBe(false);
    expect(isLocalHttp('nonsense')).toBe(false);
  });

  it('modelsUrlFor uses the explicit URL, else derives it from a chat completions URL, else null', () => {
    expect(modelsUrlFor({ models_url: ' https://m.test/list ', chat_url: 'https://x.test/v1/chat/completions' })).toBe('https://m.test/list');
    expect(modelsUrlFor({ models_url: '', chat_url: 'https://x.test/v1/chat/completions' })).toBe('https://x.test/v1/models');
    expect(modelsUrlFor({ models_url: '', chat_url: 'https://x.test/generate' })).toBeNull();
  });

  it('chatHost shows the host, and the raw text when the URL does not parse', () => {
    expect(chatHost('https://api.acme.test:8443/v1/chat/completions')).toBe('api.acme.test:8443');
    expect(chatHost('not a url')).toBe('not a url');
  });
});

describe('fetchSummaryText', () => {
  it('says what the fetch changed, in one line', () => {
    expect(fetchSummaryText({ added: 3, updated: 1, removed: 2, total: 12 })).toBe(
      '+3 new, 1 updated, 2 removed. 12 models in the list.',
    );
  });
});

describe('modelOptions and modelRefKey', () => {
  const settings: Settings = {
    version: 1,
    providers: [
      { ...SAVED },
      { ...SAVED, id: 'local', name: 'Local box', models: [{ id: 'llama', context_limit: null, provider_context: null, source: 'custom' }] },
      { ...SAVED, id: 'empty', name: 'Empty', models: [] },
    ],
    default_model: null,
    global_context_limit: null,
  };

  it('lists every model of every provider in order, labelled "Provider · model"', () => {
    const options = modelOptions(settings);
    expect(options.map((o) => o.label)).toEqual(['Acme · big/one', 'Acme · mini', 'Local box · llama']);
  });

  it('keys are unique even when a model id contains a slash', () => {
    const keys = modelOptions(settings).map((o) => o.key);
    expect(new Set(keys).size).toBe(keys.length);
    expect(modelRefKey({ provider_id: 'acme', model_id: 'big/one' })).not.toBe(modelRefKey({ provider_id: 'acme/big', model_id: 'one' }));
  });
});

import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { ProviderEditor } from './ProviderEditor';
import type { ProviderEditorProps, TestState } from './ProviderEditor';
import { draftFromPreset, draftFromProvider, draftNew } from '../lib/settingsForm';
import type { DraftProblems } from '../lib/settingsForm';
import type { Provider } from '../lib/types';

const PROVIDER: Provider = {
  id: 'acme',
  name: 'Acme',
  chat_url: 'https://api.acme.test/v1/chat/completions',
  models_url: null,
  headers: [
    { name: 'X-Team', value: 'blue', secret: false },
    { name: 'X-Key', value: '', secret: true },
  ],
  stream: true,
  max_tokens: null,
  models: [
    { id: 'big/one', context_limit: null, provider_context: 128000, source: 'fetched' },
    { id: 'mini', context_limit: 4000, provider_context: null, source: 'custom' },
  ],
};

/** A saved provider with its key and one secret header stored. */
const SAVED = draftFromProvider(PROVIDER, { api_key: true, headers: ['X-Key'] });

const NO_PROBLEMS: DraftProblems = { headers: {}, models: {} };
const NOOP = () => undefined;

/** Props for the editor. The draft is clean unless a test gives a changed draft; `saved` is then the clean one. */
function props(over: Partial<ProviderEditorProps> = {}): ProviderEditorProps {
  const draft = over.draft ?? SAVED;
  const saved = 'saved' in over ? (over.saved ?? null) : SAVED;
  return {
    draft,
    saved,
    problems: NO_PROBLEMS,
    takenIds: [],
    locked: false,
    saving: false,
    saveError: null,
    defaultModelId: null,
    fetching: false,
    fetchNote: null,
    fetchError: null,
    tests: {},
    askDelete: false,
    askDiscard: false,
    onChange: NOOP,
    onSave: NOOP,
    onAskDelete: NOOP,
    onDelete: NOOP,
    onAskDiscard: NOOP,
    onCancel: NOOP,
    onFetch: NOOP,
    onTest: NOOP,
    ...over,
  };
}

const html = (over: Partial<ProviderEditorProps> = {}) => renderToStaticMarkup(<ProviderEditor {...props(over)} />);

function escapeRe(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&');
}

/** The opening tag of the button with this text, or '' when there is none. */
function buttonText(out: string, text: string): string {
  return out.match(new RegExp(`<button[^>]*>${escapeRe(text)}</button>`))?.[0] ?? '';
}

/** The opening tag of the button with this aria-label, or ''. */
function buttonLabelled(out: string, label: string): string {
  return out.match(new RegExp(`<button[^>]*aria-label="${escapeRe(label)}"[^>]*>`))?.[0] ?? '';
}

/** The tag of the input with this aria-label, or ''. */
function inputLabelled(out: string, label: string): string {
  return out.match(new RegExp(`<input[^>]*aria-label="${escapeRe(label)}"[^>]*>`))?.[0] ?? '';
}

describe('ProviderEditor: the stored API key', () => {
  it('a stored key shows the stored tag with Replace and Remove, and no key input', () => {
    const out = html();
    expect(out).toContain('>stored</span>');
    expect(buttonText(out, 'Replace')).not.toBe('');
    expect(buttonText(out, 'Remove')).not.toBe('');
    // The only password field is the secret header's.
    expect((out.match(/type="password"/g) ?? []).length).toBe(1);
  });

  it('Replace opens an empty password input with Keep stored, and never fills the stored key in', () => {
    const out = html({ draft: { ...SAVED, keyEditing: true } });
    const keyInput = out.match(/<input[^>]*type="password"[^>]*aria-labelledby="[^"]*-key-label"[^>]*>/);
    expect(keyInput).not.toBeNull();
    expect(keyInput?.[0]).toContain('value=""');
    expect(buttonText(out, 'Keep stored')).not.toBe('');
  });

  it('a key asked to be removed says so, with Undo, and shows no key input', () => {
    const out = html({ draft: { ...SAVED, clearKey: true } });
    expect(out).toContain('The stored key is removed when you save.');
    expect(buttonText(out, 'Undo')).not.toBe('');
    expect((out.match(/type="password"/g) ?? []).length).toBe(1);
  });

  it('a provider with no stored key shows the key input directly, with a warning when remote', () => {
    const d = draftFromProvider(PROVIDER, { api_key: false, headers: ['X-Key'] });
    const out = html({ draft: d, saved: d });
    expect(out).toContain('No key stored. Requests to this provider fail until one is saved.');
    expect(out).toContain('placeholder="Paste the API key"');
  });

  it('a keyless local URL marks the key as optional, with no warning', () => {
    const d = { ...draftNew(), name: 'Local', id: 'local', chat_url: 'http://localhost:8080/v1/chat/completions' };
    const out = html({ draft: d, saved: null });
    expect(out).toContain('API key (optional for a local server)');
    expect(out).not.toContain('No key stored.');
  });
});

describe('ProviderEditor: secret headers', () => {
  it('a secret header value is a password field with no value, marked stored, with a remove control', () => {
    const out = html();
    const field = inputLabelled(out, 'Value of secret header X-Key');
    expect(field).toContain('type="password"');
    expect(field).toContain('value=""');
    expect(field).toContain('placeholder="Stored. Type to replace"');
    expect(out).toContain('>stored</span>');
    expect(buttonText(out, 'Remove stored')).not.toBe('');
  });

  it('a plain header value is a plain text field with its value', () => {
    const field = inputLabelled(html(), 'Value of header X-Team');
    expect(field).toContain('type="text"');
    expect(field).toContain('value="blue"');
  });

  it('the header hint explains the session placeholder and where secrets are kept', () => {
    const out = html();
    expect(out).toContain('{session_id}');
    expect(out).toContain('x-opencode-session');
    expect(out).toContain('Secret values are kept in the store');
  });

  it('a header marked for removal says it is removed on save', () => {
    const d = { ...SAVED, headers: SAVED.headers.map((h) => (h.name === 'X-Key' ? { ...h, clearStored: true } : h)) };
    expect(html({ draft: d, saved: SAVED })).toContain('Removed when you save');
  });
});

describe('ProviderEditor: fetch and test, and their reasons', () => {
  it('fetch is disabled, with the reason, for a provider not saved yet', () => {
    const d = { ...SAVED, isNew: true, id: 'fresh' };
    const button = buttonText(html({ draft: d, saved: null }), 'Fetch models');
    expect(button).toContain('disabled=""');
    expect(button).toContain('title="Save the provider first.');
  });

  it('fetch is disabled while there are unsaved changes', () => {
    const button = buttonText(html({ draft: { ...SAVED, apiKey: 'sk-typed' } }), 'Fetch models');
    expect(button).toContain('disabled=""');
    expect(button).toContain('Save your changes first');
  });

  it('fetch is enabled for a clean saved provider with a list URL', () => {
    expect(buttonText(html(), 'Fetch models')).not.toContain('disabled');
  });

  it('fetch is disabled when no model list URL can be derived', () => {
    const d = { ...SAVED, chat_url: 'https://api.acme.test/v1/generate' };
    const button = buttonText(html({ draft: d, saved: d }), 'Fetch models');
    expect(button).toContain('disabled=""');
    expect(button).toContain('no model list URL');
  });

  it('the fetch summary is shown as text', () => {
    const out = html({ fetchNote: '+3 new, 1 updated, 2 removed. 12 models in the list.' });
    expect(out).toContain('+3 new, 1 updated, 2 removed. 12 models in the list.');
  });

  it('a model test is disabled, with the reason, while there are unsaved changes', () => {
    const out = html({ draft: { ...SAVED, stream: false } });
    expect(buttonLabelled(out, 'Test big/one')).toContain('disabled=""');
    expect(buttonLabelled(out, 'Test big/one')).toContain('Save your changes first');
  });

  it('a clean saved model can be tested, and the test note says it costs a few tokens', () => {
    const out = html();
    expect(buttonLabelled(out, 'Test mini')).not.toContain('disabled');
    expect(out).toContain('costs a few tokens');
  });

  it('a successful test shows ok, the millis and the reply as text', () => {
    const tests: Record<string, TestState> = { mini: { kind: 'done', result: { ok: true, message: 'Reply: ok', millis: 412 } } };
    const out = html({ tests });
    expect(out).toContain('ok · 412 ms · Reply: ok');
    expect(out).toContain('settings-test-ok');
  });

  it('a failed test shows failed, the millis and the message as text', () => {
    const tests: Record<string, TestState> = { mini: { kind: 'done', result: { ok: false, message: 'HTTP 401: bad key', millis: 900 } } };
    const out = html({ tests });
    expect(out).toContain('failed · 900 ms · HTTP 401: bad key');
    expect(out).toContain('settings-test-fail');
  });

  it('a test that could not run shows its error, and a running test says so', () => {
    expect(html({ tests: { mini: { kind: 'failed', message: 'no key' } } })).toContain('Test failed: no key');
    expect(html({ tests: { mini: { kind: 'running' } } })).toContain('Testing...');
  });

  it('a message from the provider is shown as text, not markup', () => {
    const out = html({ tests: { mini: { kind: 'done', result: { ok: false, message: '<img src=x onerror=1>', millis: 1 } } } });
    expect(out).not.toContain('<img src=x');
    expect(out).toContain('&lt;img src=x onerror=1&gt;');
  });
});

describe('ProviderEditor: validation and save', () => {
  it('each problem is shown next to its field, and Save stays disabled', () => {
    const d = {
      ...SAVED,
      headers: [...SAVED.headers, { key: 'bad', name: 'Authorization', value: 'x', secret: false, clearStored: false }],
    };
    const problems: DraftProblems = {
      headers: { bad: 'Authorization must be a secret header, so its value stays out of settings.json.' },
      models: { mini: 'Model "mini" is listed more than once.' },
      chat_url: 'Chat URL must use http or https.',
      max_tokens: 'Max output tokens must be a whole number of at least 1, or blank.',
    };
    const out = html({ draft: d, saved: SAVED, problems });
    expect(out).toContain('Chat URL must use http or https.');
    expect(out).toContain('Max output tokens must be a whole number');
    expect(out).toContain('Authorization must be a secret header');
    expect(out).toContain('Model &quot;mini&quot; is listed more than once.');
    expect(buttonText(out, 'Save provider')).toContain('disabled=""');
    expect(out).toContain('Fix the fields marked above to save.');
  });

  it('a clean provider cannot be saved', () => {
    expect(buttonText(html(), 'Save provider')).toContain('disabled=""');
  });

  it('a changed provider can be saved, and the unsaved marker shows', () => {
    const out = html({ draft: { ...SAVED, stream: false } });
    expect(buttonText(out, 'Save provider')).not.toContain('disabled');
    expect(out).toContain('Unsaved changes');
  });

  it('an untouched new provider has no unsaved marker and can be saved, and fetch still says save first', () => {
    const fresh = draftFromPreset(
      { key: 'custom', label: 'Custom', provider: { ...PROVIDER, id: 'custom', name: 'Custom' }, needs_key: false, note: '' },
      [],
    );
    const out = html({ draft: fresh, saved: fresh });
    expect(out).not.toContain('Unsaved changes');
    expect(buttonText(out, 'Save provider')).not.toContain('disabled');
    const fetch = buttonText(out, 'Fetch models');
    expect(fetch).toContain('disabled=""');
    expect(fetch).toContain('Save the provider first.');
  });

  it('a save error from the backend is shown verbatim, as an alert', () => {
    const out = html({
      draft: { ...SAVED, stream: false },
      saveError: 'provider "acme": the API key must not contain control characters',
    });
    expect(out).toContain('provider &quot;acme&quot;: the API key must not contain control characters');
    expect(out).toContain('role="alert"');
  });

  it('the id is read-only once saved, and editable for a new provider', () => {
    const saved = html();
    const savedId = saved.match(/<input[^>]*value="acme"[^>]*>/)?.[0] ?? '';
    expect(savedId).toMatch(/readOnly=""|readonly=""/);

    const fresh = draftFromPreset(
      {
        key: 'custom',
        label: 'Custom',
        provider: { ...PROVIDER, id: 'custom', name: 'Custom' },
        needs_key: false,
        note: '',
      },
      [],
    );
    const out = html({ draft: fresh, saved: null });
    const freshId = out.match(/<input[^>]*value="custom"[^>]*>/)?.[0] ?? '';
    expect(freshId).not.toBe('');
    expect(freshId).not.toMatch(/readOnly|readonly/);
    expect(out).toContain('Made from the name. Fixed after the first save.');
  });

  it('a pending discard asks before dropping the draft', () => {
    const out = html({ draft: { ...SAVED, stream: false }, askDiscard: true });
    expect(out).toContain('Discard unsaved changes?');
    expect(buttonText(out, 'Keep editing')).not.toBe('');
  });

  it('a pending delete asks first, and a new provider has no delete control', () => {
    const out = html({ askDelete: true });
    expect(out).toContain('Delete this provider and its stored keys?');
    expect(buttonText(out, 'Yes, delete')).not.toBe('');
    expect(html({ draft: draftNew(), saved: null })).not.toContain('Delete provider');
  });
});

describe('ProviderEditor: models', () => {
  it('the default model cannot be removed, and the reason is the title', () => {
    const out = html({ defaultModelId: 'big/one' });
    const remove = buttonLabelled(out, 'Remove model big/one');
    expect(remove).toContain('disabled=""');
    expect(remove).toContain('This is the default model.');
    expect(out).toContain('>default</span>');
    expect(buttonLabelled(out, 'Remove model mini')).not.toContain('disabled');
  });

  it('a model shows its source, the provider context and the override as text', () => {
    const out = html();
    expect(out).toContain('>fetched</span>');
    expect(out).toContain('128,000 context');
    expect(inputLabelled(out, 'Context limit for mini')).toContain('value="4000"');
    expect(inputLabelled(out, 'Context limit for big/one')).toContain('placeholder="none"');
  });

  it('a model id is shown as text, not markup', () => {
    const d = { ...SAVED, models: [{ id: '<b>x</b>', source: 'custom' as const, context_limit: '', provider_context: null }] };
    const out = html({ draft: d, saved: d });
    expect(out).toContain('&lt;b&gt;x&lt;/b&gt;');
    expect(out).not.toContain('<b>x</b>');
  });

  it('a model listed twice is reported next to it', () => {
    const out = html({ problems: { headers: {}, models: { mini: 'Model "mini" is listed more than once.' } } });
    expect(out).toContain('Model &quot;mini&quot; is listed more than once.');
  });

  it('with no models, the list says how to get one', () => {
    const d = { ...SAVED, models: [] };
    expect(html({ draft: d, saved: d })).toContain('No models yet. Fetch the list, or add one by id.');
  });
});

describe('ProviderEditor: locked and busy', () => {
  it('a locked editor disables its fields and its actions', () => {
    const out = html({ locked: true, draft: { ...SAVED, stream: false } });
    expect(inputLabelled(out, 'Value of header X-Team')).toContain('disabled=""');
    expect(buttonText(out, 'Save provider')).toContain('disabled=""');
    expect(buttonText(out, 'Delete provider')).toContain('disabled=""');
  });

  it('a saving editor shows Saving and disables its fields', () => {
    const out = html({ saving: true, draft: { ...SAVED, stream: false } });
    expect(out).toContain('Saving...');
    expect(inputLabelled(out, 'Value of header X-Team')).toContain('disabled=""');
  });
});

describe('ProviderEditor: names are text', () => {
  it('a provider name is shown as text, not markup', () => {
    const d = { ...SAVED, name: '<img src=x>' };
    const out = html({ draft: d, saved: d });
    expect(out).not.toContain('<img src=x>');
    expect(out).toContain('&lt;img src=x&gt;');
  });
});

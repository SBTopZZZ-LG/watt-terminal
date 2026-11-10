import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { SettingsBody, storeNote } from './SettingsDialog';
import type { LoadState, SettingsBodyProps } from './SettingsDialog';
import { modelRefKey } from '../lib/settingsForm';
import type { Provider, ProviderPreset, Settings, SettingsView } from '../lib/types';

const ACME: Provider = {
  id: 'acme',
  name: 'Acme',
  chat_url: 'https://api.acme.test/v1/chat/completions',
  models_url: null,
  headers: [{ name: 'X-Key', value: '', secret: true }],
  stream: true,
  max_tokens: null,
  models: [
    { id: 'big/one', context_limit: null, provider_context: 128000, source: 'fetched' },
    { id: 'mini', context_limit: 4000, provider_context: null, source: 'custom' },
  ],
};

const LOCAL: Provider = {
  id: 'local',
  name: 'Local box',
  chat_url: 'http://localhost:8080/v1/chat/completions',
  models_url: null,
  headers: [],
  stream: true,
  max_tokens: null,
  models: [],
};

const SETTINGS: Settings = {
  version: 1,
  providers: [ACME, LOCAL],
  default_model: { provider_id: 'acme', model_id: 'big/one' },
  global_context_limit: 8000,
};

function view(over: Partial<SettingsView> = {}): SettingsView {
  return {
    settings: SETTINGS,
    secrets: {
      acme: { api_key: true, headers: [] },
      local: { api_key: false, headers: [] },
    },
    store_kind: 'keychain',
    load_error: null,
    settings_state: { kind: 'ok', reason: null, last_good_available: false, path: '/config/settings.json' },
    ...over,
  };
}

const PRESET: ProviderPreset = {
  key: 'openrouter',
  label: 'OpenRouter',
  provider: { ...ACME, id: 'openrouter', name: 'OpenRouter', chat_url: 'https://openrouter.ai/api/v1/chat/completions' },
  needs_key: true,
  note: '',
};

const NOOP = () => undefined;

function props(over: Partial<SettingsBodyProps> = {}): SettingsBodyProps {
  const load: LoadState = over.load ?? { kind: 'ready', view: view() };
  return {
    load,
    locked: false,
    presets: [PRESET],
    defaults: { busy: false, error: null },
    limit: { text: '8000', error: null, busy: false },
    notice: null,
    editor: null,
    onRetry: NOOP,
    onCheckAgain: NOOP,
    onDefault: NOOP,
    onLimitText: NOOP,
    onLimitCommit: NOOP,
    onLimitClear: NOOP,
    onOpenProvider: NOOP,
    onAddProvider: NOOP,
    ...over,
  };
}

const html = (over: Partial<SettingsBodyProps> = {}) => renderToStaticMarkup(<SettingsBody {...props(over)} />);

/** The opening tag of the option with this value, or ''. */
function optionTag(out: string, value: string): string {
  const escaped = value.replace(/&/g, '&amp;').replace(/"/g, '&quot;');
  return out.match(new RegExp(`<option[^>]*value="${escaped.replace(/[[\]]/g, '\\$&')}"[^>]*>`))?.[0] ?? '';
}

describe('SettingsBody: load states', () => {
  it('loading shows a line and nothing else', () => {
    const out = html({ load: { kind: 'loading' } });
    expect(out).toContain('Loading settings...');
    expect(out).not.toContain('Default model');
  });

  it('a failed load shows the message with Retry', () => {
    const out = html({ load: { kind: 'error', message: 'store is gone' } });
    expect(out).toContain('role="alert"');
    expect(out).toContain('Could not load the AI settings: store is gone');
    expect(out).toContain('>Retry</button>');
  });
});

describe('SettingsBody: broken settings file', () => {
  const broken = view({ load_error: 'settings are invalid: not valid JSON', settings: { ...SETTINGS, providers: [] } });

  it('shows the banner with the reason, and says changes are disabled', () => {
    const out = html({ load: { kind: 'ready', view: broken }, locked: true });
    expect(out).toContain(
      'settings.json could not be read: settings are invalid: not valid JSON. Changes are disabled until it is fixed or removed.',
    );
    expect(out).toContain('>Check again</button>');
  });

  it('disables the default model, the context limit, and adding a provider', () => {
    const out = html({ load: { kind: 'ready', view: broken }, locked: true });
    expect(out).toMatch(/<select[^>]*id="[^"]*-default"[^>]*disabled=""/);
    expect(out).toMatch(/<input[^>]*id="[^"]*-limit"[^>]*disabled=""/);
    expect(out).toMatch(/<select[^>]*id="[^"]*-preset"[^>]*disabled=""/);
    expect(out).toMatch(/<button[^>]*disabled=""[^>]*>Add provider<\/button>/);
  });

  it('a readable file has no banner', () => {
    expect(html()).not.toContain('could not be read');
  });
});

describe('SettingsBody: store note', () => {
  it('keychain and file each have their sentence', () => {
    expect(storeNote('keychain')).toBe('API keys are stored in the macOS Keychain.');
    expect(storeNote('file')).toBe('API keys are stored in the file named by AGENT_TERM_SECRETS_FILE.');
    expect(html()).toContain('API keys are stored in the macOS Keychain.');
    expect(html({ load: { kind: 'ready', view: view({ store_kind: 'file' }) } })).toContain('AGENT_TERM_SECRETS_FILE');
  });
});

describe('SettingsBody: default model', () => {
  it('lists every model of every provider, labelled "Provider · model", with None first', () => {
    const out = html();
    expect(out).toContain('None (mock agent)');
    expect(out).toContain('Acme · big/one');
    expect(out).toContain('Acme · mini');
    expect(out.indexOf('None (mock agent)')).toBeLessThan(out.indexOf('Acme · big/one'));
  });

  it('the current default is the selected option, and only that one', () => {
    const out = html();
    const key = modelRefKey({ provider_id: 'acme', model_id: 'big/one' });
    expect(optionTag(out, key)).toContain('selected=""');
    expect(optionTag(out, modelRefKey({ provider_id: 'acme', model_id: 'mini' }))).not.toContain('selected');
    expect(optionTag(out, '')).not.toContain('selected');
  });

  it('with no models, it says how to get one, and the hint says what it is for', () => {
    const out = html({ load: { kind: 'ready', view: view({ settings: { ...SETTINGS, providers: [LOCAL], default_model: null } }) } });
    expect(out).toContain('No models yet. Add one to a provider below.');
    expect(html()).toContain('Used for new messages. A running conversation keeps its history.');
  });

  it('a failed change shows its error, as an alert', () => {
    const out = html({ defaults: { busy: false, error: 'the default model cannot be used' } });
    expect(out).toContain('role="alert"');
    expect(out).toContain('the default model cannot be used');
  });
});

describe('SettingsBody: global context limit', () => {
  it('shows the limit, and its hint', () => {
    const out = html();
    expect(out).toMatch(/<input[^>]*id="[^"]*-limit"[^>]*value="8000"/);
    expect(out).toContain('Caps how much conversation is sent. Per-model limits below can lower it further.');
  });

  it('with no limit, the input says so', () => {
    const out = html({ limit: { text: '', error: null, busy: false } });
    expect(out).toMatch(/<input[^>]*id="[^"]*-limit"[^>]*placeholder="No limit"/);
  });

  it('a bad entry shows its message as an alert', () => {
    const out = html({ limit: { text: '0', error: 'Enter a whole number of at least 1, or clear the field.', busy: false } });
    expect(out).toContain('Enter a whole number of at least 1, or clear the field.');
  });
});

describe('SettingsBody: providers', () => {
  it('an empty list says how to add one', () => {
    const out = html({ load: { kind: 'ready', view: view({ settings: { ...SETTINGS, providers: [] } }) } });
    expect(out).toContain('No providers yet. Add one to use an AI model.');
  });

  it('each row shows the name, the chat host, the key state and the model count', () => {
    const out = html();
    expect(out).toContain('Acme');
    expect(out).toContain('api.acme.test');
    expect(out).toContain('key stored');
    expect(out).toContain('2 models');
    expect(out).toContain('Local box');
    expect(out).toContain('no key');
    expect(out).toContain('0 models');
  });

  it('only the default model’s provider is marked default', () => {
    // The default marker is a tag in the row; the default-model select has no tag.
    const out = html();
    expect(out.split('settings-tag-accent">default<').length).toBe(2);
    expect(out.indexOf('>default<')).toBeGreaterThan(out.indexOf('Acme'));
    expect(out.indexOf('>default<')).toBeLessThan(out.indexOf('Local box'));
  });

  it('the open provider has its row expanded, and its editor sits under that row', () => {
    const out = html({
      editor: { forId: 'acme', node: <p>EDITOR-FOR-ACME</p> },
    });
    expect(out).toContain('aria-expanded="true"');
    const editorAt = out.indexOf('EDITOR-FOR-ACME');
    expect(editorAt).toBeGreaterThan(out.indexOf('api.acme.test'));
    expect(editorAt).toBeLessThan(out.indexOf('Local box'));
  });

  it('a new provider is shown at the end of the list, after the add control', () => {
    const out = html({ editor: { forId: null, node: <p>EDITOR-NEW</p> } });
    const at = out.indexOf('EDITOR-NEW');
    expect(at).toBeGreaterThan(out.indexOf('Add provider'));
  });

  it('a notice about the open editor is shown as a status line', () => {
    const out = html({ notice: 'Save or cancel the open provider first.' });
    expect(out).toContain('role="status"');
    expect(out).toContain('Save or cancel the open provider first.');
  });

  it('the add control offers the presets and a blank provider', () => {
    const out = html();
    expect(out).toContain('Blank provider');
    expect(out).toContain('OpenRouter');
    expect(out).toContain('>Add provider</button>');
  });

  it('a provider name is shown as text, not markup', () => {
    const named = { ...ACME, name: '<img src=x>' };
    const out = html({ load: { kind: 'ready', view: view({ settings: { ...SETTINGS, providers: [named] } }) } });
    expect(out).not.toContain('<img src=x>');
    expect(out).toContain('&lt;img src=x&gt;');
  });
});

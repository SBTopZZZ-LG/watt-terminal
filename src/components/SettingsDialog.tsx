import { useEffect, useId, useRef, useState } from 'react';
import type { KeyboardEvent, ReactNode } from 'react';
import { api, errorText } from '../lib/api';
import {
  buildSaveInput,
  chatHost,
  draftFromPreset,
  draftFromProvider,
  draftNew,
  fetchSummaryText,
  isDirty,
  modelOptions,
  modelRefKey,
  parsePositiveInt,
  validateDraft,
} from '../lib/settingsForm';
import type { ProviderDraft } from '../lib/settingsForm';
import type { ProviderPreset, SettingsView } from '../lib/types';
import { ProviderEditor } from './ProviderEditor';
import type { TestState } from './ProviderEditor';

/** The settings as loaded. A failed load shows the error with Retry. */
export type LoadState = { kind: 'loading' } | { kind: 'error'; message: string } | { kind: 'ready'; view: SettingsView };

/** The editor's state: its draft and what is saved, and the in-flight and last results of its actions. */
export interface EditorState {
  /** The saved provider's id, or null for a provider not saved yet (shown at the end of the list). */
  forId: string | null;
  draft: ProviderDraft;
  saved: ProviderDraft | null;
  saving: boolean;
  saveError: string | null;
  fetching: boolean;
  fetchNote: string | null;
  fetchError: string | null;
  tests: Record<string, TestState>;
  askDelete: boolean;
  askDiscard: boolean;
}

/** The select value for "None" (default model) and "Blank provider" (add provider). Preset keys are never empty. */
const BLANK_PRESET = '';
const NO_KEY_TEXT = 'API keys are stored in the file named by AGENT_TERM_SECRETS_FILE.';
const KEYCHAIN_TEXT = 'API keys are stored in the macOS Keychain.';
const LIMIT_ERROR = 'Enter a whole number of at least 1, or clear the field.';

/** Where the secrets live, as a sentence for the top of the dialog. */
export function storeNote(kind: SettingsView['store_kind']): string {
  return kind === 'keychain' ? KEYCHAIN_TEXT : NO_KEY_TEXT;
}

function limitText(view: SettingsView): string {
  const limit = view.settings.global_context_limit;
  return limit === null ? '' : String(limit);
}

function newEditor(forId: string | null, draft: ProviderDraft, saved: ProviderDraft | null): EditorState {
  return {
    forId,
    draft,
    saved,
    saving: false,
    saveError: null,
    fetching: false,
    fetchNote: null,
    fetchError: null,
    tests: {},
    askDelete: false,
    askDiscard: false,
  };
}

/** Moves focus back to the element that had it when the dialog opened, or to the terminal when that is gone. */
function restoreFocus(opener: Element | null) {
  if (opener instanceof HTMLElement && opener.isConnected && opener !== document.body) {
    opener.focus();
    return;
  }
  document.querySelector<HTMLElement>('.xterm-helper-textarea')?.focus();
}

const FOCUSABLE = 'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export interface SettingsBodyProps {
  load: LoadState;
  /** The settings file is broken: every change is disabled. */
  locked: boolean;
  presets: ProviderPreset[];
  defaults: { busy: boolean; error: string | null };
  limit: { text: string; error: string | null; busy: boolean };
  /** A message about the providers, such as "Save or cancel the open provider first." */
  notice: string | null;
  /** The open editor, drawn under its row, or at the end of the list when `forId` is null. */
  editor: { forId: string | null; node: ReactNode } | null;
  onRetry(): void;
  onCheckAgain(): void;
  onDefault(key: string): void;
  onLimitText(text: string): void;
  onLimitCommit(): void;
  onLimitClear(): void;
  onOpenProvider(id: string): void;
  /** A preset key, or the blank provider (an empty key). */
  onAddProvider(key: string): void;
}

/**
 * The settings' body: the default model, the global context limit and the providers, or the loading and error states. It
 * holds nothing but the choice in the "add provider" select, so it renders the same in a test as in the dialog.
 */
export function SettingsBody(p: SettingsBodyProps) {
  const uid = useId();
  const [preset, setPreset] = useState(BLANK_PRESET);

  if (p.load.kind === 'loading') {
    return <div className="settings-state">Loading settings...</div>;
  }
  if (p.load.kind === 'error') {
    return (
      <div className="settings-state settings-state-error" role="alert">
        {`Could not load the AI settings: ${p.load.message}`}
        <button type="button" onClick={p.onRetry}>
          Retry
        </button>
      </div>
    );
  }

  const { view } = p.load;
  const { settings } = view;
  const options = modelOptions(settings);
  const defaultKey = settings.default_model ? modelRefKey(settings.default_model) : BLANK_PRESET;
  const defaultProviderId = settings.default_model?.provider_id ?? null;
  const off = p.locked;

  return (
    <>
      {view.load_error !== null && (
        <div className="settings-banner" role="alert">
          <span>{`settings.json could not be read: ${view.load_error}. Changes are disabled until it is fixed or removed.`}</span>
          <button type="button" onClick={p.onCheckAgain}>
            Check again
          </button>
        </div>
      )}
      <p className="settings-note">{storeNote(view.store_kind)}</p>

      <section className="settings-section" aria-labelledby={`${uid}-default-title`}>
        <h3 className="settings-section-title" id={`${uid}-default-title`}>
          Default model
        </h3>
        <div className="settings-inline">
          <label htmlFor={`${uid}-default`}>Model</label>
          <select
            id={`${uid}-default`}
            value={defaultKey}
            disabled={off || p.defaults.busy}
            onChange={(e) => p.onDefault(e.target.value)}
          >
            <option value={BLANK_PRESET}>None (mock agent)</option>
            {options.map((o) => (
              <option key={o.key} value={o.key}>
                {o.label}
              </option>
            ))}
          </select>
          {p.defaults.busy && <span className="settings-hint">Saving...</span>}
        </div>
        <p className="settings-hint">
          {options.length === 0
            ? 'No models yet. Add one to a provider below. Used for new messages.'
            : 'Used for new messages. A running conversation keeps its history.'}
        </p>
        {p.defaults.error !== null && (
          <p className="settings-error" role="alert">
            {p.defaults.error}
          </p>
        )}
      </section>

      <section className="settings-section" aria-labelledby={`${uid}-limit-title`}>
        <h3 className="settings-section-title" id={`${uid}-limit-title`}>
          Global context limit
        </h3>
        <div className="settings-inline">
          <label htmlFor={`${uid}-limit`}>Tokens</label>
          <input
            id={`${uid}-limit`}
            type="number"
            min={1}
            step={1}
            placeholder="No limit"
            value={p.limit.text}
            disabled={off || p.limit.busy}
            aria-invalid={p.limit.error !== null || undefined}
            onChange={(e) => p.onLimitText(e.target.value)}
            onBlur={p.onLimitCommit}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                p.onLimitCommit();
              }
            }}
          />
          <button
            type="button"
            disabled={off || p.limit.busy}
            // Keeps the focus in the input, so the blur does not apply the typed text just before the clear.
            onMouseDown={(e) => e.preventDefault()}
            onClick={p.onLimitClear}
          >
            Clear
          </button>
        </div>
        <p className="settings-hint">Caps how much conversation is sent. Per-model limits below can lower it further.</p>
        {p.limit.error !== null && (
          <p className="settings-error" role="alert">
            {p.limit.error}
          </p>
        )}
      </section>

      <section className="settings-section" aria-labelledby={`${uid}-providers-title`}>
        <h3 className="settings-section-title" id={`${uid}-providers-title`}>
          Providers
        </h3>
        {settings.providers.length === 0 && <p className="settings-hint">No providers yet. Add one to use an AI model.</p>}
        <ul className="settings-providers">
          {settings.providers.map((provider) => {
            const present = view.secrets[provider.id];
            const open = p.editor !== null && p.editor.forId === provider.id;
            const isDefault = defaultProviderId === provider.id;
            return (
              <li key={provider.id} className="settings-provider">
                <button
                  type="button"
                  className={`settings-row${open ? ' is-open' : ''}`}
                  aria-expanded={open}
                  onClick={() => p.onOpenProvider(provider.id)}
                >
                  <span className="settings-row-name">{provider.name}</span>
                  <span className="settings-dim">{chatHost(provider.chat_url)}</span>
                  <span className={present?.api_key ? 'settings-tag settings-tag-ok' : 'settings-tag settings-tag-muted'}>
                    {present?.api_key ? 'key stored' : 'no key'}
                  </span>
                  <span className="settings-dim">{`${provider.models.length} ${provider.models.length === 1 ? 'model' : 'models'}`}</span>
                  {isDefault && <span className="settings-tag settings-tag-accent">default</span>}
                </button>
                {open && p.editor?.node}
              </li>
            );
          })}
        </ul>

        {p.notice !== null && (
          <p className="settings-notice" role="status">
            {p.notice}
          </p>
        )}

        <div className="settings-inline">
          <label htmlFor={`${uid}-preset`}>Preset</label>
          <select id={`${uid}-preset`} value={preset} disabled={off} onChange={(e) => setPreset(e.target.value)}>
            <option value={BLANK_PRESET}>Blank provider</option>
            {p.presets.map((pr) => (
              <option key={pr.key} value={pr.key}>
                {pr.label}
              </option>
            ))}
          </select>
          <button type="button" disabled={off} onClick={() => p.onAddProvider(preset)}>
            Add provider
          </button>
        </div>

        {p.editor !== null && p.editor.forId === null && <div className="settings-editor-slot">{p.editor.node}</div>}
      </section>
    </>
  );
}

export interface SettingsDialogProps {
  onClose(): void;
  /** A change was saved: the caller refreshes the status (the chip shows the default model). */
  onChanged(): void;
}

/**
 * The AI settings dialog: a modal over the window. It loads the settings, and holds the state of the default model, the
 * context limit and the provider editor. Focus moves in on open, stays in while open, and goes back to where it was on
 * close. Esc closes; a click on the backdrop closes. Unsaved provider changes are asked about before they are dropped.
 */
export function SettingsDialog({ onClose, onChanged }: SettingsDialogProps) {
  const titleId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const backdropPressed = useRef(false);
  const [opener] = useState<Element | null>(() => document.activeElement);
  const [attempt, setAttempt] = useState(0);
  const [load, setLoad] = useState<LoadState>({ kind: 'loading' });
  const [presets, setPresets] = useState<ProviderPreset[]>([]);
  const [defaults, setDefaults] = useState<{ busy: boolean; error: string | null }>({ busy: false, error: null });
  const [limit, setLimit] = useState<{ text: string; error: string | null; busy: boolean }>({ text: '', error: null, busy: false });
  const [notice, setNotice] = useState<string | null>(null);
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [askClose, setAskClose] = useState(false);

  const view = load.kind === 'ready' ? load.view : null;
  const dirty = editor !== null && isDirty(editor.draft, editor.saved);

  // Load the settings each time the dialog opens or Retry is pressed. The presets are static, so a failure only hides them.
  useEffect(() => {
    let live = true;
    api.getSettings().then(
      (v) => {
        if (live) {
          setLoad({ kind: 'ready', view: v });
          setLimit((l) => ({ ...l, text: limitText(v) }));
        }
      },
      (e: unknown) => {
        if (live) {
          setLoad({ kind: 'error', message: errorText(e) });
        }
      },
    );
    return () => {
      live = false;
    };
  }, [attempt]);

  useEffect(() => {
    let live = true;
    api.providerPresets().then(
      (list) => {
        if (live) {
          setPresets(list);
        }
      },
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, []);

  // Focus moves into the dialog on open. Focus that leaves it (by a shortcut or the terminal taking it) comes back.
  useEffect(() => {
    dialogRef.current?.focus();
    const pull = (e: FocusEvent) => {
      const dialog = dialogRef.current;
      if (dialog && e.target instanceof Node && !dialog.contains(e.target)) {
        dialog.focus();
      }
    };
    document.addEventListener('focusin', pull);
    return () => document.removeEventListener('focusin', pull);
  }, []);

  // Focus goes back to the opener when the dialog is unmounted.
  useEffect(() => () => restoreFocus(opener), [opener]);

  const applyView = (v: SettingsView) => setLoad({ kind: 'ready', view: v });

  const requestClose = () => {
    if (dirty) {
      setAskClose(true);
      return;
    }
    onClose();
  };

  const discardAndClose = () => {
    setEditor(null);
    setAskClose(false);
    onClose();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      if (askClose) {
        setAskClose(false);
      } else if (editor?.askDiscard) {
        setEditor({ ...editor, askDiscard: false });
      } else if (editor?.askDelete) {
        setEditor({ ...editor, askDelete: false });
      } else {
        requestClose();
      }
      return;
    }
    if (e.key === 'Tab') {
      // Tab moves within the dialog: from the last control to the first, and back.
      const items = Array.from(dialogRef.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []);
      if (items.length === 0) {
        e.preventDefault();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement;
      if (e.shiftKey && (active === first || active === dialogRef.current)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      }
    }
  };

  // Changes the default model. The chip in the status strip follows through onChanged.
  const setDefault = (key: string) => {
    if (view === null) {
      return;
    }
    const option = modelOptions(view.settings).find((o) => o.key === key);
    if (key !== BLANK_PRESET && option === undefined) {
      return;
    }
    setDefaults({ busy: true, error: null });
    api.setDefaultModel(option ? option.ref : null).then(
      (v) => {
        applyView(v);
        setDefaults({ busy: false, error: null });
        onChanged();
      },
      (e: unknown) => setDefaults({ busy: false, error: errorText(e) }),
    );
  };

  const commitLimit = (text: string) => {
    if (view === null) {
      return;
    }
    if (text.trim() !== '' && parsePositiveInt(text) === null) {
      setLimit((l) => ({ ...l, error: LIMIT_ERROR }));
      return;
    }
    const value = parsePositiveInt(text);
    if (value === view.settings.global_context_limit) {
      setLimit((l) => ({ ...l, error: null }));
      return;
    }
    setLimit((l) => ({ ...l, busy: true, error: null }));
    api.setGlobalContextLimit(value).then(
      (v) => {
        applyView(v);
        setLimit({ text: limitText(v), error: null, busy: false });
        onChanged();
      },
      (e: unknown) => setLimit((l) => ({ ...l, busy: false, error: errorText(e) })),
    );
  };

  /** Opening or adding a provider while another has unsaved changes is refused, with the reason shown. */
  const mayReplaceEditor = (): boolean => {
    if (dirty) {
      setNotice('Save or cancel the open provider first.');
      return false;
    }
    return true;
  };

  const openProvider = (id: string) => {
    if (view === null || !mayReplaceEditor()) {
      return;
    }
    if (editor?.forId === id) {
      setEditor(null);
      return;
    }
    const provider = view.settings.providers.find((p) => p.id === id);
    if (provider === undefined) {
      return;
    }
    const draft = draftFromProvider(provider, view.secrets[id]);
    setEditor(newEditor(id, draft, draft));
    setNotice(null);
  };

  const addProvider = (key: string) => {
    if (view === null || !mayReplaceEditor()) {
      return;
    }
    const taken = view.settings.providers.map((p) => p.id);
    let draft: ProviderDraft;
    if (key === BLANK_PRESET) {
      draft = draftNew();
    } else {
      const preset = presets.find((p) => p.key === key);
      if (preset === undefined) {
        return;
      }
      draft = draftFromPreset(preset, taken);
    }
    setEditor(newEditor(null, draft, null));
    setNotice(null);
  };

  const cancelEditor = () => {
    setEditor(null);
    setNotice(null);
  };

  const save = () => {
    if (view === null || editor === null) {
      return;
    }
    const input = buildSaveInput(editor.draft);
    const id = input.provider.id;
    setEditor({ ...editor, saving: true, saveError: null });
    api.saveProvider(input).then(
      (v) => {
        applyView(v);
        const provider = v.settings.providers.find((p) => p.id === id);
        if (provider === undefined) {
          setEditor(null);
        } else {
          // The draft is rebuilt from the saved provider: the typed secrets are gone, and the stored flags are the new ones.
          const draft = draftFromProvider(provider, v.secrets[id]);
          setEditor(newEditor(id, draft, draft));
        }
        setNotice(null);
        onChanged();
      },
      (e: unknown) => setEditor((cur) => (cur ? { ...cur, saving: false, saveError: errorText(e) } : cur)),
    );
  };

  const deleteProvider = () => {
    if (editor === null || editor.forId === null) {
      return;
    }
    const id = editor.forId;
    setEditor({ ...editor, saving: true, saveError: null, askDelete: false });
    api.deleteProvider(id).then(
      (v) => {
        applyView(v);
        setEditor(null);
        setNotice(null);
        onChanged();
      },
      (e: unknown) => setEditor((cur) => (cur ? { ...cur, saving: false, saveError: errorText(e) } : cur)),
    );
  };

  const fetchModels = () => {
    if (editor === null || editor.forId === null) {
      return;
    }
    const id = editor.forId;
    setEditor({ ...editor, fetching: true, fetchNote: null, fetchError: null });
    api.fetchModels(id).then(
      (out) => {
        applyView(out.view);
        const provider = out.view.settings.providers.find((p) => p.id === id);
        setEditor((cur) => {
          if (cur === null || cur.forId !== id) {
            return cur;
          }
          if (provider === undefined) {
            return null;
          }
          const draft = draftFromProvider(provider, out.view.secrets[id]);
          return { ...newEditor(id, draft, draft), fetchNote: fetchSummaryText(out.summary) };
        });
        onChanged();
      },
      (e: unknown) => setEditor((cur) => (cur && cur.forId === id ? { ...cur, fetching: false, fetchError: errorText(e) } : cur)),
    );
  };

  const testModel = (modelId: string) => {
    if (editor === null || editor.forId === null) {
      return;
    }
    const id = editor.forId;
    const show = (state: TestState) =>
      setEditor((cur) => (cur && cur.forId === id ? { ...cur, tests: { ...cur.tests, [modelId]: state } } : cur));
    show({ kind: 'running' });
    api.testConnection(id, modelId).then(
      (result) => show({ kind: 'done', result }),
      (e: unknown) => show({ kind: 'failed', message: errorText(e) }),
    );
  };

  const locked = view !== null && view.load_error !== null;
  const takenIds = view === null ? [] : view.settings.providers.filter((p) => p.id !== editor?.forId).map((p) => p.id);

  let editorSlot: { forId: string | null; node: ReactNode } | null = null;
  if (editor !== null && view !== null) {
    const defaultModel = view.settings.default_model;
    editorSlot = {
      forId: editor.forId,
      node: (
        <ProviderEditor
          key={editor.forId ?? 'new'}
          draft={editor.draft}
          saved={editor.saved}
          problems={validateDraft(editor.draft, takenIds)}
          takenIds={takenIds}
          locked={locked}
          saving={editor.saving}
          saveError={editor.saveError}
          defaultModelId={defaultModel && defaultModel.provider_id === editor.forId ? defaultModel.model_id : null}
          fetching={editor.fetching}
          fetchNote={editor.fetchNote}
          fetchError={editor.fetchError}
          tests={editor.tests}
          askDelete={editor.askDelete}
          askDiscard={editor.askDiscard}
          onChange={(draft) => setEditor((cur) => (cur ? { ...cur, draft } : cur))}
          onSave={save}
          onAskDelete={(ask) => setEditor((cur) => (cur ? { ...cur, askDelete: ask } : cur))}
          onDelete={deleteProvider}
          onAskDiscard={(ask) => setEditor((cur) => (cur ? { ...cur, askDiscard: ask } : cur))}
          onCancel={cancelEditor}
          onFetch={fetchModels}
          onTest={testModel}
        />
      ),
    };
  }

  return (
    <div
      className="settings-backdrop"
      onMouseDown={(e) => {
        backdropPressed.current = e.target === e.currentTarget;
      }}
      onClick={(e) => {
        // Only a press and release on the backdrop itself closes it, so a drag that ends outside a field does not.
        if (backdropPressed.current && e.target === e.currentTarget) {
          requestClose();
        }
      }}
    >
      <div
        ref={dialogRef}
        className="settings-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        <header className="settings-head">
          <h2 className="settings-title" id={titleId}>
            AI settings
          </h2>
        </header>
        <div className="settings-body">
          <SettingsBody
            load={load}
            locked={locked}
            presets={presets}
            defaults={defaults}
            limit={limit}
            notice={notice}
            editor={editorSlot}
            onRetry={() => {
              setLoad({ kind: 'loading' });
              setAttempt((a) => a + 1);
            }}
            onCheckAgain={() => {
              setLoad({ kind: 'loading' });
              setAttempt((a) => a + 1);
            }}
            onDefault={setDefault}
            onLimitText={(text) => setLimit((l) => ({ ...l, text, error: null }))}
            onLimitCommit={() => commitLimit(limit.text)}
            onLimitClear={() => {
              setLimit((l) => ({ ...l, text: '' }));
              commitLimit('');
            }}
            onOpenProvider={openProvider}
            onAddProvider={addProvider}
          />
        </div>
        <footer className="settings-foot">
          {askClose ? (
            <>
              <span className="settings-warn">Discard unsaved provider changes?</span>
              <button type="button" className="settings-danger" onClick={discardAndClose}>
                Discard and close
              </button>
              <button type="button" onClick={() => setAskClose(false)}>
                Keep editing
              </button>
            </>
          ) : (
            <button type="button" onClick={requestClose}>
              Close
            </button>
          )}
        </footer>
      </div>
    </div>
  );
}

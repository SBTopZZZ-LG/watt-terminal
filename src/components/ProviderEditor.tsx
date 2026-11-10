import { useId, useState } from 'react';
import type { TestResult } from '../lib/types';
import {
  hasProblems,
  hostChangeNotice,
  isAutoTicked,
  isDirty,
  isLocalHttp,
  modelsUrlFor,
  newHeaderDraft,
  renameHeader,
  toggleSecret,
  withId,
  withName,
} from '../lib/settingsForm';
import type { DraftProblems, HeaderDraft, ProviderDraft } from '../lib/settingsForm';

/** One model's connection test: running, the backend's answer, or a failed call. */
export type TestState = { kind: 'running' } | { kind: 'done'; result: TestResult } | { kind: 'failed'; message: string };

export interface ProviderEditorProps {
  draft: ProviderDraft;
  /**
   * The baseline the draft is compared with: what is saved, or for a provider not saved yet, the draft it started from. Null
   * means no baseline, which counts as unsaved. The dirty marker and the save guard compare to it.
   */
  saved: ProviderDraft | null;
  problems: DraftProblems;
  /** The ids of the other providers, for the id rules of a new provider. */
  takenIds: readonly string[];
  /** The settings file is broken: nothing here can change. */
  locked: boolean;
  saving: boolean;
  saveError: string | null;
  /** The id of this provider's model that is the default, if any. It cannot be removed. */
  defaultModelId: string | null;
  fetching: boolean;
  fetchNote: string | null;
  fetchError: string | null;
  tests: Record<string, TestState>;
  askDelete: boolean;
  askDiscard: boolean;
  onChange(draft: ProviderDraft): void;
  onSave(): void;
  onAskDelete(ask: boolean): void;
  onDelete(): void;
  onAskDiscard(ask: boolean): void;
  /** Closes the editor and drops the draft. */
  onCancel(): void;
  onFetch(): void;
  onTest(modelId: string): void;
}

const NEEDS_SAVE_FIRST = 'Save the provider first. Fetching and testing use the saved provider.';
const NEEDS_SAVE_CHANGES = 'Save your changes first. Fetching and testing use the saved provider.';

function TestLine({ state }: { state: TestState }) {
  if (state.kind === 'running') {
    return <span className="settings-test" role="status">Testing...</span>;
  }
  if (state.kind === 'failed') {
    return (
      <span className="settings-test settings-test-fail" role="status">
        {`Test failed: ${state.message}`}
      </span>
    );
  }
  const { ok, message, millis } = state.result;
  return (
    <span className={`settings-test ${ok ? 'settings-test-ok' : 'settings-test-fail'}`} role="status">
      {`${ok ? 'ok' : 'failed'} · ${millis} ms · ${message}`}
    </span>
  );
}

/**
 * The editor for one provider, drawn inline under its row. It holds no draft of its own: every edit is passed up. The only
 * state here is the text of the "add model" box. Secret inputs are plain controlled inputs: the typed value is in the draft,
 * and nothing is filled in from the store.
 */
export function ProviderEditor(p: ProviderEditorProps) {
  const uid = useId();
  const [newModel, setNewModel] = useState('');
  const { draft } = p;
  const dirty = isDirty(draft, p.saved);
  // A provider not saved yet is pending even when untouched: fetch, test and save all wait for its first save.
  const pending = draft.isNew || dirty;
  const busy = p.saving || p.fetching;
  const off = p.locked || busy;
  const set = (patch: Partial<ProviderDraft>) => p.onChange({ ...draft, ...patch });

  // Fetching and testing work on the saved provider, so they need it saved and unchanged.
  const unsavedReason = draft.isNew ? NEEDS_SAVE_FIRST : NEEDS_SAVE_CHANGES;
  const fetchBlock = pending
    ? unsavedReason
    : modelsUrlFor(draft) === null
      ? 'This provider has no model list URL. Set one above and save.'
      : null;
  const testBlock = pending ? unsavedReason : null;

  const keyLocal = isLocalHttp(draft.chat_url);
  const keyInputShown = !draft.clearKey && (!draft.storedKey || draft.keyEditing);
  const newModelId = newModel.trim();
  const newModelTaken = draft.models.some((m) => m.id === newModelId);
  const saveBlocked = off || !pending || hasProblems(p.problems);
  // A saved provider's key or secret headers would go to a changed host, so the editor says so. Never blocks a save.
  const hasStoredSecrets = p.saved !== null && (p.saved.storedKey || p.saved.storedHeaders.length > 0);
  const hostNotice = hostChangeNotice(p.saved, draft, hasStoredSecrets);

  const setHeader = (key: string, patch: Partial<HeaderDraft>) =>
    set({ headers: draft.headers.map((h) => (h.key === key ? { ...h, ...patch } : h)) });

  const updateHeader = (key: string, change: (h: HeaderDraft) => HeaderDraft) =>
    set({ headers: draft.headers.map((h) => (h.key === key ? change(h) : h)) });

  const addModel = () => {
    if (newModelId === '' || newModelTaken) {
      return;
    }
    set({ models: [...draft.models, { id: newModelId, source: 'custom', context_limit: '', provider_context: null }] });
    setNewModel('');
  };

  return (
    <section className="settings-editor" aria-label={draft.isNew ? 'New provider' : `Provider ${draft.name}`}>
      <div className="settings-editor-head">
        <span className="settings-editor-title">{draft.isNew ? 'New provider' : draft.name}</span>
        {dirty && <span className="settings-unsaved">Unsaved changes</span>}
      </div>

      <div className="settings-fields">
        <div className="settings-field">
          <label htmlFor={`${uid}-name`}>Name</label>
          <input
            id={`${uid}-name`}
            type="text"
            value={draft.name}
            disabled={off}
            aria-invalid={p.problems.name !== undefined || undefined}
            aria-describedby={p.problems.name ? `${uid}-name-err` : undefined}
            onChange={(e) => p.onChange(withName(draft, e.target.value, p.takenIds))}
          />
          {p.problems.name && <span className="settings-error" id={`${uid}-name-err`}>{p.problems.name}</span>}
        </div>

        <div className="settings-field">
          <label htmlFor={`${uid}-id`}>Id</label>
          <input
            id={`${uid}-id`}
            type="text"
            value={draft.id}
            readOnly={!draft.isNew}
            disabled={off}
            spellCheck={false}
            aria-invalid={p.problems.id !== undefined || undefined}
            aria-describedby={`${uid}-id-hint`}
            onChange={(e) => p.onChange(withId(draft, e.target.value))}
          />
          <span className="settings-hint" id={`${uid}-id-hint`}>
            {draft.isNew ? 'Made from the name. Fixed after the first save.' : 'Fixed. Names the stored secrets.'}
          </span>
          {p.problems.id && <span className="settings-error">{p.problems.id}</span>}
        </div>

        <div className="settings-field settings-field-wide">
          <label htmlFor={`${uid}-chat`}>Chat URL</label>
          <input
            id={`${uid}-chat`}
            type="text"
            value={draft.chat_url}
            disabled={off}
            spellCheck={false}
            placeholder="https://.../v1/chat/completions"
            aria-invalid={p.problems.chat_url !== undefined || undefined}
            onChange={(e) => set({ chat_url: e.target.value })}
          />
          {p.problems.chat_url && <span className="settings-error">{p.problems.chat_url}</span>}
        </div>

        <div className="settings-field settings-field-wide">
          <label htmlFor={`${uid}-models`}>Model list URL</label>
          <input
            id={`${uid}-models`}
            type="text"
            value={draft.models_url}
            disabled={off}
            spellCheck={false}
            placeholder="Blank: taken from the chat URL when it ends in /chat/completions"
            aria-invalid={p.problems.models_url !== undefined || undefined}
            onChange={(e) => set({ models_url: e.target.value })}
          />
          {p.problems.models_url && <span className="settings-error">{p.problems.models_url}</span>}
          {hostNotice !== null && (
            <span className="settings-warn" role="status">
              {hostNotice}
            </span>
          )}
        </div>

        <div className="settings-field">
          <label className="settings-check">
            <input type="checkbox" checked={draft.stream} disabled={off} onChange={(e) => set({ stream: e.target.checked })} />
            Stream replies
          </label>
        </div>

        <div className="settings-field">
          <label htmlFor={`${uid}-max`}>Max output tokens</label>
          <input
            id={`${uid}-max`}
            type="number"
            min={1}
            step={1}
            value={draft.max_tokens}
            placeholder="Provider default"
            disabled={off}
            aria-invalid={p.problems.max_tokens !== undefined || undefined}
            onChange={(e) => set({ max_tokens: e.target.value })}
          />
          {p.problems.max_tokens && <span className="settings-error">{p.problems.max_tokens}</span>}
        </div>
      </div>

      <div className="settings-section-block">
        <span className="settings-label" id={`${uid}-key-label`}>
          {keyLocal ? 'API key (optional for a local server)' : 'API key'}
        </span>
        {draft.storedKey && !draft.keyEditing && !draft.clearKey && (
          <div className="settings-inline">
            <span className="settings-tag settings-tag-ok">stored</span>
            <button type="button" disabled={off} onClick={() => set({ keyEditing: true, clearKey: false })}>
              Replace
            </button>
            <button type="button" disabled={off} onClick={() => set({ clearKey: true, keyEditing: false, apiKey: '' })}>
              Remove
            </button>
          </div>
        )}
        {draft.clearKey && (
          <div className="settings-inline">
            <span className="settings-warn">The stored key is removed when you save.</span>
            <button type="button" disabled={off} onClick={() => set({ clearKey: false })}>
              Undo
            </button>
          </div>
        )}
        {keyInputShown && (
          <div className="settings-inline">
            <input
              id={`${uid}-key`}
              type="password"
              autoComplete="off"
              spellCheck={false}
              aria-labelledby={`${uid}-key-label`}
              value={draft.apiKey}
              disabled={off}
              placeholder={draft.storedKey ? 'Type a new key' : 'Paste the API key'}
              onChange={(e) => set({ apiKey: e.target.value, clearKey: false })}
            />
            {draft.storedKey && (
              <button type="button" disabled={off} onClick={() => set({ keyEditing: false, apiKey: '' })}>
                Keep stored
              </button>
            )}
          </div>
        )}
        {!draft.storedKey && draft.apiKey.trim() === '' && !draft.clearKey && !keyLocal && (
          <span className="settings-hint">No key stored. Requests to this provider fail until one is saved.</span>
        )}
        <span className="settings-hint">A typed key is never shown again, and a stored key is not read back into this form.</span>
      </div>

      <div className="settings-section-block">
        <h4 className="settings-section-title">Headers</h4>
        {draft.headers.length > 0 && (
          <table className="settings-headers">
            <thead>
              <tr>
                <th scope="col">Name</th>
                <th scope="col">Value</th>
                <th scope="col">Secret</th>
                <th scope="col">
                  <span className="settings-sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {draft.headers.map((h) => {
                // A renamed saved secret keeps its stored value under the new name (see buildSaveInput), so it still shows as stored.
                const stored =
                  (draft.storedHeaders.includes(h.name.trim()) && h.name.trim() !== '') ||
                  (h.savedName !== null && draft.storedHeaders.includes(h.savedName));
                const error = p.problems.headers[h.key];
                return (
                  <tr key={h.key}>
                    <td>
                      <input
                        type="text"
                        aria-label="Header name"
                        value={h.name}
                        disabled={off}
                        spellCheck={false}
                        aria-invalid={error !== undefined || undefined}
                        onChange={(e) => updateHeader(h.key, (x) => renameHeader(x, e.target.value))}
                      />
                      {error && <span className="settings-error">{error}</span>}
                      {isAutoTicked(h) && <span className="settings-hint">Looks like a credential, so it is stored in the Keychain.</span>}
                    </td>
                    <td>
                      {h.secret ? (
                        <div className="settings-inline">
                          <input
                            type="password"
                            autoComplete="off"
                            spellCheck={false}
                            aria-label={`Value of secret header ${h.name.trim()}`}
                            value={h.value}
                            disabled={off}
                            placeholder={stored && !h.clearStored ? 'Stored. Type to replace' : 'Value'}
                            onChange={(e) => setHeader(h.key, { value: e.target.value, clearStored: false })}
                          />
                          {stored && !h.clearStored && <span className="settings-tag settings-tag-ok">stored</span>}
                          {stored && !h.clearStored && (
                            <button type="button" disabled={off} onClick={() => setHeader(h.key, { clearStored: true, value: '' })}>
                              Remove stored
                            </button>
                          )}
                          {h.clearStored && <span className="settings-warn">Removed when you save</span>}
                        </div>
                      ) : (
                        <input
                          type="text"
                          aria-label={`Value of header ${h.name.trim()}`}
                          value={h.value}
                          disabled={off}
                          spellCheck={false}
                          onChange={(e) => setHeader(h.key, { value: e.target.value })}
                        />
                      )}
                    </td>
                    <td>
                      <input
                        type="checkbox"
                        aria-label={`Secret: ${h.name.trim() || 'new header'}`}
                        checked={h.secret}
                        disabled={off}
                        onChange={(e) => updateHeader(h.key, (x) => toggleSecret(x, e.target.checked))}
                      />
                    </td>
                    <td>
                      <button
                        type="button"
                        disabled={off}
                        aria-label={`Remove header ${h.name.trim() || 'new header'}`}
                        onClick={() => set({ headers: draft.headers.filter((x) => x.key !== h.key) })}
                      >
                        Remove
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
        <div className="settings-inline">
          <button
            type="button"
            disabled={off}
            onClick={() => set({ headers: [...draft.headers, newHeaderDraft()] })}
          >
            Add header
          </button>
        </div>
        <span className="settings-hint">
          A value may use {'{session_id}'}, which is replaced by the conversation id. OpenCode Go needs x-opencode-session: {'{session_id}'}.
          Secret values are kept in the store, not in settings.json. Renaming a secret header keeps its stored value, unless a new value is typed.
        </span>
      </div>

      <div className="settings-section-block">
        <h4 className="settings-section-title">Models</h4>
        {draft.models.length === 0 && <p className="settings-hint">No models yet. Fetch the list, or add one by id.</p>}
        <ul className="settings-models">
          {draft.models.map((m) => {
            const isDefault = p.defaultModelId === m.id;
            const error = p.problems.models[m.id];
            return (
              <li key={m.id} className="settings-model">
                <div className="settings-inline">
                  <span className="settings-model-id">{m.id}</span>
                  <span className="settings-tag">{m.source}</span>
                  {m.provider_context !== null && (
                    <span className="settings-dim">{`${m.provider_context.toLocaleString('en-US')} context`}</span>
                  )}
                  {isDefault && <span className="settings-tag settings-tag-accent">default</span>}
                </div>
                <div className="settings-inline">
                  <label className="settings-inline-label">
                    Limit
                    <input
                      type="number"
                      min={1}
                      step={1}
                      aria-label={`Context limit for ${m.id}`}
                      placeholder="none"
                      value={m.context_limit}
                      disabled={off}
                      aria-invalid={error !== undefined || undefined}
                      onChange={(e) =>
                        set({ models: draft.models.map((x) => (x.id === m.id ? { ...x, context_limit: e.target.value } : x)) })
                      }
                    />
                  </label>
                  <button
                    type="button"
                    disabled={off || pending}
                    aria-label={`Test ${m.id}`}
                    title={testBlock ?? 'Send one short test message to this model'}
                    onClick={() => p.onTest(m.id)}
                  >
                    Test
                  </button>
                  <button
                    type="button"
                    disabled={off || isDefault}
                    aria-label={`Remove model ${m.id}`}
                    title={isDefault ? 'This is the default model. Choose another default first.' : 'Remove this model'}
                    onClick={() => set({ models: draft.models.filter((x) => x.id !== m.id) })}
                  >
                    Remove
                  </button>
                </div>
                {error && <span className="settings-error">{error}</span>}
                {p.tests[m.id] && <TestLine state={p.tests[m.id]} />}
              </li>
            );
          })}
        </ul>
        <div className="settings-inline">
          <input
            type="text"
            aria-label="New model id"
            placeholder="Model id"
            value={newModel}
            disabled={off}
            spellCheck={false}
            onChange={(e) => setNewModel(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                addModel();
              }
            }}
          />
          <button
            type="button"
            disabled={off || newModelId === '' || newModelTaken}
            title={newModelTaken ? 'That model is already listed.' : undefined}
            onClick={addModel}
          >
            Add custom model
          </button>
        </div>
        <div className="settings-inline">
          <button
            type="button"
            disabled={off || fetchBlock !== null}
            title={fetchBlock ?? 'Fetch the model list from the provider and merge it into the saved models'}
            onClick={p.onFetch}
          >
            {p.fetching ? 'Fetching...' : 'Fetch models'}
          </button>
          {p.fetchNote && <span className="settings-hint" role="status">{p.fetchNote}</span>}
          {p.fetchError && <span className="settings-error" role="alert">{p.fetchError}</span>}
        </div>
        <span className="settings-hint">
          Test sends one tiny request, which costs a few tokens. It needs the provider and the model saved.
        </span>
      </div>

      <div className="settings-editor-foot">
        {p.saveError !== null && (
          <div className="settings-error settings-error-block" role="alert">
            {p.saveError}
          </div>
        )}
        {hasProblems(p.problems) && <span className="settings-hint">Fix the fields marked above to save.</span>}
        {p.askDiscard ? (
          <div className="settings-inline">
            <span className="settings-warn">Discard unsaved changes?</span>
            <button type="button" className="settings-danger" onClick={p.onCancel}>
              Discard
            </button>
            <button type="button" onClick={() => p.onAskDiscard(false)}>
              Keep editing
            </button>
          </div>
        ) : (
          <div className="settings-inline">
            <button type="button" className="settings-primary" disabled={saveBlocked} onClick={p.onSave}>
              {p.saving ? 'Saving...' : 'Save provider'}
            </button>
            <button type="button" disabled={busy} onClick={() => (dirty ? p.onAskDiscard(true) : p.onCancel())}>
              Cancel
            </button>
            {!draft.isNew &&
              (p.askDelete ? (
                <span className="settings-inline">
                  <span className="settings-warn">Delete this provider and its stored keys?</span>
                  <button type="button" className="settings-danger" disabled={off} onClick={p.onDelete}>
                    Yes, delete
                  </button>
                  <button type="button" onClick={() => p.onAskDelete(false)}>
                    No
                  </button>
                </span>
              ) : (
                <button type="button" className="settings-danger" disabled={off} onClick={() => p.onAskDelete(true)}>
                  Delete provider
                </button>
              ))}
          </div>
        )}
      </div>
    </section>
  );
}

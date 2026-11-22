import { useId, useState } from 'react';
import { errorText } from '../lib/api';
import { buildServerInput, LEVEL_LABEL, MCP_PRESETS, presetDraft } from '../lib/mcpForm';
import type { McpFormDraft, McpPresetKey, SecretRow } from '../lib/mcpForm';
import type { McpImportResult, McpLevel, McpServerConfig, McpServerInput } from '../lib/types';

export interface McpServerFormProps {
  /** The draft the form starts from. It is read once: the parent keys the form so a new preset or server remounts it. */
  initial: McpFormDraft;
  /** The saved config of an edit: its tool overrides and timeout are kept on save. */
  original?: McpServerConfig;
  /** The names of the saved servers. A new server may not take one. */
  takenNames: readonly string[];
  busy: boolean;
  /** Saves the input. Rejects with the backend's message, which the form shows. */
  onSave(input: McpServerInput): Promise<void>;
  onCancel(): void;
}

/**
 * A server's form: name, transport, the command or URL, and the secret rows. Used for a new server (from a preset or blank)
 * and for an edit. Checks run here (lib/mcpForm); the backend still checks on save and its message is shown. Typed secret
 * values stay in this form only until it closes: they are never shown again, and a stored one is never read back.
 */
export function McpServerForm({ initial, original, takenNames, busy, onSave, onCancel }: McpServerFormProps) {
  const uid = useId();
  const [draft, setDraft] = useState<McpFormDraft>(initial);
  const [problems, setProblems] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const off = busy || saving;

  const set = (patch: Partial<McpFormDraft>) => {
    setDraft((d) => ({ ...d, ...patch }));
    setError(null);
  };

  const submit = () => {
    const built = buildServerInput(draft, original, takenNames);
    if (!built.ok) {
      setProblems(built.problems);
      setError(null);
      return;
    }
    setProblems([]);
    setSaving(true);
    setError(null);
    onSave(built.input).then(
      () => setSaving(false),
      (e: unknown) => {
        setSaving(false);
        setError(errorText(e));
      },
    );
  };

  const isStdio = draft.transport === 'stdio';
  return (
    <form
      className="settings-mcp-form"
      aria-label={draft.isNew ? 'New MCP server' : `Edit ${draft.name}`}
      autoComplete="off"
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <div className="settings-fields">
        <div className="settings-field">
          <label htmlFor={`${uid}-name`}>Name</label>
          <input
            id={`${uid}-name`}
            type="text"
            spellCheck={false}
            value={draft.name}
            readOnly={!draft.isNew}
            disabled={off}
            placeholder="fetch"
            aria-describedby={`${uid}-name-hint`}
            onChange={(e) => set({ name: e.target.value })}
          />
          <span className="settings-hint" id={`${uid}-name-hint`}>
            {draft.isNew ? 'Prefixes its tools: mcp__name__tool. 1 to 32 characters: a-z, 0-9, "-" or "_".' : 'The name cannot change.'}
          </span>
        </div>
        <div className="settings-field">
          <span className="settings-label" id={`${uid}-transport-label`}>
            Connects by
          </span>
          <div className="settings-inline" role="radiogroup" aria-labelledby={`${uid}-transport-label`}>
            <label className="settings-check">
              <input type="radio" name={`${uid}-transport`} checked={isStdio} disabled={off} onChange={() => set({ transport: 'stdio' })} />
              Command (stdio)
            </label>
            <label className="settings-check">
              <input type="radio" name={`${uid}-transport`} checked={!isStdio} disabled={off} onChange={() => set({ transport: 'http' })} />
              URL (HTTP)
            </label>
          </div>
        </div>
      </div>

      {isStdio ? (
        <div className="settings-fields">
          <div className="settings-field">
            <label htmlFor={`${uid}-command`}>Command</label>
            <input
              id={`${uid}-command`}
              type="text"
              spellCheck={false}
              value={draft.command}
              disabled={off}
              placeholder="npx"
              onChange={(e) => set({ command: e.target.value })}
            />
          </div>
          <div className="settings-field">
            <label htmlFor={`${uid}-cwd`}>Working folder (optional)</label>
            <input id={`${uid}-cwd`} type="text" spellCheck={false} value={draft.cwd} disabled={off} onChange={(e) => set({ cwd: e.target.value })} />
          </div>
          {draft.folders !== null ? (
            <div className="settings-field settings-field-wide">
              <label htmlFor={`${uid}-folders`}>Folders the server may read, one per line</label>
              <textarea
                id={`${uid}-folders`}
                rows={3}
                spellCheck={false}
                value={draft.folders}
                disabled={off}
                onChange={(e) => set({ folders: e.target.value })}
              />
              <span className="settings-hint">Use full paths. The server has no read-only switch.</span>
            </div>
          ) : (
            <div className="settings-field settings-field-wide">
              <label htmlFor={`${uid}-args`}>Arguments, one per line</label>
              <textarea id={`${uid}-args`} rows={3} spellCheck={false} value={draft.args} disabled={off} onChange={(e) => set({ args: e.target.value })} />
            </div>
          )}
          <SecretRows
            uid={`${uid}-env`}
            kind="env"
            rows={draft.env}
            disabled={off}
            onChange={(env) => set({ env })}
          />
        </div>
      ) : (
        <div className="settings-fields">
          <div className="settings-field settings-field-wide">
            <label htmlFor={`${uid}-url`}>URL</label>
            <input id={`${uid}-url`} type="text" spellCheck={false} value={draft.url} disabled={off} placeholder="https://" onChange={(e) => set({ url: e.target.value })} />
            <span className="settings-hint">https, or http for localhost only. A key in the query string is not hidden from the settings view, so keep it in the headers.</span>
          </div>
          <SecretRows uid={`${uid}-hdr`} kind="header" rows={draft.headers} disabled={off} onChange={(headers) => set({ headers })} />
        </div>
      )}

      <div className="settings-fields">
        <div className="settings-field">
          <label htmlFor={`${uid}-approval`}>Approval for every tool of this server</label>
          <select id={`${uid}-approval`} value={draft.approval} disabled={off} onChange={(e) => set({ approval: e.target.value as McpLevel })}>
            {(Object.keys(LEVEL_LABEL) as McpLevel[]).map((level) => (
              <option key={level} value={level}>
                {LEVEL_LABEL[level]}
              </option>
            ))}
          </select>
        </div>
        <div className="settings-field">
          <span className="settings-label">Enabled</span>
          <label className="settings-check">
            <input type="checkbox" checked={draft.enabled} disabled={off} onChange={(e) => set({ enabled: e.target.checked })} />
            Offer this server's tools to the model
          </label>
        </div>
      </div>

      <div className="settings-editor-foot">
        {problems.length > 0 && (
          <div className="settings-error settings-error-block" role="alert">
            {problems.map((p) => (
              <span key={p} className="settings-error-line">
                {p}
              </span>
            ))}
          </div>
        )}
        {error !== null && (
          <div className="settings-error settings-error-block" role="alert">
            {error}
          </div>
        )}
        <div className="settings-inline">
          <button type="submit" className="settings-primary" disabled={off}>
            {saving ? 'Saving...' : draft.isNew ? 'Add server' : 'Save server'}
          </button>
          <button type="button" disabled={off} onClick={onCancel}>
            Cancel
          </button>
        </div>
      </div>
    </form>
  );
}

interface SecretRowsProps {
  uid: string;
  kind: 'env' | 'header';
  rows: SecretRow[];
  disabled: boolean;
  onChange(rows: SecretRow[]): void;
}

/**
 * The env variables or headers of a server, each with a name and a masked value. A stored value shows only the "stored"
 * tag: leave the value empty to keep it, type to replace it, or Remove it (on save). An env variable's removal keeps its
 * name; a header is dropped from the server. A header named Authorization takes the bare key (sent as Bearer).
 */
function SecretRows({ uid, kind, rows, disabled, onChange }: SecretRowsProps) {
  const update = (index: number, patch: Partial<SecretRow>) => onChange(rows.map((r, i) => (i === index ? { ...r, ...patch } : r)));
  const remove = (index: number) => onChange(rows.filter((_, i) => i !== index));
  const noun = kind === 'env' ? 'variable' : 'header';
  return (
    <div className="settings-field settings-field-wide">
      <span className="settings-label">{kind === 'env' ? 'Environment variables' : 'Headers'}</span>
      {rows.map((r, i) => {
        const id = `${uid}-${i}`;
        const keyLike = kind === 'header' && r.name.trim().toLowerCase() === 'authorization';
        return (
          <div key={id} className="settings-inline settings-mcp-secret">
            <input
              type="text"
              spellCheck={false}
              aria-label={`${noun} name`}
              placeholder={kind === 'env' ? 'NAME' : 'Header name'}
              value={r.name}
              disabled={disabled}
              onChange={(e) => update(i, { name: e.target.value })}
            />
            <input
              type="password"
              autoComplete="off"
              spellCheck={false}
              aria-label={`${noun} value`}
              placeholder={
                r.remove ? 'Removed on save' : r.stored ? 'Leave empty to keep' : keyLike ? 'API key (sent as Bearer)' : r.optional ? 'Optional' : 'Value'
              }
              value={r.value}
              disabled={disabled || r.remove}
              onChange={(e) => update(i, { value: e.target.value })}
            />
            {r.stored && !r.remove && <span className="settings-tag settings-tag-ok">stored</span>}
            {r.remove && <span className="settings-tag settings-tag-warn">removed on save</span>}
            {r.stored && !r.remove && (
              <button type="button" className="settings-danger" disabled={disabled} onClick={() => update(i, { remove: true, value: '' })}>
                Remove
              </button>
            )}
            {r.remove && (
              <button type="button" disabled={disabled} onClick={() => update(i, { remove: false })}>
                Undo
              </button>
            )}
            {!r.stored && (
              <button type="button" disabled={disabled} onClick={() => remove(i)}>
                Delete row
              </button>
            )}
          </div>
        );
      })}
      <div className="settings-inline">
        <button type="button" disabled={disabled} onClick={() => onChange([...rows, { name: '', value: '', stored: false, remove: false, optional: false }])}>
          {kind === 'env' ? 'Add variable' : 'Add header'}
        </button>
      </div>
      <span className="settings-hint">A typed value is never shown again, and a stored value is not read back into this form.</span>
    </div>
  );
}

export interface McpAddPanelProps {
  busy: boolean;
  takenNames: readonly string[];
  onSave(input: McpServerInput): Promise<void>;
  onImport(text: string): Promise<McpImportResult>;
  onClose(): void;
}

/** The add panel: a preset (or a blank server) as a form, or a pasted `mcpServers` list. */
export function McpAddPanel({ busy, takenNames, onSave, onImport, onClose }: McpAddPanelProps) {
  const uid = useId();
  const [mode, setMode] = useState<'form' | 'json'>('form');
  const [preset, setPreset] = useState<McpPresetKey>('custom');
  return (
    <section className="settings-mcp-panel" aria-labelledby={`${uid}-title`}>
      <div className="settings-inline">
        <h4 className="settings-section-title" id={`${uid}-title`}>
          Add a server
        </h4>
        <div className="settings-inline" role="tablist" aria-label="How to add">
          <button type="button" role="tab" aria-selected={mode === 'form'} disabled={busy} onClick={() => setMode('form')}>
            Form
          </button>
          <button type="button" role="tab" aria-selected={mode === 'json'} disabled={busy} onClick={() => setMode('json')}>
            Paste JSON
          </button>
        </div>
      </div>
      {mode === 'form' ? (
        <>
          <div className="settings-inline">
            <label htmlFor={`${uid}-preset`}>Preset</label>
            <select id={`${uid}-preset`} value={preset} disabled={busy} onChange={(e) => setPreset(e.target.value as McpPresetKey)}>
              {MCP_PRESETS.map((p) => (
                <option key={p.key} value={p.key}>
                  {p.label}
                </option>
              ))}
            </select>
          </div>
          <McpServerForm
            key={preset}
            initial={presetDraft(preset)}
            takenNames={takenNames}
            busy={busy}
            onSave={onSave}
            onCancel={onClose}
          />
        </>
      ) : (
        <McpJsonImport busy={busy} onImport={onImport} onClose={onClose} />
      )}
    </section>
  );
}

interface McpJsonImportProps {
  busy: boolean;
  onImport(text: string): Promise<McpImportResult>;
  onClose(): void;
}

/**
 * Pastes an `mcpServers` list. A name that exists refuses the whole list. On success the text is cleared, since it may hold
 * keys, and the names added and the warnings are shown. A failure shows the backend's message, which names no value.
 */
function McpJsonImport({ busy, onImport, onClose }: McpJsonImportProps) {
  const uid = useId();
  const [text, setText] = useState('');
  const [busyHere, setBusyHere] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<McpImportResult | null>(null);
  const off = busy || busyHere;
  const submit = () => {
    if (text.trim() === '') {
      setError('Paste a JSON object with an mcpServers list.');
      return;
    }
    setBusyHere(true);
    setError(null);
    setResult(null);
    onImport(text).then(
      (r) => {
        setBusyHere(false);
        setText('');
        setResult(r);
      },
      (e: unknown) => {
        setBusyHere(false);
        setError(errorText(e));
      },
    );
  };
  return (
    <div className="settings-mcp-json">
      <label htmlFor={`${uid}-json`} className="settings-label">
        mcpServers JSON (keys in it are stored, not shown again)
      </label>
      <textarea id={`${uid}-json`} rows={6} spellCheck={false} autoComplete="off" value={text} disabled={off} onChange={(e) => setText(e.target.value)} />
      {error !== null && (
        <p className="settings-error" role="alert">
          {error}
        </p>
      )}
      {result !== null && (
        <div className="settings-notice" role="status">
          {result.added.length === 0 ? 'Nothing was added.' : `Added: ${result.added.join(', ')}.`}
          {result.warnings.map((w) => (
            <span key={w} className="settings-warn settings-error-line">
              {w}
            </span>
          ))}
        </div>
      )}
      <div className="settings-inline">
        <button type="button" className="settings-primary" disabled={off} onClick={submit}>
          {busyHere ? 'Importing...' : 'Import'}
        </button>
        <button type="button" disabled={off} onClick={onClose}>
          Close
        </button>
      </div>
    </div>
  );
}

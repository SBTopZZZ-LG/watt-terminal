import { useId, useState } from 'react';
import type { KeyboardEvent } from 'react';
import { api, errorText } from '../lib/api';
import { overRuleLimit, ruleCountText } from '../lib/approvalForm';
import type { ApprovalDraft } from '../lib/approvalForm';
import { AUTO_PER_TURN, previewText } from '../lib/approvalReasons';
import type { ApprovalMode } from '../lib/types';

export interface ApprovalSectionProps {
  draft: ApprovalDraft;
  /** The draft differs from the saved settings. */
  dirty: boolean;
  /** settings.json cannot be used, or it has a load error: nothing in this section can change. */
  locked: boolean;
  saving: boolean;
  saveError: string | null;
  /** The built-in deny list: undefined while it loads, null when it could not be read. */
  builtinDeny: string[] | null | undefined;
  onChange(draft: ApprovalDraft): void;
  onSave(): void;
  onUndo(): void;
}

const MODES: { value: ApprovalMode; label: string }[] = [
  { value: 'manual', label: 'Manual' },
  { value: 'regex', label: 'Regex rules' },
];

/**
 * The Approvals section of the AI settings: how run_command is approved. Manual asks for every command. Regex rules run a
 * command without asking only when an allow rule matches all of it and no deny rule does, and the built-in checks pass.
 * The section is controlled: the dialog holds the draft and the save, as it does for the provider editor. Saved rules are
 * checked against a command with "Try a command", which needs no draft.
 */
export function ApprovalSection(p: ApprovalSectionProps) {
  const uid = useId();
  const off = p.locked;
  const frozen = off || p.saving;
  const { draft } = p;
  const builtin = p.builtinDeny;

  return (
    <section className="settings-section" aria-labelledby={`${uid}-title`}>
      <div className="settings-editor-head">
        <h3 className="settings-section-title" id={`${uid}-title`}>
          Approvals
        </h3>
        {p.dirty && <span className="settings-unsaved">Unsaved changes</span>}
      </div>

      <fieldset className="settings-modes" disabled={frozen}>
        <legend>Mode</legend>
        <div className="settings-inline">
          {MODES.map((m) => (
            <label key={m.value} className="settings-check">
              <input
                type="radio"
                name={`${uid}-mode`}
                value={m.value}
                checked={draft.mode === m.value}
                disabled={frozen}
                onChange={() => p.onChange({ ...draft, mode: m.value })}
              />
              {m.label}
            </label>
          ))}
        </div>
      </fieldset>
      <p className="settings-hint">
        {`Regex rules run a command without asking only when an allow rule matches all of it and no deny rule does. At most ${AUTO_PER_TURN} commands run this way in one turn; after that a command asks.`}
      </p>

      <div className="settings-field settings-field-wide">
        <label htmlFor={`${uid}-allow`}>Allow rules</label>
        <textarea
          id={`${uid}-allow`}
          className="settings-rules"
          rows={4}
          spellCheck={false}
          placeholder="git status"
          value={draft.allow}
          disabled={frozen}
          aria-invalid={overRuleLimit(draft.allow) || undefined}
          aria-describedby={`${uid}-allow-count`}
          onChange={(e) => p.onChange({ ...draft, allow: e.target.value })}
        />
        <span className="settings-hint" id={`${uid}-allow-count`}>
          {ruleCountText(draft.allow)}
        </span>
      </div>

      <div className="settings-field settings-field-wide">
        <label htmlFor={`${uid}-deny`}>Deny rules</label>
        <textarea
          id={`${uid}-deny`}
          className="settings-rules"
          rows={3}
          spellCheck={false}
          placeholder={'rm .*'}
          value={draft.deny}
          disabled={frozen}
          aria-invalid={overRuleLimit(draft.deny) || undefined}
          aria-describedby={`${uid}-deny-count`}
          onChange={(e) => p.onChange({ ...draft, deny: e.target.value })}
        />
        <span className="settings-hint" id={`${uid}-deny-count`}>
          {ruleCountText(draft.deny)}
        </span>
      </div>

      <p className="settings-hint">
        One regular expression per line, matched against the whole command as it is run. A deny rule wins over an allow rule.
        Only commands made of letters, digits, spaces and the characters _-./:=@+, can run by rule; anything else always asks.
        Approve-all in the status bar overrides the rules for the session.
      </p>

      <details className="settings-builtin">
        <summary>
          {builtin === undefined
            ? 'Built-in deny list'
            : builtin === null
              ? 'Built-in deny list (not available)'
              : `Built-in deny list (${builtin.length} commands)`}
        </summary>
        {builtin !== undefined && builtin !== null && (
          <>
            <p className="settings-hint">These first words always ask, whatever the rules say.</p>
            <p className="settings-builtin-list">{builtin.join(', ')}</p>
          </>
        )}
      </details>

      {p.saveError !== null && (
        <p className="settings-error" role="alert">
          {p.saveError}
        </p>
      )}
      <div className="settings-inline">
        <button type="button" className="settings-primary" disabled={off || p.saving || !p.dirty} onClick={p.onSave}>
          {p.saving ? 'Saving...' : 'Save approvals'}
        </button>
        <button type="button" disabled={off || p.saving || !p.dirty} onClick={p.onUndo}>
          Undo changes
        </button>
      </div>

      <TryCommand disabled={off} />
    </section>
  );
}

type TryResult = { kind: 'ok'; text: string } | { kind: 'error'; text: string };

/**
 * "Try a command": what the saved mode and rules would do with a command, without running it. It tests the saved settings,
 * not the draft above, and says so. Enter or Check runs it.
 */
function TryCommand({ disabled }: { disabled: boolean }) {
  const uid = useId();
  const [command, setCommand] = useState('');
  const [result, setResult] = useState<TryResult | null>(null);
  const [busy, setBusy] = useState(false);
  const empty = command.trim() === '';

  const run = () => {
    if (busy || disabled || empty) {
      return;
    }
    setBusy(true);
    api.testApproval(command).then(
      (preview) => {
        setResult({ kind: 'ok', text: previewText(preview) });
        setBusy(false);
      },
      (e: unknown) => {
        setResult({ kind: 'error', text: errorText(e) });
        setBusy(false);
      },
    );
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      run();
    }
  };

  return (
    <div className="settings-try">
      <label className="settings-label" htmlFor={`${uid}-try`}>
        Try a command
      </label>
      <div className="settings-inline">
        <input
          id={`${uid}-try`}
          type="text"
          spellCheck={false}
          placeholder="git status"
          value={command}
          disabled={disabled}
          aria-describedby={`${uid}-try-note`}
          onChange={(e) => {
            setCommand(e.target.value);
            setResult(null);
          }}
          onKeyDown={onKeyDown}
        />
        <button type="button" disabled={disabled || busy || empty} onClick={run}>
          Check
        </button>
      </div>
      <span className="settings-hint" id={`${uid}-try-note`}>
        Checks the saved mode and rules. Unsaved changes above are not used, and nothing runs.
      </span>
      {result !== null && (
        <p className={result.kind === 'ok' ? 'settings-test' : 'settings-error'} role="status">
          {result.text}
        </p>
      )}
    </div>
  );
}

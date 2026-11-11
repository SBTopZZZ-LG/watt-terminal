import { useEffect, useId, useRef, useState } from 'react';
import type { KeyboardEvent } from 'react';
import { AUTO_PER_TURN } from '../lib/approvalReasons';
import { DEFAULT_LIMITS, HIGH_LIMIT_WARNING, isHighModelCalls, limitProblems, limitsInput } from '../lib/limitsForm';
import type { LimitsDraft } from '../lib/limitsForm';

export interface LimitsSectionProps {
  draft: LimitsDraft;
  /** The draft differs from the saved limits. */
  dirty: boolean;
  /** settings.json cannot be used, or it has a load error: nothing in this section can change. */
  locked: boolean;
  saving: boolean;
  saveError: string | null;
  onChange(draft: LimitsDraft): void;
  onSave(): void;
  /** Puts the default values in the fields. Nothing is saved until Save limits. */
  onReset(): void;
}

/** The warning on the unlimited confirm. Its words are the ones asked for: keep them exact. */
export const UNLIMITED_WARNING =
  'The agent can then keep making model calls until you cancel it. Every call re-sends the conversation, so a runaway turn can cost a lot.';

/** The help under each field: what the cap counts, and its default and range. */
const HELP = {
  modelCalls: `A turn stops after this many model replies that are not polling. Default ${DEFAULT_LIMITS.max_model_calls}, at least 10.`,
  unlimited: 'No cap on model calls in one turn. Commands per turn and wait time still apply.',
  pollCalls: `wait, get_screen and send_keys while a command you started is running are counted here, separately. Default ${DEFAULT_LIMITS.max_poll_calls}, 10 to 500.`,
  commands: `run_command calls the agent may make in one turn. Auto-approved commands are still capped at ${AUTO_PER_TURN} per turn by the approval rules. Default ${DEFAULT_LIMITS.max_commands_per_turn}, 1 to 500.`,
  waitMinutes: `Total time the agent may spend in wait calls in one turn. Default ${DEFAULT_LIMITS.max_wait_minutes}, 1 to 720.`,
};

/**
 * The Limits section of the AI settings: the per-turn caps of the agent. Each field is checked as it is typed, and Save is off
 * until every field holds a valid number and something changed. Turning unlimited on for model calls opens a confirm, and the
 * draft changes only on Confirm; turning it off needs none. Controlled: the dialog holds the draft and the save, as it does for
 * Approvals.
 */
export function LimitsSection(p: LimitsSectionProps) {
  const uid = useId();
  const { draft } = p;
  const frozen = p.locked || p.saving;
  const problems = limitProblems(draft);
  const canSave = !frozen && p.dirty && limitsInput(draft) !== null;
  const high = !draft.unlimited && isHighModelCalls(draft.modelCalls);
  const [confirming, setConfirming] = useState(false);
  const unlimitedRef = useRef<HTMLInputElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const backdropPressed = useRef(false);

  // The safe choice has the focus when the confirm opens, so Enter does not turn unlimited on.
  useEffect(() => {
    if (confirming) {
      cancelRef.current?.focus();
    }
  }, [confirming]);

  const closeConfirm = () => {
    setConfirming(false);
    unlimitedRef.current?.focus();
  };

  const confirmUnlimited = () => {
    p.onChange({ ...draft, unlimited: true });
    closeConfirm();
  };

  // The confirm is a modal over the settings. Escape cancels it, and Tab moves between its two buttons. Its keys stop here, so
  // the dialog's own Escape (close) and Tab handling never see them.
  const onConfirmKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      closeConfirm();
    } else if (e.key === 'Tab') {
      e.preventDefault();
      e.stopPropagation();
      (document.activeElement === cancelRef.current ? confirmRef.current : cancelRef.current)?.focus();
    }
  };

  const describedBy = (...ids: (string | null | undefined | false)[]) => ids.filter(Boolean).join(' ') || undefined;

  return (
    <section className="settings-section" aria-labelledby={`${uid}-title`}>
      <div className="settings-editor-head">
        <h3 className="settings-section-title" id={`${uid}-title`}>
          Limits
        </h3>
        {p.dirty && <span className="settings-unsaved">Unsaved changes</span>}
      </div>
      <p className="settings-hint">Caps on what the agent does in one turn. A change applies from the next message.</p>

      <div className="settings-fields limits-fields">
        <div className="settings-field">
          <label htmlFor={`${uid}-model-calls`}>Model calls per turn</label>
          <input
            id={`${uid}-model-calls`}
            type="number"
            inputMode="numeric"
            min={10}
            step={1}
            placeholder={`default ${DEFAULT_LIMITS.max_model_calls}`}
            value={draft.modelCalls}
            disabled={frozen || draft.unlimited}
            aria-invalid={problems.modelCalls !== undefined || undefined}
            aria-describedby={describedBy(problems.modelCalls && `${uid}-modelCalls-problem`, high && `${uid}-model-calls-warn`, `${uid}-model-calls-help`)}
            onChange={(e) => p.onChange({ ...draft, modelCalls: e.target.value })}
          />
          {problems.modelCalls !== undefined && (
            <span className="settings-error" id={`${uid}-modelCalls-problem`}>
              {problems.modelCalls}
            </span>
          )}
          {high && (
            <span className="settings-warn" id={`${uid}-model-calls-warn`}>
              {HIGH_LIMIT_WARNING}
            </span>
          )}
          <span className="settings-hint" id={`${uid}-model-calls-help`}>
            {HELP.modelCalls}
          </span>
          <label className="settings-check">
            <input
              ref={unlimitedRef}
              type="checkbox"
              checked={draft.unlimited}
              disabled={frozen}
              aria-describedby={`${uid}-unlimited-help`}
              onChange={(e) => {
                if (e.target.checked) {
                  setConfirming(true);
                } else {
                  p.onChange({ ...draft, unlimited: false });
                }
              }}
            />
            Unlimited
          </label>
          <span className="settings-hint" id={`${uid}-unlimited-help`}>
            {HELP.unlimited}
          </span>
        </div>

        <div className="settings-field">
          <label htmlFor={`${uid}-poll-calls`}>Polling calls per turn</label>
          <input
            id={`${uid}-poll-calls`}
            type="number"
            inputMode="numeric"
            min={10}
            max={500}
            step={1}
            placeholder={`default ${DEFAULT_LIMITS.max_poll_calls}`}
            value={draft.pollCalls}
            disabled={frozen}
            aria-invalid={problems.pollCalls !== undefined || undefined}
            aria-describedby={describedBy(problems.pollCalls && `${uid}-pollCalls-problem`, `${uid}-poll-calls-help`)}
            onChange={(e) => p.onChange({ ...draft, pollCalls: e.target.value })}
          />
          {problems.pollCalls !== undefined && (
            <span className="settings-error" id={`${uid}-pollCalls-problem`}>
              {problems.pollCalls}
            </span>
          )}
          <span className="settings-hint" id={`${uid}-poll-calls-help`}>
            {HELP.pollCalls}
          </span>
        </div>

        <div className="settings-field">
          <label htmlFor={`${uid}-commands`}>Commands per turn</label>
          <input
            id={`${uid}-commands`}
            type="number"
            inputMode="numeric"
            min={1}
            max={500}
            step={1}
            placeholder={`default ${DEFAULT_LIMITS.max_commands_per_turn}`}
            value={draft.commands}
            disabled={frozen}
            aria-invalid={problems.commands !== undefined || undefined}
            aria-describedby={describedBy(problems.commands && `${uid}-commands-problem`, `${uid}-commands-help`)}
            onChange={(e) => p.onChange({ ...draft, commands: e.target.value })}
          />
          {problems.commands !== undefined && (
            <span className="settings-error" id={`${uid}-commands-problem`}>
              {problems.commands}
            </span>
          )}
          <span className="settings-hint" id={`${uid}-commands-help`}>
            {HELP.commands}
          </span>
        </div>

        <div className="settings-field">
          <label htmlFor={`${uid}-wait-minutes`}>Wait time per turn (minutes)</label>
          <input
            id={`${uid}-wait-minutes`}
            type="number"
            inputMode="numeric"
            min={1}
            max={720}
            step={1}
            placeholder={`default ${DEFAULT_LIMITS.max_wait_minutes}`}
            value={draft.waitMinutes}
            disabled={frozen}
            aria-invalid={problems.waitMinutes !== undefined || undefined}
            aria-describedby={describedBy(problems.waitMinutes && `${uid}-waitMinutes-problem`, `${uid}-wait-minutes-help`)}
            onChange={(e) => p.onChange({ ...draft, waitMinutes: e.target.value })}
          />
          {problems.waitMinutes !== undefined && (
            <span className="settings-error" id={`${uid}-waitMinutes-problem`}>
              {problems.waitMinutes}
            </span>
          )}
          <span className="settings-hint" id={`${uid}-wait-minutes-help`}>
            {HELP.waitMinutes}
          </span>
        </div>
      </div>

      {p.saveError !== null && (
        <p className="settings-error" role="alert">
          {p.saveError}
        </p>
      )}
      <div className="settings-inline">
        <button type="button" className="settings-primary" disabled={!canSave} onClick={p.onSave}>
          {p.saving ? 'Saving...' : 'Save limits'}
        </button>
        <button type="button" disabled={frozen} onClick={p.onReset}>
          Reset to defaults
        </button>
      </div>

      {confirming && (
        <div
          className="modal-backdrop"
          onMouseDown={(e) => {
            backdropPressed.current = e.target === e.currentTarget;
          }}
          onClick={(e) => {
            // Only a press and release on the backdrop itself cancels, so a drag that ends outside the card does not.
            if (backdropPressed.current && e.target === e.currentTarget) {
              closeConfirm();
            }
          }}
        >
          <div
            className="settings-confirm"
            role="alertdialog"
            aria-modal="true"
            aria-labelledby={`${uid}-confirm-title`}
            aria-describedby={`${uid}-confirm-body`}
            tabIndex={-1}
            onKeyDown={onConfirmKeyDown}
          >
            <h3 className="settings-section-title" id={`${uid}-confirm-title`}>
              Turn on unlimited model calls?
            </h3>
            <p className="settings-warn" id={`${uid}-confirm-body`}>
              {UNLIMITED_WARNING}
            </p>
            <div className="settings-inline settings-confirm-foot">
              <button ref={cancelRef} type="button" onClick={closeConfirm}>
                Cancel
              </button>
              <button ref={confirmRef} type="button" className="settings-danger" onClick={confirmUnlimited}>
                Turn on unlimited
              </button>
            </div>
          </div>
        </div>
      )}
    </section>
  );
}

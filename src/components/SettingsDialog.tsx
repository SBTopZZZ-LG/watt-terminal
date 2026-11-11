import { useEffect, useId, useRef, useState } from 'react';
import type { KeyboardEvent, ReactNode } from 'react';
import { api, errorText } from '../lib/api';
import { approvalInput, draftFromApproval, isApprovalDirty } from '../lib/approvalForm';
import type { ApprovalDraft } from '../lib/approvalForm';
import { defaultDraft, draftFromLimits, isLimitsDirty, limitsInput } from '../lib/limitsForm';
import type { LimitsDraft } from '../lib/limitsForm';
import {
  buildSaveInput,
  chatHost,
  draftFromPreset,
  draftFromProvider,
  draftNew,
  fetchSummaryText,
  isDirty,
  limitCommit,
  modelOptions,
  modelRefKey,
  validateDraft,
} from '../lib/settingsForm';
import type { ProviderDraft } from '../lib/settingsForm';
import { RECOVERY_BUSY_LABEL, REVERT_CONFIRM, reasonText, recoveryExplanation, recoveryKind } from '../lib/settingsRecovery';
import type { AgentRequests, ProviderPreset, RecoverAction, SettingsStateView, SettingsView } from '../lib/types';
import { AboutSection } from './AboutSection';
import { ApprovalSection } from './ApprovalSection';
import { LimitsSection } from './LimitsSection';
import { ProviderEditor } from './ProviderEditor';
import type { TestState } from './ProviderEditor';
import { SettingsTabs } from './SettingsTabs';
import type { TabDef } from './SettingsTabs';

/** The recovery actions in flight or waiting for their confirm step. `busy` names the action that is running. */
export interface RecoveryState {
  busy: RecoverAction | 'reload' | null;
  /** The "Revert to last working settings" confirm step is showing. */
  confirming: boolean;
  error: string | null;
}

export const NO_RECOVERY: RecoveryState = { busy: null, confirming: false, error: null };

/** The settings as loaded. A failed load shows the error with Retry. */
export type LoadState = { kind: 'loading' } | { kind: 'error'; message: string } | { kind: 'ready'; view: SettingsView };

/** The editor's state: its draft and what is saved, and the in-flight and last results of its actions. */
export interface EditorState {
  /** The saved provider's id, or null for a provider not saved yet (shown at the end of the list). */
  forId: string | null;
  draft: ProviderDraft;
  /** The baseline the draft is compared with: the saved provider, or the draft a new provider started from. */
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

/**
 * The tabs of the AI settings, in order. Models holds the default model, the context limit, the providers and the editor;
 * Approvals holds the approval rules; Limits holds the per-turn caps; About shows the app's name and version. A new tab is one
 * entry here and one panel in the dialog's render.
 */
export const SETTINGS_TABS: readonly TabDef[] = [
  { id: 'models', label: 'Models' },
  { id: 'approvals', label: 'Approvals' },
  { id: 'limits', label: 'Limits' },
  { id: 'about', label: 'About' },
];

/** The select value for "None" (default model) and "Blank provider" (add provider). Preset keys are never empty. */
const BLANK_PRESET = '';
const NO_KEY_TEXT = 'API keys are stored in the file named by WATT_SECRETS_FILE.';
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

interface RecoveryPanelProps {
  state: SettingsStateView;
  recovery: RecoveryState;
  onAskRevert(ask: boolean): void;
  onRecover(action: RecoverAction): void;
  onReload(): void;
}

/**
 * What to do about a settings.json that cannot be used: the reason, the file's path (selectable), what the state means, and
 * the actions. "Revert" keeps the broken file aside and restores the last working copy, after a confirm step. "Use last
 * working settings" runs the last copy for this session and leaves the file as it is. "Reload" re-reads the file, so a file
 * fixed by hand takes effect without a restart. Without a last working copy only the reason, the path and Reload are shown.
 */
function RecoveryPanel({ state, recovery, onAskRevert, onRecover, onReload }: RecoveryPanelProps) {
  const uid = useId();
  const kind = recoveryKind(state) === 'last_good' ? 'last_good' : 'broken';
  const { busy, confirming, error } = recovery;
  const title = kind === 'broken' ? 'settings.json is broken' : 'Running on the last working settings';
  const canUseLast = kind === 'broken' && state.last_good_available;
  return (
    <section className="settings-recovery" aria-labelledby={`${uid}-title`}>
      <h3 className="settings-section-title" id={`${uid}-title`}>
        {title}
      </h3>
      <p className="settings-error" role="alert">
        {`The file could not be used: ${reasonText(state)}`}
      </p>
      <p className="settings-path">{state.path}</p>
      <p className="settings-hint">{recoveryExplanation(kind)}</p>
      {error !== null && (
        <p className="settings-error" role="alert">
          {error}
        </p>
      )}
      {confirming ? (
        <div className="settings-inline">
          <span className="settings-warn">{REVERT_CONFIRM}</span>
          <button type="button" className="settings-danger" disabled={busy !== null} onClick={() => onRecover('revert')}>
            {busy === 'revert' ? RECOVERY_BUSY_LABEL.revert : 'Yes'}
          </button>
          <button type="button" disabled={busy !== null} onClick={() => onAskRevert(false)}>
            No
          </button>
        </div>
      ) : (
        <div className="settings-inline">
          {state.last_good_available && (
            <button type="button" disabled={busy !== null} onClick={() => onAskRevert(true)}>
              Revert to last working settings
            </button>
          )}
          {canUseLast && (
            <button type="button" disabled={busy !== null} onClick={() => onRecover('use_last_good')}>
              {busy === 'use_last_good' ? RECOVERY_BUSY_LABEL.use_last_good : 'Use last working settings this session'}
            </button>
          )}
          <button type="button" disabled={busy !== null} onClick={onReload}>
            {busy === 'reload' ? RECOVERY_BUSY_LABEL.reload : 'Reload settings file'}
          </button>
        </div>
      )}
      {!confirming && canUseLast && <p className="settings-hint">{recoveryExplanation('last_good')}</p>}
    </section>
  );
}

export interface SettingsNoticesProps {
  /** The settings as loaded, or null while they load (or after a failed load): nothing is shown then. */
  view: SettingsView | null;
  /** The recovery panel's state: which action runs, the revert confirm step, and the last error. */
  recovery: RecoveryState;
  onCheckAgain(): void;
  onAskRevert(ask: boolean): void;
  onRecover(action: RecoverAction): void;
  onReload(): void;
}

/**
 * What the dialog says about settings.json, above the tabs so it stays in view on every tab: the recovery panel while the
 * file cannot be used, or the banner when it cannot be read. Nothing when the file is in use.
 */
export function SettingsNotices({ view, recovery, onCheckAgain, onAskRevert, onRecover, onReload }: SettingsNoticesProps) {
  if (view === null) {
    return null;
  }
  const recovering = recoveryKind(view.settings_state) !== 'ok';
  if (!recovering && view.load_error === null) {
    return null;
  }
  return (
    <div className="settings-notices">
      {recovering && (
        <RecoveryPanel state={view.settings_state} recovery={recovery} onAskRevert={onAskRevert} onRecover={onRecover} onReload={onReload} />
      )}
      {!recovering && view.load_error !== null && (
        <div className="settings-banner" role="alert">
          <span>{`settings.json could not be read: ${view.load_error}. Changes are disabled until it is fixed or removed.`}</span>
          <button type="button" onClick={onCheckAgain}>
            Check again
          </button>
        </div>
      )}
    </div>
  );
}

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
  /** The settings were loaded (on open, Retry or Check again): the caller reads the settings state again. */
  onViewLoaded?(): void;
}

/**
 * The AI settings dialog: a modal over the window. It loads the settings, and holds the state of the default model, the
 * context limit, the provider editor, the approval draft and the agent integration setting. The notices about settings.json stay above a tab strip
 * (Models, Approvals); every tab stays mounted, so a switch keeps the drafts. Focus moves in on open, stays in while open,
 * and goes back to where it was on close. Esc closes; a click on the backdrop closes. Unsaved changes are asked about
 * before they are dropped.
 */
export function SettingsDialog({ onClose, onChanged, onViewLoaded }: SettingsDialogProps) {
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
  const [recovery, setRecovery] = useState<RecoveryState>(NO_RECOVERY);
  // The Approvals section's edit, or null when it shows the saved settings. Its baseline is the view's approval settings.
  const [approvalDraft, setApprovalDraft] = useState<ApprovalDraft | null>(null);
  const [approvalSaving, setApprovalSaving] = useState(false);
  const [approvalError, setApprovalError] = useState<string | null>(null);
  // The Limits section's edit, or null when it shows the saved limits. Its baseline is the view's limits.
  const [limitsDraft, setLimitsDraft] = useState<LimitsDraft | null>(null);
  const [limitsSaving, setLimitsSaving] = useState(false);
  const [limitsError, setLimitsError] = useState<string | null>(null);
  // The agent integration setting is saved on each change (its own command), so it has its own busy and error state.
  const [integrationSave, setIntegrationSave] = useState<{ busy: boolean; error: string | null }>({ busy: false, error: null });
  // undefined while the built-in list loads; null when it could not be read.
  const [builtinDeny, setBuiltinDeny] = useState<string[] | null | undefined>(undefined);
  // The active tab. The dialog opens on the first one, and a closed dialog forgets it.
  const [tab, setTab] = useState(SETTINGS_TABS[0].id);

  const view = load.kind === 'ready' ? load.view : null;
  const dirty = editor !== null && isDirty(editor.draft, editor.saved);
  const approvalDirty = view !== null && approvalDraft !== null && isApprovalDirty(approvalDraft, view.settings.approval);
  const approvalShown = view === null ? null : (approvalDraft ?? draftFromApproval(view.settings.approval));
  const limitsDirtyNow = view !== null && limitsDraft !== null && isLimitsDirty(limitsDraft, view.settings.limits);
  const limitsShown = view === null ? null : (limitsDraft ?? draftFromLimits(view.settings.limits));

  // Load the settings each time the dialog opens or Retry is pressed. The presets are static, so a failure only hides them.
  useEffect(() => {
    let live = true;
    api.getSettings().then(
      (v) => {
        if (live) {
          setLoad({ kind: 'ready', view: v });
          setLimit((l) => ({ ...l, text: limitText(v) }));
          onViewLoaded?.();
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

  // The built-in deny list is fixed in the backend, so it is read once. A failure shows as "not available" in the section.
  useEffect(() => {
    let live = true;
    api.approvalBuiltinDeny().then(
      (list) => {
        if (live) {
          setBuiltinDeny(list);
        }
      },
      () => {
        if (live) {
          setBuiltinDeny(null);
        }
      },
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

  // Focus is lost when the focused control unmounts itself (Keep editing, Remove, Retry) or is disabled while busy. The
  // browser then puts it on <body> without a focusin, so the listener above never sees it, and Esc would do nothing. After
  // each render, a dialog with no usable focus takes it itself, so Esc and Tab work at once. The check waits for a timer:
  // a commit from a blur (Tab out of the limit field) renders while the browser is still moving focus, when activeElement
  // is <body>, and an immediate check would take the focus from the control Tab is moving to.
  useEffect(() => {
    const timer = window.setTimeout(() => {
      const active = document.activeElement;
      if (!active || active === document.body || !active.isConnected || active.matches(':disabled')) {
        dialogRef.current?.focus({ preventScroll: true });
      }
    }, 0);
    return () => window.clearTimeout(timer);
  });

  const applyView = (v: SettingsView) => setLoad({ kind: 'ready', view: v });

  // A save in flight: closing now would offer to discard a change that is being saved, and would drop its result. Esc, the
  // backdrop and Close are ignored until it settles.
  const saving = editor?.saving === true || approvalSaving || limitsSaving;

  const requestClose = () => {
    if (saving) {
      return;
    }
    if (dirty || approvalDirty || limitsDirtyNow) {
      setAskClose(true);
      return;
    }
    onClose();
  };

  // The question the close guard asks, naming what is unsaved. Two or more unsaved areas share one question.
  const unsavedCount = [dirty, approvalDirty, limitsDirtyNow].filter(Boolean).length;
  const discardQuestion =
    unsavedCount > 1
      ? 'Discard unsaved changes?'
      : dirty
        ? 'Discard unsaved provider changes?'
        : approvalDirty
          ? 'Discard unsaved approval changes?'
          : 'Discard unsaved limits changes?';

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
      } else if (recovery.confirming) {
        setRecovery((r) => ({ ...r, confirming: false }));
      } else {
        requestClose();
      }
      return;
    }
    if (e.key === 'Tab') {
      // Tab moves within the dialog: from the last control to the first, and back. Controls in a hidden tab panel, and the
      // inactive tabs (tabindex -1), are not on the Tab order, so they are not the first or last stop.
      const items = Array.from(dialogRef.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []).filter(
        (el) => el.closest('[hidden]') === null && el.getAttribute('tabindex') !== '-1',
      );
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

  // A commit in flight blocks the next one. The field is disabled while busy, and the blur that follows the focus moving off
  // it would otherwise send the same value again before the first call has come back. A ref, since state lags a render.
  const limitInFlight = useRef(false);

  const commitLimit = (text: string) => {
    if (view === null || limitInFlight.current) {
      return;
    }
    const plan = limitCommit(text, view.settings.global_context_limit);
    if (plan.kind === 'invalid') {
      setLimit((l) => ({ ...l, error: LIMIT_ERROR }));
      return;
    }
    if (plan.kind === 'unchanged') {
      setLimit((l) => ({ ...l, error: null }));
      return;
    }
    limitInFlight.current = true;
    setLimit((l) => ({ ...l, busy: true, error: null }));
    api.setGlobalContextLimit(plan.value).then(
      (v) => {
        limitInFlight.current = false;
        applyView(v);
        setLimit({ text: limitText(v), error: null, busy: false });
        onChanged();
      },
      (e: unknown) => {
        limitInFlight.current = false;
        setLimit((l) => ({ ...l, busy: false, error: errorText(e) }));
      },
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
    // A new provider's baseline is the draft it starts from, so it is clean until something is typed or added.
    setEditor(newEditor(null, draft, draft));
    setNotice(null);
  };

  const cancelEditor = () => {
    setEditor(null);
    setNotice(null);
  };

  // The save button is disabled while saving. This ref guard holds for any other call (a second click or key) that lands before
  // that render, since the state read by the handler is the one from the render the click came from.
  const saveInFlight = useRef(false);

  const save = () => {
    if (view === null || editor === null || editor.saving || saveInFlight.current) {
      return;
    }
    const input = buildSaveInput(editor.draft);
    const id = input.provider.id;
    saveInFlight.current = true;
    setAskClose(false);
    setEditor({ ...editor, saving: true, saveError: null });
    api.saveProvider(input).then(
      (v) => {
        saveInFlight.current = false;
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
      (e: unknown) => {
        saveInFlight.current = false;
        setEditor((cur) => (cur ? { ...cur, saving: false, saveError: errorText(e) } : cur));
      },
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

  /**
   * A recovery action: revert, use the last working copy, or reload the file. Refused while a provider has unsaved changes,
   * since the file underneath it changes. On success the returned view replaces the dialog's, and the status is read again.
   */
  const runRecovery = (action: RecoverAction | 'reload') => {
    if (view === null || recovery.busy !== null || !mayReplaceEditor()) {
      setRecovery((r) => ({ ...r, confirming: false }));
      return;
    }
    setRecovery({ busy: action, confirming: false, error: null });
    const call = action === 'reload' ? api.reloadSettings() : api.recoverSettings(action);
    call.then(
      (v) => {
        applyView(v);
        setEditor(null);
        setAskClose(false);
        setDefaults({ busy: false, error: null });
        setLimit({ text: limitText(v), error: null, busy: false });
        setApprovalDraft(null);
        setApprovalError(null);
        setLimitsDraft(null);
        setLimitsError(null);
        setRecovery(NO_RECOVERY);
        onChanged();
      },
      (e: unknown) => setRecovery((r) => ({ ...r, busy: null, error: errorText(e) })),
    );
  };

  /** Saves the agent integration setting as soon as it changes. A refused save shows its message; the box shows the saved value. */
  const setAgentRequests = (value: AgentRequests) => {
    if (view === null || integrationSave.busy) {
      return;
    }
    setIntegrationSave({ busy: true, error: null });
    api.setIntegrationSettings({ agent_requests: value }).then(
      (v) => {
        applyView(v);
        setIntegrationSave({ busy: false, error: null });
      },
      (e: unknown) => setIntegrationSave({ busy: false, error: errorText(e) }),
    );
  };

  /** Saves the Approvals draft. The backend checks the rules; a refused save shows its message and keeps the draft. */
  const saveApproval = () => {
    if (view === null || approvalDraft === null || approvalSaving) {
      return;
    }
    setApprovalSaving(true);
    setApprovalError(null);
    api.setApprovalSettings(approvalInput(approvalDraft)).then(
      (v) => {
        applyView(v);
        setApprovalDraft(null);
        setApprovalSaving(false);
        onChanged();
      },
      (e: unknown) => {
        setApprovalSaving(false);
        setApprovalError(errorText(e));
      },
    );
  };

  /** Saves the Limits draft. The backend checks the ranges; a refused save shows its message and keeps the draft. */
  const saveLimits = () => {
    if (view === null || limitsDraft === null || limitsSaving) {
      return;
    }
    const input = limitsInput(limitsDraft);
    if (input === null) {
      return;
    }
    setLimitsSaving(true);
    setLimitsError(null);
    api.setLimitsSettings(input).then(
      (v) => {
        applyView(v);
        setLimitsDraft(null);
        setLimitsSaving(false);
      },
      (e: unknown) => {
        setLimitsSaving(false);
        setLimitsError(errorText(e));
      },
    );
  };

  const locked = view !== null && view.load_error !== null;
  // The Approvals and Limits sections are read-only while settings.json is not in use (broken, last good, or a load error).
  const approvalLocked = locked || (view !== null && recoveryKind(view.settings_state) !== 'ok');
  const limitsLocked = approvalLocked;
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

  // The tabs whose draft is dirty: a dot on the tab, and the close guard's question.
  const dirtyTabs: string[] = [];
  if (dirty) {
    dirtyTabs.push('models');
  }
  if (approvalDirty) {
    dirtyTabs.push('approvals');
  }
  if (limitsDirtyNow) {
    dirtyTabs.push('limits');
  }

  const modelsPanel = (
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
  );

  const approvalsPanel =
    view !== null && approvalShown !== null ? (
      <ApprovalSection
        draft={approvalShown}
        dirty={approvalDirty}
        locked={approvalLocked}
        saving={approvalSaving}
        saveError={approvalError}
        builtinDeny={builtinDeny}
        integration={view.settings.integration.agent_requests}
        integrationBusy={integrationSave.busy}
        integrationError={integrationSave.error}
        onIntegrationChange={setAgentRequests}
        onChange={(draft) => {
          setApprovalDraft(draft);
          setApprovalError(null);
        }}
        onSave={saveApproval}
        onUndo={() => {
          setApprovalDraft(null);
          setApprovalError(null);
        }}
      />
    ) : null;

  const limitsPanel =
    view !== null && limitsShown !== null ? (
      <LimitsSection
        draft={limitsShown}
        dirty={limitsDirtyNow}
        locked={limitsLocked}
        saving={limitsSaving}
        saveError={limitsError}
        onChange={(draft) => {
          setLimitsDraft(draft);
          setLimitsError(null);
        }}
        onSave={saveLimits}
        onReset={() => {
          setLimitsDraft(defaultDraft());
          setLimitsError(null);
        }}
      />
    ) : null;

  const aboutPanel = <AboutSection />;

  return (
    <div
      className="modal-backdrop"
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
        <SettingsNotices
          view={view}
          recovery={recovery}
          onCheckAgain={() => {
            setLoad({ kind: 'loading' });
            setAttempt((a) => a + 1);
          }}
          onAskRevert={(ask) => setRecovery((r) => ({ ...r, confirming: ask, error: null }))}
          onRecover={runRecovery}
          onReload={() => runRecovery('reload')}
        />
        <SettingsTabs
          label="AI settings sections"
          tabs={SETTINGS_TABS}
          selected={tab}
          dirty={dirtyTabs}
          onSelect={setTab}
          panel={(id) =>
            id === 'approvals' ? approvalsPanel : id === 'limits' ? limitsPanel : id === 'about' ? aboutPanel : modelsPanel
          }
        />
        <footer className="settings-foot">
          {askClose ? (
            <>
              <span className="settings-warn">{discardQuestion}</span>
              <button type="button" className="settings-danger" onClick={discardAndClose}>
                Discard and close
              </button>
              <button type="button" onClick={() => setAskClose(false)}>
                Keep editing
              </button>
            </>
          ) : (
            <button type="button" disabled={saving} onClick={requestClose}>
              Close
            </button>
          )}
        </footer>
      </div>
    </div>
  );
}

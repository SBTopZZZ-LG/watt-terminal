// What the UI says while settings.json cannot be used: the banner, the model chip and the settings dialog's recovery panel.
// Pure, so the wording and the rules are tested without the DOM. The backend (core/settings) decides the state; this only
// names it. Reasons come from the file, so they go through safeText like the History text does.

import { safeText } from './historyView';
import type { LlmStatus, RecoverAction, SettingsStateView } from './types';

/** The UI's view of settings.json: in use, broken (nothing runs), or the last working copy running for this session. */
export type RecoveryKind = 'ok' | 'broken' | 'last_good';

/** The state's kind. A missing state (no app folders, so no settings service) is "ok": nothing to recover. */
export function recoveryKind(state: SettingsStateView | null | undefined): RecoveryKind {
  return state === null || state === undefined ? 'ok' : state.kind;
}

/** The reason the file cannot be used, as plain text. A fallback when the backend gave none. */
export function reasonText(state: SettingsStateView): string {
  return safeText(state.reason ?? '').trim() || 'the file could not be read';
}

/** The line at the top of the agent panel's chat view, or null when the settings are fine. */
export function bannerText(state: SettingsStateView | null | undefined): string | null {
  if (state === null || state === undefined || state.kind === 'ok') {
    return null;
  }
  const reason = reasonText(state);
  if (state.kind === 'broken') {
    return `Settings file is broken: ${reason}. The agent cannot send messages until it is fixed.`;
  }
  return `Running on the last working settings (settings.json is broken: ${reason}).`;
}

/** The model chip's label. In either broken state the chip says so, instead of the model or "mock agent". */
export function chipLabel(state: SettingsStateView | null | undefined, llm: LlmStatus): string {
  const kind = recoveryKind(state);
  if (kind === 'broken') {
    return 'settings error';
  }
  if (kind === 'last_good') {
    return 'last good';
  }
  if (!llm.enabled) {
    return 'mock agent';
  }
  return `model: ${llm.model ?? 'unknown'}${llm.variant ? `#${llm.variant}` : ''}`;
}

/** The model chip's hover text while the settings are broken; null otherwise, and the caller keeps its own. */
export function chipBrokenTitle(state: SettingsStateView | null | undefined): string | null {
  if (state === null || state === undefined || state.kind === 'ok') {
    return null;
  }
  const reason = reasonText(state);
  return state.kind === 'broken'
    ? `settings.json is broken: ${reason}`
    : `Running on the last working settings: settings.json is broken (${reason})`;
}

/** What the dialog says under the broken file's error, for each state. */
export function recoveryExplanation(kind: Exclude<RecoveryKind, 'ok'>): string {
  return kind === 'broken'
    ? 'Changes are disabled until the file is fixed, reverted or reloaded.'
    : 'settings.json stays as it is on disk; editing is disabled until you fix it and reload or restart.';
}

/** The confirm line for "Revert to last working settings". The broken file is kept, under a name with the time in it. */
export const REVERT_CONFIRM =
  'This keeps the broken file as settings.json.broken-<unix time> and restores the last working one. Revert?';

/** The busy label of each recovery action, shown on its button while it runs. */
export const RECOVERY_BUSY_LABEL: Record<RecoverAction | 'reload', string> = {
  revert: 'Reverting...',
  use_last_good: 'Switching...',
  reload: 'Reloading...',
};

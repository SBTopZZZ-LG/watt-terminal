// The Limits form: the draft the Limits section edits, its checks, and the settings it saves. Pure, so the rules are tested
// without the DOM. The backend (core/settings) checks the ranges again on save; these messages are for the inline checks.

import type { LimitsSettings } from './types';

/** The most a model-call cap can hold: the backend stores it as a u32. The UI has no lower-than-this upper bound. */
export const MAX_MODEL_CALLS = 4294967295;

/** The saved values before the user changes anything (Settings::default in core/settings). */
export const DEFAULT_LIMITS: LimitsSettings = {
  max_model_calls: 50,
  model_calls_unlimited: false,
  max_poll_calls: 60,
  max_commands_per_turn: 30,
  max_wait_minutes: 30,
};

/** Above this, the model-call field shows the warning (a high cap lets a runaway turn cost a lot). */
export const HIGH_MODEL_CALLS = 200;
export const HIGH_LIMIT_WARNING = 'A high limit lets a runaway turn cost a lot.';

/** The text of each field, as typed. Numbers stay text until they are checked. */
export interface LimitsDraft {
  modelCalls: string;
  unlimited: boolean;
  pollCalls: string;
  commands: string;
  waitMinutes: string;
}

export type LimitField = 'modelCalls' | 'pollCalls' | 'commands' | 'waitMinutes';

/** The inclusive range of each numeric field. The model-call cap has no practical upper bound: the u32 is the bound. */
export const LIMIT_RANGES: Record<LimitField, { min: number; max: number }> = {
  modelCalls: { min: 10, max: MAX_MODEL_CALLS },
  pollCalls: { min: 10, max: 500 },
  commands: { min: 1, max: 500 },
  waitMinutes: { min: 1, max: 720 },
};

export function draftFromLimits(limits: LimitsSettings): LimitsDraft {
  return {
    modelCalls: String(limits.max_model_calls),
    unlimited: limits.model_calls_unlimited,
    pollCalls: String(limits.max_poll_calls),
    commands: String(limits.max_commands_per_turn),
    waitMinutes: String(limits.max_wait_minutes),
  };
}

/** The draft the Reset to defaults button puts in the fields. It is not saved until Save limits. */
export function defaultDraft(): LimitsDraft {
  return draftFromLimits(DEFAULT_LIMITS);
}

/** A whole number as typed (spaces around it allowed), or null. "2.5", "1e3", "-4" and "" are not whole numbers. */
export function parseWhole(text: string): number | null {
  const trimmed = text.trim();
  if (!/^\d+$/.test(trimmed)) {
    return null;
  }
  const n = Number(trimmed);
  return Number.isSafeInteger(n) ? n : null;
}

/** The message under a field that does not hold a whole number in its range, or null when it does. */
export function fieldProblem(field: LimitField, text: string): string | null {
  const range = LIMIT_RANGES[field];
  const n = parseWhole(text);
  if (n !== null && n >= range.min && n <= range.max) {
    return null;
  }
  return `Enter a whole number from ${range.min} to ${range.max}.`;
}

/** The problems in each field, keyed by field. Empty when the draft can be saved. */
export function limitProblems(draft: LimitsDraft): Partial<Record<LimitField, string>> {
  const out: Partial<Record<LimitField, string>> = {};
  for (const field of Object.keys(LIMIT_RANGES) as LimitField[]) {
    const problem = fieldProblem(field, draft[field]);
    if (problem !== null) {
      out[field] = problem;
    }
  }
  return out;
}

/** The settings a draft saves, or null while a field is out of range. The model-call number is checked even when unlimited. */
export function limitsInput(draft: LimitsDraft): LimitsSettings | null {
  if (Object.keys(limitProblems(draft)).length > 0) {
    return null;
  }
  return {
    max_model_calls: parseWhole(draft.modelCalls) ?? 0,
    model_calls_unlimited: draft.unlimited,
    max_poll_calls: parseWhole(draft.pollCalls) ?? 0,
    max_commands_per_turn: parseWhole(draft.commands) ?? 0,
    max_wait_minutes: parseWhole(draft.waitMinutes) ?? 0,
  };
}

/** Whether the draft differs from the saved limits. A field that is not a number counts as a change. */
export function isLimitsDirty(draft: LimitsDraft, saved: LimitsSettings): boolean {
  const input = limitsInput(draft);
  if (input === null) {
    return true;
  }
  return (
    input.max_model_calls !== saved.max_model_calls ||
    input.model_calls_unlimited !== saved.model_calls_unlimited ||
    input.max_poll_calls !== saved.max_poll_calls ||
    input.max_commands_per_turn !== saved.max_commands_per_turn ||
    input.max_wait_minutes !== saved.max_wait_minutes
  );
}

/** Whether the model-call number is above the warning level. The warning is shown only while the cap is in effect. */
export function isHighModelCalls(text: string): boolean {
  const n = parseWhole(text);
  return n !== null && n > HIGH_MODEL_CALLS;
}

/** The draft once unlimited is confirmed on. The model-call field is disabled while unlimited is on, so a number that is not valid is put back to the default there (it could not be fixed otherwise). A valid number is kept. */
export function withUnlimitedOn(draft: LimitsDraft): LimitsDraft {
  const modelCalls = fieldProblem('modelCalls', draft.modelCalls) === null ? draft.modelCalls : String(DEFAULT_LIMITS.max_model_calls);
  return { ...draft, modelCalls, unlimited: true };
}

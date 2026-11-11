import { describe, expect, it } from 'vitest';
import {
  DEFAULT_LIMITS,
  HIGH_LIMIT_WARNING,
  LIMIT_RANGES,
  MAX_MODEL_CALLS,
  defaultDraft,
  draftFromLimits,
  fieldProblem,
  isHighModelCalls,
  isLimitsDirty,
  limitProblems,
  limitsInput,
  parseWhole,
  withUnlimitedOn,
} from './limitsForm';
import type { LimitsDraft } from './limitsForm';
import type { LimitsSettings } from './types';

const SAVED: LimitsSettings = { max_model_calls: 75, model_calls_unlimited: false, max_poll_calls: 120, max_commands_per_turn: 30, max_wait_minutes: 30 };

describe('defaults', () => {
  it('match the backend defaults', () => {
    expect(DEFAULT_LIMITS).toEqual({ max_model_calls: 50, model_calls_unlimited: false, max_poll_calls: 60, max_commands_per_turn: 30, max_wait_minutes: 30 });
    expect(defaultDraft()).toEqual({ modelCalls: '50', unlimited: false, pollCalls: '60', commands: '30', waitMinutes: '30' });
  });

  it('the model-call field is bounded by the u32 the backend stores, and nothing lower than 10', () => {
    expect(LIMIT_RANGES.modelCalls).toEqual({ min: 10, max: 4294967295 });
    expect(MAX_MODEL_CALLS).toBe(4294967295);
  });
});

describe('parseWhole', () => {
  it('reads whole numbers, with spaces around them allowed', () => {
    expect(parseWhole('50')).toBe(50);
    expect(parseWhole(' 7 ')).toBe(7);
    expect(parseWhole('007')).toBe(7);
  });

  it('rejects blanks, decimals, signs, exponents and words', () => {
    for (const text of ['', '   ', '2.5', '-4', '+4', '1e3', '12px', 'abc', '1 2']) {
      expect(parseWhole(text), text).toBeNull();
    }
  });

  it('rejects numbers that are not safe integers', () => {
    expect(parseWhole('9007199254740993')).toBeNull();
  });
});

describe('fieldProblem', () => {
  it('accepts the ends of each range', () => {
    expect(fieldProblem('modelCalls', '10')).toBeNull();
    expect(fieldProblem('modelCalls', String(MAX_MODEL_CALLS))).toBeNull();
    expect(fieldProblem('pollCalls', '10')).toBeNull();
    expect(fieldProblem('pollCalls', '500')).toBeNull();
    expect(fieldProblem('commands', '1')).toBeNull();
    expect(fieldProblem('commands', '500')).toBeNull();
    expect(fieldProblem('waitMinutes', '1')).toBeNull();
    expect(fieldProblem('waitMinutes', '720')).toBeNull();
  });

  it('names the range just outside each end', () => {
    expect(fieldProblem('modelCalls', '9')).toBe('Enter a whole number from 10 to 4294967295.');
    expect(fieldProblem('modelCalls', '4294967296')).toBe('Enter a whole number from 10 to 4294967295.');
    expect(fieldProblem('pollCalls', '9')).toBe('Enter a whole number from 10 to 500.');
    expect(fieldProblem('pollCalls', '501')).toBe('Enter a whole number from 10 to 500.');
    expect(fieldProblem('commands', '0')).toBe('Enter a whole number from 1 to 500.');
    expect(fieldProblem('waitMinutes', '721')).toBe('Enter a whole number from 1 to 720.');
  });

  it('refuses text that is not a whole number', () => {
    expect(fieldProblem('pollCalls', '')).not.toBeNull();
    expect(fieldProblem('pollCalls', '60.5')).not.toBeNull();
    expect(fieldProblem('waitMinutes', 'x')).not.toBeNull();
  });
});

describe('limitProblems and limitsInput', () => {
  it('a draft from the saved settings has no problems and saves them back', () => {
    const draft = draftFromLimits(SAVED);
    expect(limitProblems(draft)).toEqual({});
    expect(limitsInput(draft)).toEqual(SAVED);
  });

  it('lists each bad field, and saves nothing', () => {
    const draft: LimitsDraft = { modelCalls: '9', unlimited: false, pollCalls: '501', commands: '1', waitMinutes: '' };
    expect(Object.keys(limitProblems(draft)).sort()).toEqual(['modelCalls', 'pollCalls', 'waitMinutes']);
    expect(limitsInput(draft)).toBeNull();
  });

  it('checks the model-call number even when unlimited is on, so a bad number still blocks the save', () => {
    const draft: LimitsDraft = { modelCalls: '3', unlimited: true, pollCalls: '60', commands: '30', waitMinutes: '30' };
    expect(limitProblems(draft)).toEqual({ modelCalls: 'Enter a whole number from 10 to 4294967295.' });
    expect(limitsInput(draft)).toBeNull();
  });

  it('keeps the model-call number when unlimited is on, and saves the flag', () => {
    const draft: LimitsDraft = { modelCalls: '250', unlimited: true, pollCalls: '60', commands: '30', waitMinutes: '30' };
    expect(limitsInput(draft)).toEqual({ max_model_calls: 250, model_calls_unlimited: true, max_poll_calls: 60, max_commands_per_turn: 30, max_wait_minutes: 30 });
  });

  it('accepts a model-call number far above 200 (the warning is not an error)', () => {
    expect(limitsInput({ ...defaultDraft(), modelCalls: '100000' })?.max_model_calls).toBe(100000);
  });
});

describe('isLimitsDirty', () => {
  it('is clean when the draft matches the saved limits', () => {
    expect(isLimitsDirty(draftFromLimits(SAVED), SAVED)).toBe(false);
  });

  it('is dirty when any field changes', () => {
    const base = draftFromLimits(SAVED);
    expect(isLimitsDirty({ ...base, modelCalls: '76' }, SAVED)).toBe(true);
    expect(isLimitsDirty({ ...base, unlimited: true }, SAVED)).toBe(true);
    expect(isLimitsDirty({ ...base, pollCalls: '121' }, SAVED)).toBe(true);
    expect(isLimitsDirty({ ...base, commands: '31' }, SAVED)).toBe(true);
    expect(isLimitsDirty({ ...base, waitMinutes: '29' }, SAVED)).toBe(true);
  });

  it('ignores spacing and leading zeros in the typed text', () => {
    expect(isLimitsDirty({ ...draftFromLimits(SAVED), modelCalls: ' 075 ' }, SAVED)).toBe(false);
  });

  it('a field that is not a number is a change, and so is a draft that cannot be saved', () => {
    expect(isLimitsDirty({ ...draftFromLimits(SAVED), pollCalls: '' }, SAVED)).toBe(true);
  });
});

describe('withUnlimitedOn', () => {
  it('turns unlimited on and keeps a valid model-call number', () => {
    const draft: LimitsDraft = { modelCalls: '250', unlimited: false, pollCalls: '120', commands: '31', waitMinutes: '29' };
    expect(withUnlimitedOn(draft)).toEqual({ modelCalls: '250', unlimited: true, pollCalls: '120', commands: '31', waitMinutes: '29' });
  });

  it('puts the default back in place of a model-call number that is not valid, so the save is not blocked', () => {
    for (const modelCalls of ['9', '2.5', '', 'abc', '4294967296']) {
      const draft: LimitsDraft = { modelCalls, unlimited: false, pollCalls: '60', commands: '30', waitMinutes: '30' };
      const next = withUnlimitedOn(draft);
      expect(next.modelCalls, modelCalls).toBe('50');
      expect(next.unlimited, modelCalls).toBe(true);
      expect(limitsInput(next), modelCalls).toEqual({ ...DEFAULT_LIMITS, max_model_calls: 50, model_calls_unlimited: true });
    }
  });

  it('leaves the other fields as they were, even when they are not valid', () => {
    const draft: LimitsDraft = { modelCalls: '', unlimited: false, pollCalls: '501', commands: '', waitMinutes: '0' };
    expect(withUnlimitedOn(draft)).toEqual({ modelCalls: '50', unlimited: true, pollCalls: '501', commands: '', waitMinutes: '0' });
  });

  it('does not change the draft it is given', () => {
    const draft: LimitsDraft = { modelCalls: '9', unlimited: false, pollCalls: '60', commands: '30', waitMinutes: '30' };
    withUnlimitedOn(draft);
    expect(draft.modelCalls).toBe('9');
    expect(draft.unlimited).toBe(false);
  });
});

describe('isHighModelCalls', () => {
  it('warns above 200, and not at 200', () => {
    expect(isHighModelCalls('200')).toBe(false);
    expect(isHighModelCalls('201')).toBe(true);
    expect(isHighModelCalls('5000')).toBe(true);
  });

  it('does not warn for text that is not a number', () => {
    expect(isHighModelCalls('')).toBe(false);
    expect(isHighModelCalls('abc')).toBe(false);
  });

  it('the warning text is the one the user asked for', () => {
    expect(HIGH_LIMIT_WARNING).toBe('A high limit lets a runaway turn cost a lot.');
  });
});

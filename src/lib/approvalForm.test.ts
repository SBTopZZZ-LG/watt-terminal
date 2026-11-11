import { describe, expect, it } from 'vitest';
import { MAX_RULES, approvalInput, draftFromApproval, isApprovalDirty, overRuleLimit, parseRules, ruleCountText } from './approvalForm';
import type { ApprovalSettings } from './types';

const SAVED: ApprovalSettings = { mode: 'regex', allow: ['git status', 'ls .*'], deny: ['rm .*'] };

describe('parseRules', () => {
  it('gives one rule per line, in order', () => {
    expect(parseRules('git status\nls .*')).toEqual(['git status', 'ls .*']);
  });

  it('drops blank and whitespace-only lines', () => {
    expect(parseRules('\n  \ngit status\n\t\n')).toEqual(['git status']);
  });

  it('trims each rule, so a Windows line break and stray spaces do not reach the rule', () => {
    expect(parseRules('git status \r\n  ls  \r\n')).toEqual(['git status', 'ls']);
  });

  it('is empty for an empty box', () => {
    expect(parseRules('')).toEqual([]);
    expect(parseRules('   \n  ')).toEqual([]);
  });
});

describe('draftFromApproval and approvalInput', () => {
  it('round-trips the saved settings', () => {
    expect(approvalInput(draftFromApproval(SAVED))).toEqual(SAVED);
  });

  it('puts each list in its box, one rule per line', () => {
    expect(draftFromApproval(SAVED)).toEqual({ mode: 'regex', allow: 'git status\nls .*', deny: 'rm .*' });
  });

  it('a manual draft with no rules saves no rules', () => {
    expect(approvalInput({ mode: 'manual', allow: '', deny: '' })).toEqual({ mode: 'manual', allow: [], deny: [] });
  });
});

describe('isApprovalDirty', () => {
  it('is clean for the saved settings', () => {
    expect(isApprovalDirty(draftFromApproval(SAVED), SAVED)).toBe(false);
  });

  it('is dirty when the mode differs', () => {
    expect(isApprovalDirty({ ...draftFromApproval(SAVED), mode: 'manual' }, SAVED)).toBe(true);
  });

  it('is dirty when a rule is added, removed or changed', () => {
    const base = draftFromApproval(SAVED);
    expect(isApprovalDirty({ ...base, allow: `${base.allow}\nnpm test` }, SAVED)).toBe(true);
    expect(isApprovalDirty({ ...base, deny: '' }, SAVED)).toBe(true);
    expect(isApprovalDirty({ ...base, allow: 'git status\nls -la' }, SAVED)).toBe(true);
  });

  it('is clean when only blank lines or the spacing around a rule changed', () => {
    const base = draftFromApproval(SAVED);
    expect(isApprovalDirty({ ...base, allow: '\n git status  \n\nls .*\n' }, SAVED)).toBe(false);
  });

  it('is dirty when the same rules are in a different order, since order is kept', () => {
    expect(isApprovalDirty({ ...draftFromApproval(SAVED), allow: 'ls .*\ngit status' }, SAVED)).toBe(true);
  });
});

describe('ruleCountText and overRuleLimit', () => {
  it('counts the rules and shows the limit', () => {
    expect(ruleCountText('')).toBe(`0 of ${MAX_RULES} rules`);
    expect(ruleCountText('git status\n\nls')).toBe(`2 of ${MAX_RULES} rules`);
  });

  it('keeps MAX_RULES at the backend limit', () => {
    expect(MAX_RULES).toBe(50);
  });

  it('flags more rules than the limit, and not the limit itself', () => {
    const at = Array.from({ length: MAX_RULES }, (_, i) => `cmd${i}`).join('\n');
    expect(overRuleLimit(at)).toBe(false);
    expect(overRuleLimit(`${at}\nextra`)).toBe(true);
  });
});

// The approval settings form: the draft the Approvals section edits, and the settings it saves. Pure, so the rules for
// parsing and comparing are tested without the DOM. The backend (core/approval) checks the rules again on save.

import type { ApprovalMode, ApprovalSettings, Nested } from './types';

/** Rules one list may hold (MAX_RULES in core/approval.rs). */
export const MAX_RULES = 50;

/** The draft: the mode, each list as the text in its box (one rule per line), and how nested shells are approved. */
export interface ApprovalDraft {
  mode: ApprovalMode;
  allow: string;
  deny: string;
  nested: Nested;
}

/** The rules in a box: one per line, each trimmed, blank lines dropped. */
export function parseRules(text: string): string[] {
  return text
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line !== '');
}

export function draftFromApproval(approval: ApprovalSettings): ApprovalDraft {
  return { mode: approval.mode, allow: approval.allow.join('\n'), deny: approval.deny.join('\n'), nested: approval.nested };
}

/** The settings a draft saves. */
export function approvalInput(draft: ApprovalDraft): ApprovalSettings {
  return { mode: draft.mode, allow: parseRules(draft.allow), deny: parseRules(draft.deny), nested: draft.nested };
}

/** Whether the draft differs from the saved settings. Blank lines and spacing around a rule do not count. */
export function isApprovalDirty(draft: ApprovalDraft, saved: ApprovalSettings): boolean {
  const input = approvalInput(draft);
  return (
    input.mode !== saved.mode ||
    input.nested !== saved.nested ||
    !sameRules(input.allow, saved.allow) ||
    !sameRules(input.deny, saved.deny)
  );
}

function sameRules(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((rule, i) => rule === b[i]);
}

/** The count under a rules box: "2 of 50 rules". */
export function ruleCountText(text: string): string {
  return `${parseRules(text).length} of ${MAX_RULES} rules`;
}

/** Whether a rules box holds more rules than a list may. The save is refused then, with the backend's message. */
export function overRuleLimit(text: string): boolean {
  return parseRules(text).length > MAX_RULES;
}

// What the UI says about approvals: why a command asked, why it ran without asking, and what the saved rules would do with
// a command. Pure, so the wording is tested without the DOM. The codes are core/approval's (see ApprovalReasonCode in
// types.ts). A code this build does not know gets a generic line, never the raw code.

import type { ApprovalPreview, ApprovalReasonCode, ToolDecision } from './types';

/** The reasons a call asks. The approval card and the tool row say these. */
export type AskReasonCode = Exclude<ApprovalReasonCode, 'approve_all' | 'rule' | 'interrupt' | 'latched'>;

/**
 * Automatic approvals one turn may use before the rules stop running commands by themselves. Mirrors the Rust
 * `MAX_AUTO_PER_TURN` in `src-tauri/src/core/approval.rs`: both must change together.
 */
export const AUTO_PER_TURN = 8;

/**
 * Why a call asks, as a clause: the card says "Asked because <clause>", and the try-a-command line says "Would ask: <clause>".
 */
const ASK_CLAUSE: Record<Exclude<AskReasonCode, 'unintegrated_shell'>, string> = {
  manual_mode: 'manual approval is on, so every command waits for you.',
  send_keys: 'approving lets the agent keep typing into that command until you type in the terminal.',
  gate_chars: 'the command has a character outside letters, digits, spaces and _-./:=@+, so your rules do not apply to it.',
  gate_path: 'the command does not start with a plain command name (a path, a VAR=value prefix or a leading -), so your rules do not apply to it.',
  gate_denylist: 'its first word is on the built-in list of commands that always need approval.',
  deny_rule: 'a deny rule matches this command.',
  no_rule: 'no allow rule matches this command.',
  auto_cap: `this turn has already used its ${AUTO_PER_TURN} automatic approvals.`,
  nested_level: 'this is a nested shell, so every command in it needs your approval.',
  approve_all_suspended: 'approve-all is off in this shell, because it was turned on in another shell.',
};

/** Reasons that are a whole sentence on their own, so the card shows them as they are (no "Asked because"). */
const ASK_SENTENCE: Record<'unintegrated_shell', string> = {
  unintegrated_shell: 'Typed into a shell without integration: your rules cannot check this.',
};

/** Why a call ran without asking, as a sentence. */
const RAN_TEXT: Record<'approve_all' | 'rule' | 'interrupt' | 'latched', string> = {
  approve_all: 'Ran without asking: approve-all is on for this session.',
  rule: 'Ran without asking: an allow rule matches this command.',
  interrupt: 'Ran without asking: Ctrl-C, Ctrl-D or Esc into the command the agent started.',
  latched: "Ran without asking: you approved this command's first keystrokes, so its later ones run too until you type.",
};

const GENERIC_ASK = 'Asked for your approval.';
const GENERIC_RAN = 'Ran without asking.';

function own<T>(table: Record<string, T>, key: string): T | undefined {
  return Object.prototype.hasOwnProperty.call(table, key) ? table[key] : undefined;
}

/** The approval card's line: why it asks. Null when there is no reason. */
export function askedBecause(code: ApprovalReasonCode | null | undefined): string | null {
  if (code === null || code === undefined) {
    return null;
  }
  const sentence = own(ASK_SENTENCE, code);
  if (sentence !== undefined) {
    return sentence;
  }
  const clause = own(ASK_CLAUSE, code);
  return clause === undefined ? GENERIC_ASK : `Asked because ${clause}`;
}

/** Why a call ran without asking, as a sentence. Falls back to a generic line for a code this build does not know. */
export function ranBecause(code: ApprovalReasonCode): string {
  return own(RAN_TEXT, code) ?? GENERIC_RAN;
}

/** The tool row's decision label. An automatic run shows its reason ("auto · rule"); the other decisions show their name. */
export function decisionLabel(decision: ToolDecision, reason: ApprovalReasonCode | null): string {
  return decision === 'auto' && reason !== null ? `auto · ${reason}` : decision;
}

/**
 * The tool row's tooltip: why it ran without asking (auto), or why it asked (manual, denied). Null for the other decisions
 * and when there is no reason.
 */
export function decisionTooltip(decision: ToolDecision, reason: ApprovalReasonCode | null): string | null {
  if (reason === null) {
    return null;
  }
  if (decision === 'auto') {
    return ranBecause(reason);
  }
  if (decision === 'manual' || decision === 'denied') {
    return askedBecause(reason);
  }
  return null;
}

/** The result line of "Try a command": what the saved settings would do with the command. */
export function previewText(preview: ApprovalPreview): string {
  if (preview.verdict === 'auto') {
    return `Would run without asking (${preview.reason})`;
  }
  const clause = own(ASK_CLAUSE, preview.reason);
  return `Would ask: ${clause ?? 'the saved settings ask for this command.'}`;
}

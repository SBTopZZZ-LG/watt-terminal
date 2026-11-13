// The agent's clarification questions (`ask_user`, src-tauri/src/core/chat/questions.rs): parsing the call's
// arguments and the stored result, the card's per-question drafts, and the summary of a resolved card. Pure, so the
// rules are tested without the DOM. Answers never touch the shell: they go back through `answer_questions` only.

import type { QuestionAnswerInput, QuestionResult, QuestionSpec, QuestionStatus } from './types';

/** The tool name of the clarification tool. Its rows are not shown in the transcript: the card is the view of it. */
export const ASK_USER_TOOL = 'ask_user';

/** The card's draft answer to one question: the chosen option indices, the free text, or skipped. */
export interface QuestionDraft {
  selected: number[];
  other: string;
  skipped: boolean;
}

/** A fresh draft for each question: nothing chosen, nothing typed, not skipped. */
export function newDrafts(count: number): QuestionDraft[] {
  return Array.from({ length: count }, () => ({ selected: [], other: '', skipped: false }));
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function nonEmptyText(value: unknown): string | null {
  if (typeof value !== 'string') {
    return null;
  }
  const text = value.trim();
  return text === '' ? null : text;
}

/**
 * The questions of an `ask_user` call's arguments (`{questions: [...]}`). The model sends no ids, so they are
 * assigned in order (`q1` and up), as the backend does. Lenient: the backend validates strictly, the card only needs
 * the shape. Null when the shape is not questions (then the history view falls back to a generic tool row).
 */
export function parseQuestions(args: unknown): QuestionSpec[] | null {
  if (!isRecord(args) || !Array.isArray(args.questions) || args.questions.length === 0) {
    return null;
  }
  const out: QuestionSpec[] = [];
  for (let i = 0; i < args.questions.length; i += 1) {
    const raw = args.questions[i];
    if (!isRecord(raw)) {
      return null;
    }
    const question = nonEmptyText(raw.question);
    const header = nonEmptyText(raw.header);
    if (question === null || header === null || !Array.isArray(raw.options) || raw.options.length === 0) {
      return null;
    }
    const options: QuestionSpec['options'] = [];
    for (const item of raw.options) {
      if (!isRecord(item)) {
        return null;
      }
      const label = nonEmptyText(item.label);
      if (label === null) {
        return null;
      }
      const description = typeof item.description === 'string' ? item.description.trim() : '';
      options.push({ label, description, recommended: item.recommended === true });
    }
    out.push({
      id: `q${i + 1}`,
      question,
      header,
      options,
      multiple: raw.multiple === true,
    });
  }
  return out;
}

const STATUSES: readonly QuestionStatus[] = ['answered', 'skipped', 'timed_out', 'cancelled'];

/**
 * The stored result of an `ask_user` call. Accepts the parsed object or its compact JSON string (that is what the
 * history keeps). Null when it is not a question result (then the history view falls back to a generic tool row).
 */
export function parseResult(value: unknown): QuestionResult | null {
  let parsed: unknown = value;
  if (typeof parsed === 'string') {
    try {
      parsed = JSON.parse(parsed) as unknown;
    } catch {
      return null;
    }
  }
  if (!isRecord(parsed) || !Array.isArray(parsed.answers) || parsed.answers.length === 0) {
    return null;
  }
  const answers: QuestionResult['answers'] = [];
  for (const raw of parsed.answers) {
    if (!isRecord(raw)) {
      return null;
    }
    const questionId = nonEmptyText(raw.question_id);
    const question = nonEmptyText(raw.question);
    const header = nonEmptyText(raw.header);
    const status: unknown = raw.status;
    if (questionId === null || question === null || header === null || !STATUSES.includes(status as QuestionStatus)) {
      return null;
    }
    const entry: QuestionResult['answers'][number] = { question_id: questionId, question, header, status: status as QuestionStatus };
    if (Array.isArray(raw.selected)) {
      const selected = raw.selected.filter((s): s is string => typeof s === 'string');
      if (selected.length > 0) {
        entry.selected = selected;
      }
    }
    if (typeof raw.other === 'string' && raw.other.trim() !== '') {
      entry.other = raw.other.trim();
    }
    answers.push(entry);
  }
  return { answers };
}

/**
 * Choosing an option. A single-select replaces the choice and clears the free text; a multiple-select toggles the
 * option and keeps the text. Either way the question is no longer skipped.
 */
export function pickOption(drafts: QuestionDraft[], qi: number, oi: number, multiple: boolean): QuestionDraft[] {
  return drafts.map((draft, i) => {
    if (i !== qi) {
      return draft;
    }
    if (!multiple) {
      return { selected: [oi], other: '', skipped: false };
    }
    const selected = draft.selected.includes(oi) ? draft.selected.filter((o) => o !== oi) : [...draft.selected, oi];
    return { ...draft, selected, skipped: false };
  });
}

/**
 * Typing the free text. On a single-select it clears the chosen option (the text is the answer instead); a
 * multiple-select keeps the chosen options. Either way the question is no longer skipped.
 */
export function setOther(drafts: QuestionDraft[], qi: number, text: string, multiple: boolean): QuestionDraft[] {
  return drafts.map((draft, i) => {
    if (i !== qi) {
      return draft;
    }
    return multiple ? { ...draft, other: text, skipped: false } : { selected: [], other: text, skipped: false };
  });
}

/** Skipping a question, or unskipping it. The draft's choice and text are kept, so unskipping restores them. */
export function toggleSkip(drafts: QuestionDraft[], qi: number): QuestionDraft[] {
  return drafts.map((draft, i) => (i !== qi ? draft : { ...draft, skipped: !draft.skipped }));
}

/** Whether one draft is settled: skipped, or answered with a choice or free text. Only settled questions advance. */
export function settled(draft: QuestionDraft): boolean {
  return draft.skipped || draft.selected.length > 0 || draft.other.trim() !== '';
}

/** Whether every question is settled: skipped, or answered with a choice or free text. Only then is Submit enabled. */
export function isComplete(drafts: QuestionDraft[]): boolean {
  return drafts.every(settled);
}

/** The index of the first unsettled draft, or the draft count when every question is settled. */
export function firstUnsettled(drafts: QuestionDraft[]): number {
  const at = drafts.findIndex((draft) => !settled(draft));
  return at < 0 ? drafts.length : at;
}

/**
 * What the Enter key does on the stepped card, by where the focus is. Single-select picks the focused option and
 * advances (or submits on the last question when complete); multi-select only toggles, never advances; Other keeps
 * the text and advances, or does nothing when empty; buttons and anything else keep their native behaviour.
 */
export type EnterTarget = { kind: 'option'; multiple: boolean } | { kind: 'other'; empty: boolean } | { kind: 'control' };

export type EnterIntent = 'select' | 'toggle' | 'keep' | 'none' | 'native';

export function enterIntent(target: EnterTarget): EnterIntent {
  if (target.kind === 'option') {
    return target.multiple ? 'toggle' : 'select';
  }
  if (target.kind === 'other') {
    return target.empty ? 'none' : 'keep';
  }
  return 'native';
}

/**
 * The result of questions that can no longer be answered (a saved call with no result): every question `cancelled`,
 * carrying its text. Rendered as a closed card, never a generic tool row.
 */
export function closedResult(questions: QuestionSpec[]): QuestionResult {
  return {
    answers: questions.map((question) => ({
      question_id: question.id,
      question: question.question,
      header: question.header,
      status: 'cancelled' as const,
    })),
  };
}

/**
 * Drafts rebuilt from sent answers, for retrying a failed send: a skipped answer stays skipped (keeping no choice or
 * text of its own), any other answer restores its choice and free text. A question with no answer starts fresh.
 */
export function draftsFromAnswers(questions: QuestionSpec[], answers: QuestionAnswerInput[]): QuestionDraft[] {
  const byId = new Map(answers.map((answer) => [answer.question_id, answer]));
  return questions.map((question) => {
    const answer = byId.get(question.id);
    if (answer === undefined || answer.skipped) {
      return { selected: [], other: '', skipped: answer !== undefined };
    }
    return { selected: [...answer.selected], other: answer.other ?? '', skipped: false };
  });
}

/**
 * The answers for `answer_questions`: one per question, in order, with the chosen indices in question order. A
 * skipped question carries nothing else; `other` is left out when absent. An unsettled draft cannot happen (the UI
 * only submits when complete), and reads as skipped.
 */
export function payload(questions: QuestionSpec[], drafts: QuestionDraft[]): QuestionAnswerInput[] {
  return questions.map((question, i) => {
    const draft = drafts[i] ?? { selected: [], other: '', skipped: false };
    const other = draft.other.trim();
    if (draft.skipped || (draft.selected.length === 0 && other === '')) {
      return { question_id: question.id, selected: [], skipped: true };
    }
    const selected = [...draft.selected].sort((a, b) => a - b);
    return { question_id: question.id, selected, ...(other === '' ? {} : { other }), skipped: false };
  });
}

export interface QuestionSummary {
  header: string;
  status: QuestionStatus;
  lines: string[];
}

/**
 * A resolved card's summary, grouped by question: the option labels the user chose, the free text, or why there is
 * nothing (skipped, timed out, cancelled).
 */
export function summary(result: QuestionResult): QuestionSummary[] {
  return result.answers.map((answer) => {
    if (answer.status === 'answered') {
      const lines = [...(answer.selected ?? [])];
      if (answer.other !== undefined) {
        lines.push(`Other: ${answer.other}`);
      }
      return { header: answer.header, status: answer.status, lines: lines.length > 0 ? lines : ['Answered'] };
    }
    if (answer.status === 'skipped') {
      return { header: answer.header, status: answer.status, lines: ['Skipped'] };
    }
    if (answer.status === 'timed_out') {
      return { header: answer.header, status: answer.status, lines: ['No answer: the question timed out'] };
    }
    return { header: answer.header, status: answer.status, lines: ['No answer: the question was cancelled'] };
  });
}

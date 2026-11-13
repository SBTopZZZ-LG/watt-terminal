import { describe, expect, it } from 'vitest';
import { ASK_USER_TOOL, closedResult, draftsFromAnswers, enterIntent, firstUnsettled, isComplete, newDrafts, parseQuestions, parseResult, payload, pickOption, setOther, settled, summary, toggleSkip } from './questions';
import type { QuestionSpec } from './types';

const ARGS = {
  questions: [
    {
      question: 'Which capa?',
      header: 'Capa',
      options: [
        { label: 'a', description: 'first', recommended: true },
        { label: 'b', description: 'second' },
      ],
    },
    {
      question: 'Which size?',
      header: 'Size',
      multiple: true,
      options: [{ label: 's' }, { label: 'm' }, { label: 'l', description: '' }],
    },
  ],
};

function specs(): QuestionSpec[] {
  const parsed = parseQuestions(ARGS);
  if (parsed === null) {
    throw new Error('the fixture must parse');
  }
  return parsed;
}

describe('parseQuestions', () => {
  it('names the tool', () => {
    expect(ASK_USER_TOOL).toBe('ask_user');
  });

  it('assigns ids in order and fills the defaults', () => {
    const questions = specs();
    expect(questions.map((q) => q.id)).toEqual(['q1', 'q2']);
    expect(questions[0]).toMatchObject({ question: 'Which capa?', header: 'Capa', multiple: false });
    expect(questions[0].options).toEqual([
      { label: 'a', description: 'first', recommended: true },
      { label: 'b', description: 'second', recommended: false },
    ]);
    expect(questions[1]).toMatchObject({ multiple: true });
    expect(questions[1].options[0]).toEqual({ label: 's', description: '', recommended: false });
  });

  it('trims the text fields', () => {
    const questions = parseQuestions({ questions: [{ question: '  Q? ', header: ' H ', options: [{ label: ' a ' }] }] });
    expect(questions?.[0]).toMatchObject({ question: 'Q?', header: 'H' });
    expect(questions?.[0].options[0].label).toBe('a');
  });

  it('rejects what is not questions', () => {
    expect(parseQuestions(null)).toBeNull();
    expect(parseQuestions({})).toBeNull();
    expect(parseQuestions({ questions: [] })).toBeNull();
    expect(parseQuestions({ questions: [{ question: '', header: 'H', options: [{ label: 'a' }] }] })).toBeNull();
    expect(parseQuestions({ questions: [{ question: 'Q?', header: 'H', options: [] }] })).toBeNull();
    expect(parseQuestions({ questions: [{ question: 'Q?', header: 'H', options: [{ label: '  ' }] }] })).toBeNull();
    expect(parseQuestions({ questions: ['nope'] })).toBeNull();
    expect(parseQuestions('{"questions": []}')).toBeNull();
  });
});

describe('parseResult', () => {
  const RESULT = {
    answers: [
      { question_id: 'q1', question: 'Which capa?', header: 'Capa', status: 'answered', selected: ['a'], other: 'typed' },
      { question_id: 'q2', question: 'Which size?', header: 'Size', status: 'skipped' },
    ],
  };

  it('parses the object and the JSON string alike', () => {
    expect(parseResult(RESULT)).toEqual(RESULT);
    expect(parseResult(JSON.stringify(RESULT))).toEqual(RESULT);
  });

  it('trims the free text and leaves out an absent one', () => {
    const parsed = parseResult({ answers: [{ question_id: 'q1', question: 'Q?', header: 'H', status: 'answered', other: '  free  ' }] });
    expect(parsed?.answers[0].other).toBe('free');
    expect(parseResult({ answers: [{ question_id: 'q1', question: 'Q?', header: 'H', status: 'answered', selected: [], other: '   ' }] })?.answers[0]).not.toHaveProperty('other');
  });

  it('keeps only string labels, and leaves out an empty selection', () => {
    const parsed = parseResult({ answers: [{ question_id: 'q1', question: 'Q?', header: 'H', status: 'answered', selected: ['a', 3, null] }] });
    expect(parsed?.answers[0].selected).toEqual(['a']);
    expect(parseResult({ answers: [{ question_id: 'q1', question: 'Q?', header: 'H', status: 'answered', selected: [] }] })?.answers[0]).not.toHaveProperty('selected');
  });

  it('rejects what is not a result', () => {
    expect(parseResult(null)).toBeNull();
    expect(parseResult('not json')).toBeNull();
    expect(parseResult('[]')).toBeNull();
    expect(parseResult({})).toBeNull();
    expect(parseResult({ answers: [] })).toBeNull();
    expect(parseResult({ answers: [{ question_id: 'q1', question: 'Q?', header: 'H', status: 'bogus' }] })).toBeNull();
    expect(parseResult({ answers: [{ question_id: '', question: 'Q?', header: 'H', status: 'skipped' }] })).toBeNull();
    expect(parseResult({ answers: ['nope'] })).toBeNull();
  });
});

describe('drafts', () => {
  it('starts unsettled', () => {
    expect(newDrafts(2)).toEqual([
      { selected: [], other: '', skipped: false },
      { selected: [], other: '', skipped: false },
    ]);
    expect(isComplete(newDrafts(2))).toBe(false);
  });

  it('a single-select replaces the choice and clears the free text', () => {
    const questions = specs();
    let drafts = pickOption(newDrafts(2), 0, 1, questions[0].multiple);
    expect(drafts[0]).toEqual({ selected: [1], other: '', skipped: false });
    drafts = setOther(drafts, 0, 'typed', questions[0].multiple);
    // Typing on a single-select clears the choice, so the text is the answer.
    expect(drafts[0]).toEqual({ selected: [], other: 'typed', skipped: false });
    drafts = pickOption(drafts, 0, 0, questions[0].multiple);
    // Picking clears the text again.
    expect(drafts[0]).toEqual({ selected: [0], other: '', skipped: false });
  });

  it('a multiple-select toggles each option and keeps the free text', () => {
    const questions = specs();
    let drafts = pickOption(newDrafts(2), 1, 0, questions[1].multiple);
    drafts = pickOption(drafts, 1, 2, questions[1].multiple);
    expect(drafts[1].selected).toEqual([0, 2]);
    drafts = pickOption(drafts, 1, 0, questions[1].multiple);
    expect(drafts[1].selected).toEqual([2]);
    drafts = setOther(drafts, 1, 'typed', questions[1].multiple);
    expect(drafts[1]).toEqual({ selected: [2], other: 'typed', skipped: false });
  });

  it('picking or typing unskips the question', () => {
    const questions = specs();
    const skipped = toggleSkip(newDrafts(2), 0);
    expect(skipped[0].skipped).toBe(true);
    expect(pickOption(skipped, 0, 0, questions[0].multiple)[0].skipped).toBe(false);
    expect(setOther(skipped, 0, 'x', questions[0].multiple)[0].skipped).toBe(false);
  });

  it('a skip keeps the draft, so unskipping restores it', () => {
    const questions = specs();
    const picked = pickOption(newDrafts(2), 0, 1, questions[0].multiple);
    const skipped = toggleSkip(picked, 0);
    expect(skipped[0]).toEqual({ selected: [1], other: '', skipped: true });
    expect(toggleSkip(skipped, 0)[0]).toEqual({ selected: [1], other: '', skipped: false });
  });

  it('leaves the other questions alone', () => {
    const drafts = toggleSkip(newDrafts(2), 1);
    expect(drafts[0]).toEqual({ selected: [], other: '', skipped: false });
  });

  it('is complete when every question is answered or skipped', () => {
    const questions = specs();
    let drafts = toggleSkip(newDrafts(2), 1);
    expect(isComplete(drafts)).toBe(false);
    drafts = pickOption(drafts, 0, 0, questions[0].multiple);
    expect(isComplete(drafts)).toBe(true);
    // Free text alone answers too; blank text does not.
    expect(isComplete(setOther(newDrafts(1), 0, '  typed  ', false))).toBe(true);
    expect(isComplete(setOther(newDrafts(1), 0, '   ', false))).toBe(false);
  });
});

describe('settled', () => {
  it('is a skipped question, a chosen option, or non-empty free text', () => {
    expect(settled({ selected: [], other: '', skipped: false })).toBe(false);
    expect(settled({ selected: [], other: '   ', skipped: false })).toBe(false);
    expect(settled({ selected: [], other: '', skipped: true })).toBe(true);
    expect(settled({ selected: [1], other: '', skipped: false })).toBe(true);
    expect(settled({ selected: [], other: 'typed', skipped: false })).toBe(true);
  });

  it('finds the first unsettled question, or the count when all are settled', () => {
    expect(firstUnsettled(newDrafts(2))).toBe(0);
    const one = toggleSkip(newDrafts(2), 0);
    expect(firstUnsettled(one)).toBe(1);
    expect(firstUnsettled(toggleSkip(one, 1))).toBe(2);
  });
});

describe('enterIntent', () => {
  it('a single-select option selects and advances', () => {
    expect(enterIntent({ kind: 'option', multiple: false })).toBe('select');
  });

  it('a multi-select option only toggles, never advances', () => {
    expect(enterIntent({ kind: 'option', multiple: true })).toBe('toggle');
  });

  it('a non-empty Other keeps the text and advances, an empty one does nothing', () => {
    expect(enterIntent({ kind: 'other', empty: false })).toBe('keep');
    expect(enterIntent({ kind: 'other', empty: true })).toBe('none');
  });

  it('buttons and anything else keep their native behaviour', () => {
    expect(enterIntent({ kind: 'control' })).toBe('native');
  });
});

describe('closedResult', () => {
  it('cancels every question, carrying its text', () => {
    const questions = specs();
    expect(closedResult(questions)).toEqual({
      answers: [
        { question_id: 'q1', question: 'Which capa?', header: 'Capa', status: 'cancelled' },
        { question_id: 'q2', question: 'Which size?', header: 'Size', status: 'cancelled' },
      ],
    });
  });
});

describe('draftsFromAnswers', () => {
  it('restores the sent choice and free text, and keeps a skip skipped', () => {
    const questions = specs();
    const drafts = draftsFromAnswers(questions, [
      { question_id: 'q1', selected: [1], other: 'typed', skipped: false },
      { question_id: 'q2', selected: [], skipped: true },
    ]);
    expect(drafts).toEqual([
      { selected: [1], other: 'typed', skipped: false },
      { selected: [], other: '', skipped: true },
    ]);
  });

  it('a question with no answer starts fresh, and unknown answers are ignored', () => {
    const questions = specs();
    expect(draftsFromAnswers(questions, [{ question_id: 'q2', selected: [0], skipped: false }])).toEqual([
      { selected: [], other: '', skipped: false },
      { selected: [0], other: '', skipped: false },
    ]);
  });
});

describe('payload', () => {
  it('answers every question in order, with indices in question order', () => {
    const questions = specs();
    let drafts = pickOption(newDrafts(2), 1, 2, questions[1].multiple);
    drafts = pickOption(drafts, 1, 0, questions[1].multiple);
    drafts = setOther(drafts, 0, '  typed  ', questions[0].multiple);
    expect(payload(questions, drafts)).toEqual([
      { question_id: 'q1', selected: [], other: 'typed', skipped: false },
      { question_id: 'q2', selected: [0, 2], skipped: false },
    ]);
  });

  it('a skipped question carries nothing else, and an absent other is left out', () => {
    const questions = specs();
    const drafts = toggleSkip(pickOption(newDrafts(2), 0, 0, questions[0].multiple), 1);
    expect(payload(questions, drafts)).toEqual([
      { question_id: 'q1', selected: [0], skipped: false },
      { question_id: 'q2', selected: [], skipped: true },
    ]);
  });

  it('an unsettled draft reads as skipped, since Submit stays disabled until complete', () => {
    expect(payload(specs(), newDrafts(2))).toEqual([
      { question_id: 'q1', selected: [], skipped: true },
      { question_id: 'q2', selected: [], skipped: true },
    ]);
  });
});

describe('summary', () => {
  it('lists the chosen labels and the free text per question', () => {
    expect(
      summary({
        answers: [
          { question_id: 'q1', question: 'Which capa?', header: 'Capa', status: 'answered', selected: ['a', 'b'], other: 'typed' },
          { question_id: 'q2', question: 'Which size?', header: 'Size', status: 'skipped' },
        ],
      }),
    ).toEqual([
      { header: 'Capa', status: 'answered', lines: ['a', 'b', 'Other: typed'] },
      { header: 'Size', status: 'skipped', lines: ['Skipped'] },
    ]);
  });

  it('names unsettled questions for what ended them', () => {
    expect(
      summary({
        answers: [
          { question_id: 'q1', question: 'Q?', header: 'H1', status: 'timed_out' },
          { question_id: 'q2', question: 'Q?', header: 'H2', status: 'cancelled' },
          { question_id: 'q3', question: 'Q?', header: 'H3', status: 'answered' },
        ],
      }),
    ).toEqual([
      { header: 'H1', status: 'timed_out', lines: ['No answer: the question timed out'] },
      { header: 'H2', status: 'cancelled', lines: ['No answer: the question was cancelled'] },
      { header: 'H3', status: 'answered', lines: ['Answered'] },
    ]);
  });
});

import { describe, expect, it } from 'vitest';
import { currentIndex, displayText, titleText, TITLE_MAX, todoCounts, todoRows, todoSummary } from './todoView';
import type { TodoItem } from './types';

const t = (content: string, status: TodoItem['status'] = 'pending'): TodoItem => ({ content, status });

describe('todoSummary', () => {
  it('shows done over total and the in-progress task', () => {
    const items = [t('a', 'completed'), t('b', 'completed'), t('c', 'in_progress'), t('d'), t('e')];
    expect(todoSummary(items)).toBe('TODO 2/5 · c');
  });

  it('the current task is the first pending one when nothing is in progress', () => {
    const items = [t('a', 'completed'), t('b'), t('c')];
    expect(todoSummary(items)).toBe('TODO 1/3 · b');
  });

  it('the in-progress task wins over an earlier pending one', () => {
    const items = [t('first'), t('active', 'in_progress')];
    expect(todoSummary(items)).toBe('TODO 0/2 · active');
  });

  it('says done when every item is completed', () => {
    expect(todoSummary([t('a', 'completed'), t('b', 'completed'), t('c', 'completed'), t('d', 'completed'), t('e', 'completed')])).toBe(
      'TODO 5/5 done',
    );
  });

  it('is empty for an empty list', () => {
    expect(todoSummary([])).toBe('');
  });

  it('shows the task as plain text, with unsafe characters turned into spaces', () => {
    expect(todoSummary([t('fix‮the\tbuild\n  now')])).toBe('TODO 0/1 · fix the build now');
  });

  it('shows a placeholder for a task with no visible text', () => {
    expect(todoSummary([t('​')])).toBe('TODO 0/1 · (empty task)');
  });
});

describe('todoCounts and currentIndex', () => {
  it('counts each status', () => {
    expect(todoCounts([t('a', 'completed'), t('b', 'in_progress'), t('c'), t('d')])).toEqual({ total: 4, done: 1, inProgress: 1, pending: 2 });
  });

  it('is -1 when every item is completed or the list is empty', () => {
    expect(currentIndex([t('a', 'completed')])).toBe(-1);
    expect(currentIndex([])).toBe(-1);
  });
});

describe('todoRows', () => {
  it('keeps the list order and marks only the current item', () => {
    const rows = todoRows([t('a', 'completed'), t('b'), t('c', 'in_progress'), t('d', 'completed')]);
    expect(rows.map((r) => r.index)).toEqual([0, 1, 2, 3]);
    expect(rows.map((r) => r.text)).toEqual(['a', 'b', 'c', 'd']);
    expect(rows.map((r) => r.current)).toEqual([false, false, true, false]);
    expect(rows.map((r) => r.status)).toEqual(['completed', 'pending', 'in_progress', 'completed']);
  });

  it('marks the first pending item when nothing is in progress', () => {
    const rows = todoRows([t('a', 'completed'), t('b'), t('c')]);
    expect(rows.map((r) => r.current)).toEqual([false, true, false]);
  });

  it('marks nothing when everything is completed', () => {
    expect(todoRows([t('a', 'completed')]).map((r) => r.current)).toEqual([false]);
  });

  it('keeps a long list whole, with every row in order', () => {
    const items = Array.from({ length: 30 }, (_, i) => t(`step ${i + 1}`, i < 10 ? 'completed' : i === 10 ? 'in_progress' : 'pending'));
    const rows = todoRows(items);
    expect(rows).toHaveLength(30);
    expect(rows[29]).toMatchObject({ index: 29, text: 'step 30', current: false });
    expect(rows[10].current).toBe(true);
  });
});

describe('displayText', () => {
  it('turns bidi overrides and isolates into spaces', () => {
    expect(displayText('a‮b⁦c⁩d')).toBe('a b c d');
  });

  it('turns zero-width and byte-order-mark characters into spaces', () => {
    expect(displayText('a​b﻿c')).toBe('a b c');
  });

  it('turns C0 and C1 control characters into spaces', () => {
    expect(displayText('a\u0007b\u009Bc')).toBe('a b c');
  });

  it('collapses white space and trims the ends', () => {
    expect(displayText('  one \n\t two  ')).toBe('one two');
  });

  it('leaves ordinary text, markdown and HTML as plain characters', () => {
    expect(displayText('**bold** <b>x</b> `code`')).toBe('**bold** <b>x</b> `code`');
  });
});

describe('titleText', () => {
  it('keeps a short task whole', () => {
    expect(titleText('write tests')).toBe('write tests');
  });

  it('cuts a long task to TITLE_MAX characters with an ellipsis', () => {
    const long = 'x'.repeat(TITLE_MAX + 50);
    const out = titleText(long);
    expect(Array.from(out)).toHaveLength(TITLE_MAX);
    expect(out.endsWith('…')).toBe(true);
  });

  it('does not split a surrogate pair at the cut', () => {
    const long = '😀'.repeat(TITLE_MAX + 5);
    const out = titleText(long);
    expect(Array.from(out)).toHaveLength(TITLE_MAX);
    expect(out.endsWith('…')).toBe(true);
    expect(out.slice(0, -1)).toBe('😀'.repeat(TITLE_MAX - 1));
  });

  it('cleans unsafe characters before measuring', () => {
    expect(titleText('a‮b')).toBe('a b');
  });
});

import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { TodoPanel } from './TodoPanel';
import type { TodoItem } from '../lib/types';

const t = (content: string, status: TodoItem['status'] = 'pending'): TodoItem => ({ content, status });
const html = (items: TodoItem[], defaultExpanded?: boolean) => renderToStaticMarkup(<TodoPanel items={items} defaultExpanded={defaultExpanded} />);

const LIST = [t('read the code', 'completed'), t('write the fix', 'in_progress'), t('run the tests')];

describe('TodoPanel: collapsed (the default)', () => {
  it('shows the summary line and a closed toggle, with the list hidden', () => {
    const out = html(LIST);
    expect(out).toContain('TODO 1/3 · write the fix');
    expect(out).toContain('aria-expanded="false"');
    expect(out).toMatch(/<ol id="([^"]+)" class="todo-list" hidden=""/);
    expect(out).not.toContain('todo-panel expanded');
  });

  it('the toggle controls the list by id', () => {
    const out = html(LIST);
    const controls = /aria-controls="([^"]+)"/.exec(out)?.[1];
    expect(controls).toBeTruthy();
    expect(out).toContain(`id="${controls}"`);
  });

  it('the toggle is a button, so it is keyboard operable', () => {
    expect(html(LIST)).toMatch(/<button type="button" class="todo-toggle"/);
  });

  it('the summary carries the full current task in the title, cut when it is long', () => {
    const long = 'y'.repeat(300);
    const out = html([t(long, 'in_progress')]);
    expect(out).toContain(`title="Show the todo list. Current: ${'y'.repeat(199)}…"`);
    // The text itself is not cut: the summary and the row keep all of it, and CSS ellipsises the summary line.
    expect(out).toContain(`TODO 0/1 · ${'y'.repeat(300)}`);
  });
});

describe('TodoPanel: expanded', () => {
  it('starts expanded when told to, and the list shows every row in order', () => {
    const out = html(LIST, true);
    expect(out).toContain('aria-expanded="true"');
    expect(out).toContain('todo-panel expanded');
    expect(out).not.toMatch(/class="todo-list" hidden/);
    // The current task also appears in the toggle's title, so the order is checked inside the list only.
    const list = out.slice(out.indexOf('<ol'));
    const order = ['read the code', 'write the fix', 'run the tests'].map((s) => list.indexOf(s));
    expect(order.every((i) => i > 0)).toBe(true);
    expect(order[0]).toBeLessThan(order[1]);
    expect(order[1]).toBeLessThan(order[2]);
  });

  it('marks the current item with aria-current and the current class, and no other', () => {
    const out = html(LIST, true);
    expect(out.match(/aria-current="step"/g)).toHaveLength(1);
    expect(out).toMatch(/class="todo-item todo-in_progress todo-current" aria-current="step"/);
    expect(out).toContain('class="todo-item todo-completed"');
    expect(out).toContain('class="todo-item todo-pending"');
  });

  it('gives each row a status word for screen readers', () => {
    const out = html(LIST, true);
    expect(out).toContain('Done: ');
    expect(out).toContain('In progress: ');
    expect(out).toContain('Pending: ');
  });

  it('shows the task text as plain text: markup in the content is escaped', () => {
    const out = html([t('<img src=x onerror=alert(1)> **bold**')], true);
    expect(out).not.toContain('<img');
    expect(out).toContain('&lt;img');
    expect(out).toContain('**bold**');
  });

  it('removes bidi controls from what is shown', () => {
    const out = html([t('safe‮evil', 'in_progress')], true);
    expect(out).not.toContain('‮');
    expect(out).toContain('safe evil');
  });
});

describe('TodoPanel: empty and done', () => {
  it('renders nothing for an empty list', () => {
    expect(html([])).toBe('');
  });

  it('says done when every item is completed, with no current marker', () => {
    const out = html([t('a', 'completed'), t('b', 'completed'), t('c', 'completed'), t('d', 'completed'), t('e', 'completed')]);
    expect(out).toContain('TODO 5/5 done');
    expect(out).not.toContain('aria-current');
    expect(out).toContain('todo-panel all-done');
  });

  it('a list with a current item is not marked all-done', () => {
    expect(html(LIST)).not.toContain('all-done');
  });
});

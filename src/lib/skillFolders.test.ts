import { describe, expect, it } from 'vitest';
import {
  MAX_SKILL_FOLDERS,
  MAX_SKILL_FOLDER_CHARS,
  addFolder,
  folderProblem,
  removeFolder,
  rowsProblem,
  saveInput,
  setFolder,
  setSwitch,
  shadowNote,
} from './skillFolders';

describe('skill folder rows', () => {
  it('adds a trimmed folder at the end and refuses an empty, repeated or too long one', () => {
    expect(addFolder(['~/a'], '  ~/b  ')).toEqual({ rows: ['~/a', '~/b'], error: null });
    expect(addFolder(['~/a'], '   ').error).toBe('A folder is empty.');
    expect(addFolder(['~/a'], '~/a').error).toBe('That folder is already in the list.');
    expect(addFolder([], 'x'.repeat(MAX_SKILL_FOLDER_CHARS + 1)).error).toContain('longer than 1024');
  });

  it('refuses a folder past the count, leaving the rows as they were', () => {
    const full = Array.from({ length: MAX_SKILL_FOLDERS }, (_, i) => `/f${i}`);
    const result = addFolder(full, '/one-more');
    expect(result.rows).toBe(full);
    expect(result.error).toBe(`At most ${MAX_SKILL_FOLDERS} folders.`);
  });

  it('removes and replaces the row at an index', () => {
    expect(removeFolder(['a', 'b', 'c'], 1)).toEqual(['a', 'c']);
    expect(setFolder(['a', 'b'], 0, '~/x')).toEqual(['~/x', 'b']);
  });

  it('reports the first problem of the rows, or none', () => {
    expect(rowsProblem(['~/a', '/b'])).toBeNull();
    expect(rowsProblem(['~/a', ' '])).toBe('A folder is empty.');
    expect(folderProblem('a\0b')).toBe('A folder has a NUL character.');
  });
});

describe('skill switches', () => {
  it('turns a skill off once and on again by name', () => {
    expect(setSwitch([], 'pdf', false)).toEqual(['pdf']);
    expect(setSwitch(['pdf'], 'pdf', false)).toEqual(['pdf']);
    expect(setSwitch(['pdf', 'git'], 'pdf', true)).toEqual(['git']);
    expect(setSwitch(['git'], 'pdf', true)).toEqual(['git']);
  });
});

describe('save input', () => {
  it('trims folders and sends null for the defaults', () => {
    expect(saveInput([' ~/a ', '/b'], ['pdf'])).toEqual({ folders: ['~/a', '/b'], disabled: ['pdf'] });
    expect(saveInput(null, [])).toEqual({ folders: null, disabled: [] });
  });

  it('names the shadowing folder in the note', () => {
    expect(shadowNote('C:/Users/me/.agents/skills')).toBe('Hidden: the one in C:/Users/me/.agents/skills is used instead.');
  });
});

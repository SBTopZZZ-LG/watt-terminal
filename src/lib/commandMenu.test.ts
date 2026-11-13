import { describe, expect, it } from 'vitest';
import { acceptMenuItem, commandMenuItems, filterCommandMenu, menuAcceptRuns, menuKeyIntent, menuQuery, moveMenuIndex } from './commandMenu';
import type { CommandMenuItem } from './commandMenu';
import { BUILTIN_COMMANDS } from './commands';
import type { CommandCatalog } from './types';

const EMPTY: CommandCatalog = { commands: [], project_dir: null, global_dir: null };

const CATALOG: CommandCatalog = {
  commands: [
    { name: 'deploy', description: 'Deploy it.', hint: '[env]', source: 'project', template: 'deploy' },
    { name: 'fixup', description: 'Fix it.', hint: null, source: 'global', template: 'fix' },
  ],
  project_dir: '/w/.watt/commands',
  global_dir: '/g/commands',
};

const item = (over: Partial<CommandMenuItem> = {}): CommandMenuItem => ({
  name: 'plan',
  description: 'Plan.',
  hint: '[task]',
  source: null,
  ...over,
});

const plain = { shift: false, ctrl: false, meta: false, alt: false };

describe('menuQuery', () => {
  it('is the name prefix for a bare slash name, empty for a lone slash', () => {
    expect(menuQuery('/')).toBe('');
    expect(menuQuery('/pl')).toBe('pl');
    expect(menuQuery('/PLAN')).toBe('PLAN');
    expect(menuQuery('/a-b_c9')).toBe('a-b_c9');
  });

  it('is null for anything that is not a bare slash name', () => {
    expect(menuQuery('')).toBeNull();
    expect(menuQuery('hello')).toBeNull();
    expect(menuQuery('/plan now')).toBeNull();
    expect(menuQuery('/plan ')).toBeNull();
    expect(menuQuery('/usr/local')).toBeNull();
    expect(menuQuery(' /plan')).toBeNull();
    expect(menuQuery('/pl@n')).toBeNull();
  });
});

describe('commandMenuItems', () => {
  it('lists the built-ins first, then the /clear alias, then the customs', () => {
    const items = commandMenuItems(CATALOG);
    const names = items.map((i) => i.name);
    expect(names.slice(0, BUILTIN_COMMANDS.length)).toEqual(BUILTIN_COMMANDS.map((b) => b.name));
    expect(names[BUILTIN_COMMANDS.length]).toBe('clear');
    expect(names.slice(BUILTIN_COMMANDS.length + 1)).toEqual(['deploy', 'fixup']);
  });

  it('the alias points at /new and carries no hint or source', () => {
    const clear = commandMenuItems(EMPTY).find((i) => i.name === 'clear');
    expect(clear).toMatchObject({ description: 'Alias for /new.', hint: null, source: null });
  });

  it('customs keep their hint, description and source', () => {
    const items = commandMenuItems(CATALOG);
    expect(items[items.length - 2]).toMatchObject({ name: 'deploy', hint: '[env]', source: 'project' });
    expect(items[items.length - 1]).toMatchObject({ name: 'fixup', hint: null, source: 'global' });
  });

  it('is only the built-ins and the alias with an empty catalog', () => {
    expect(commandMenuItems(EMPTY)).toHaveLength(BUILTIN_COMMANDS.length + 1);
  });
});

describe('filterCommandMenu', () => {
  const items = commandMenuItems(CATALOG);

  it('an empty query matches every row', () => {
    expect(filterCommandMenu(items, '')).toBe(items);
  });

  it('matches a case-insensitive prefix on the name', () => {
    expect(filterCommandMenu(items, 'P').map((i) => i.name)).toContain('plan');
    expect(filterCommandMenu(items, 'pl').map((i) => i.name)).toEqual(['plan']);
    expect(filterCommandMenu(items, 'dep').map((i) => i.name)).toEqual(['deploy']);
  });

  it('does not match the middle of a name, or anything else', () => {
    expect(filterCommandMenu(items, 'lan')).toEqual([]);
    expect(filterCommandMenu(items, 'zzz')).toEqual([]);
  });
});

describe('moveMenuIndex', () => {
  it('steps and wraps around both ends', () => {
    expect(moveMenuIndex(0, 1, 3)).toBe(1);
    expect(moveMenuIndex(2, 1, 3)).toBe(0);
    expect(moveMenuIndex(0, -1, 3)).toBe(2);
    expect(moveMenuIndex(1, -2, 3)).toBe(2);
  });

  it('is zero with no rows', () => {
    expect(moveMenuIndex(0, 1, 0)).toBe(0);
  });
});

describe('acceptMenuItem', () => {
  it('adds a trailing space when the command takes arguments', () => {
    expect(acceptMenuItem(item({ name: 'plan', hint: '[task]' }))).toBe('/plan ');
    expect(acceptMenuItem(item({ name: 'deploy', hint: '[env]' }))).toBe('/deploy ');
  });

  it('has no trailing space when the command takes none', () => {
    expect(acceptMenuItem(item({ name: 'new', hint: null }))).toBe('/new');
    expect(acceptMenuItem(item({ name: 'clear', hint: null }))).toBe('/clear');
  });
});

describe('menuAcceptRuns', () => {
  it('is true when the text is exactly the command, case-insensitively', () => {
    expect(menuAcceptRuns('/model', item({ name: 'model', hint: null }))).toBe(true);
    expect(menuAcceptRuns('/MODEL', item({ name: 'model', hint: null }))).toBe(true);
    expect(menuAcceptRuns('/Model', item({ name: 'MoDeL', hint: null }))).toBe(true);
  });

  it('is false for a partial name or anything with arguments', () => {
    expect(menuAcceptRuns('/mod', item({ name: 'model', hint: null }))).toBe(false);
    expect(menuAcceptRuns('/model ', item({ name: 'model', hint: null }))).toBe(false);
    expect(menuAcceptRuns('/model x', item({ name: 'model', hint: null }))).toBe(false);
    expect(menuAcceptRuns('/other', item({ name: 'model', hint: null }))).toBe(false);
  });
});

describe('menuKeyIntent', () => {
  it('moves on plain arrows', () => {
    expect(menuKeyIntent('ArrowDown', plain)).toBe('next');
    expect(menuKeyIntent('ArrowUp', plain)).toBe('prev');
  });

  it('ignores modified arrows and Enter', () => {
    expect(menuKeyIntent('ArrowDown', { ...plain, shift: true })).toBeNull();
    expect(menuKeyIntent('ArrowUp', { ...plain, ctrl: true })).toBeNull();
    expect(menuKeyIntent('Enter', { ...plain, shift: true })).toBeNull();
    expect(menuKeyIntent('Enter', { ...plain, ctrl: true })).toBeNull();
    expect(menuKeyIntent('Enter', { ...plain, meta: true })).toBeNull();
    expect(menuKeyIntent('Enter', { ...plain, alt: true })).toBeNull();
  });

  it('accepts on plain Enter and Tab, but not Shift+Tab', () => {
    expect(menuKeyIntent('Enter', plain)).toBe('accept');
    expect(menuKeyIntent('Tab', plain)).toBe('accept');
    expect(menuKeyIntent('Tab', { ...plain, shift: true })).toBeNull();
  });

  it('dismisses on Escape and ignores other keys', () => {
    expect(menuKeyIntent('Escape', plain)).toBe('dismiss');
    expect(menuKeyIntent('c', plain)).toBeNull();
    expect(menuKeyIntent('Enter', plain)).toBe('accept');
  });
});

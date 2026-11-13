// The slash-command autocomplete menu's logic: when the bar's text is a command prefix, what the menu shows,
// how it filters, moves and accepts. Pure; InputBar.tsx renders it. Built-ins first, then the `/clear` alias,
// then the custom commands (the catalog is already sorted). Unknown `/...` text is not a command.

import { BUILTIN_COMMANDS, COMMAND_ALIASES } from './commands';
import type { CommandCatalog, CommandSource } from './types';

/** One menu row. `source` is null for built-ins and the alias, project/global for custom commands. */
export interface CommandMenuItem {
  name: string;
  description: string;
  hint: string | null;
  source: CommandSource | null;
}

/**
 * The menu's query for the bar's text: the name prefix when the whole text is `/` plus name characters
 * (letters, digits, `_`, `-`), or null when the text is anything else (plain text, `/usr/local`, `/cmd args).
 * A lone `/` is the empty query: the whole menu.
 */
export function menuQuery(text: string): string | null {
  const match = /^\/([A-Za-z0-9_-]*)$/.exec(text);
  return match ? match[1] : null;
}

/** Every menu row: the built-ins in list order, the `/clear` alias, then the catalog's commands. */
export function commandMenuItems(catalog: CommandCatalog): CommandMenuItem[] {
  const items: CommandMenuItem[] = BUILTIN_COMMANDS.map((b) => ({
    name: b.name,
    description: b.description,
    hint: b.hint,
    source: null,
  }));
  const clearTarget = COMMAND_ALIASES.clear;
  if (clearTarget !== undefined) {
    items.push({ name: 'clear', description: `Alias for /${clearTarget}.`, hint: null, source: null });
  }
  for (const cmd of catalog.commands) {
    items.push({ name: cmd.name, description: cmd.description, hint: cmd.hint, source: cmd.source });
  }
  return items;
}

/** The rows whose name starts with the query, case-insensitively. An empty query matches every row. */
export function filterCommandMenu(items: CommandMenuItem[], query: string): CommandMenuItem[] {
  if (query === '') {
    return items;
  }
  const lower = query.toLowerCase();
  return items.filter((item) => item.name.toLowerCase().startsWith(lower));
}

/** The index `delta` steps from `index`, wrapping around `count`. Zero when there is no row. */
export function moveMenuIndex(index: number, delta: number, count: number): number {
  if (count <= 0) {
    return 0;
  }
  return ((index + delta) % count + count) % count;
}

/** The bar's text for an accepted row: `/name`, with a trailing space when it takes arguments. */
export function acceptMenuItem(item: CommandMenuItem): string {
  return item.hint !== null ? `/${item.name} ` : `/${item.name}`;
}

/**
 * True when the bar's text is exactly the command, case-insensitively (`/model` for `model`): Enter runs
 * it instead of completing it. Anything else (`/mod`, `/model x`) still completes.
 */
export function menuAcceptRuns(text: string, item: CommandMenuItem): boolean {
  return text.toLowerCase() === `/${item.name.toLowerCase()}`;
}

export type MenuKeyIntent = 'next' | 'prev' | 'accept' | 'dismiss' | null;

export interface MenuKeyMods {
  shift: boolean;
  ctrl: boolean;
  meta: boolean;
  alt: boolean;
}

/**
 * What a key pressed with the menu open means. Plain ArrowDown/Up move, plain Enter accepts, Tab accepts
 * (Shift+Tab keeps moving focus), Escape dismisses. Anything else, and modified arrows/Enter, is null:
 * the bar handles it as usual. (Modified Tab and Escape are left to the caller: InputBar skips the menu
 * for any Ctrl/Meta/Alt chord.)
 */
export function menuKeyIntent(key: string, mods: MenuKeyMods): MenuKeyIntent {
  switch (key) {
    case 'ArrowDown':
      return mods.shift || mods.ctrl || mods.meta || mods.alt ? null : 'next';
    case 'ArrowUp':
      return mods.shift || mods.ctrl || mods.meta || mods.alt ? null : 'prev';
    case 'Enter':
      return mods.shift || mods.ctrl || mods.meta || mods.alt ? null : 'accept';
    case 'Tab':
      return mods.shift ? null : 'accept';
    case 'Escape':
      return 'dismiss';
    default:
      return null;
  }
}

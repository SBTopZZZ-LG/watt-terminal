// The custom context menu's rules, without the DOM: which items each region offers, how the keyboard moves through
// them, and where the menu goes. Pure, so it is unit-tested. ContextMenu.tsx runs the actions and contextMenuDom.ts reads
// the page; neither is unit-tested (no DOM in vitest).

import type { InputState } from './types';
import { IS_MAC, shortcutLabel } from './platform';
import type { ShortcutName } from './platform';
import { moveMenuIndex } from './commandMenu';
import type { MenuKeyMods } from './commandMenu';
import { placement } from './tooltip';
import type { Placement, Size } from './tooltip';

/** Where the right click (or the Menu key) landed. `none` is anywhere without items: dividers, empty chrome, buttons. */
export type MenuRegion = 'terminal' | 'field' | 'chat' | 'chip' | 'tab' | 'none';

export type MenuItemId =
  | 'copy'
  | 'copy-terminal'
  | 'cut'
  | 'paste'
  | 'select-all'
  | 'find-terminal'
  | 'find-chat'
  | 'clear'
  | 'scroll-bottom'
  | 'copy-message'
  | 'copy-code'
  | 'copy-tool-input'
  | 'copy-tool-output'
  | 'copy-tool-json'
  | 'copy-link'
  | 'copy-chip'
  | 'tab-visibility'
  | 'tab-restart'
  | 'tab-close'
  | 'open-layout-inspector';

export interface MenuItem {
  /** What the item does. `separator` for a divider row (no action). */
  id: MenuItemId | 'separator';
  label: string;
  /** The platform's shortcut for the same action, as a hint. Absent when the app has none for it. */
  shortcut?: string;
  disabled?: boolean;
  separator?: boolean;
}

/** The facts the resolver reads from the page. Plain data, so menuFor needs no DOM. */
export interface MenuContext {
  region: MenuRegion;
  /** There is something to copy: xterm's selection, the field's selected text, or the page's selection in the chat. */
  hasSelection: boolean;
  /** Terminal: keys go to the PTY, so a paste reaches the shell. */
  pasteRoutesToPty: boolean;
  /** Terminal: the effective input state. Clear is offered only at the prompt. */
  inputState: InputState;
  /** Terminal: the alternate screen is up (a full-screen app owns the screen). */
  altScreen: boolean;
  /** The agent is busy (a turn or a side command). Clearing the terminal would race it. */
  agentBusy: boolean;
  /** Field: it takes text (not read-only, not disabled). Cut and Paste need it. */
  writable: boolean;
  /** Chat: the target is inside a message. */
  message: boolean;
  /** Chat: the target is inside a code block. */
  code: boolean;
  /** Chat: the target is inside a tool row. */
  tool: boolean;
  /** Chat: the target is a link with an href. */
  link: boolean;
  /** Tab: the terminal is visible to the chat (its owner is this chat). The item says what the click does. */
  tabVisible: boolean;
  /** Tab: another chat owns the terminal. The item then moves it to this chat. */
  tabOther: boolean;
  /** Tab: the terminal can be closed (the last one cannot). */
  tabCanClose: boolean;
}

/** A context with every flag off: the region alone decides, and each test sets what it needs. */
export function emptyContext(region: MenuRegion): MenuContext {
  return {
    region,
    hasSelection: false,
    pasteRoutesToPty: false,
    inputState: 'prompt',
    altScreen: false,
    agentBusy: false,
    writable: false,
    message: false,
    code: false,
    tool: false,
    link: false,
    tabVisible: false,
    tabOther: false,
    tabCanClose: false,
  };
}

/**
 * One item. Its shortcut hint is the platform's label for the same key (none when the app has no such key: the terminal's
 * Select all, since Ctrl+A there is the shell's). `false` entries are dropped by joinGroups, so an item can be offered
 * conditionally.
 */
function item(id: MenuItemId, label: string, opts: { shortcut?: ShortcutName; disabled?: boolean; mac: boolean }): MenuItem {
  const out: MenuItem = { id, label };
  if (opts.shortcut) {
    out.shortcut = shortcutLabel(opts.shortcut, opts.mac);
  }
  if (opts.disabled) {
    out.disabled = true;
  }
  return out;
}

const SEPARATOR: MenuItem = { id: 'separator', label: '', separator: true };

/** Joins the groups with one separator between each pair of non-empty groups. */
function joinGroups(groups: (MenuItem | false)[][]): MenuItem[] {
  const out: MenuItem[] = [];
  for (const group of groups) {
    const items = group.filter((entry): entry is MenuItem => entry !== false);
    if (items.length === 0) {
      continue;
    }
    if (out.length > 0) {
      out.push(SEPARATOR);
    }
    out.push(...items);
  }
  return out;
}

/**
 * The items for a right click in a region. Terminal: Copy (xterm selection), Paste (only when keys reach the PTY), Select
 * all, Find in terminal; then Clear terminal (only at the prompt, with the agent idle, off the alternate screen) and Scroll
 * to bottom. Field: Cut and Paste when it is writable, Copy and Cut without a selection are disabled, then Select all.
 * Chat: Copy (selection), Copy message, Select all, Find in chat; then the code, tool and link items where they apply.
 * Chip: Copy value. Tab: the assignment (Make visible / Move to this chat / Hide from agent), Restart, then Close (disabled for
 * the last terminal). None: no items.
 */
export function menuFor(ctx: MenuContext, mac: boolean = IS_MAC): MenuItem[] {
  switch (ctx.region) {
    case 'tab':
      return joinGroups([
        [
          item('tab-visibility', ctx.tabVisible ? 'Hide from agent' : ctx.tabOther ? 'Move to this chat' : 'Make visible to agent', { mac }),
          item('tab-restart', 'Restart', { mac }),
        ],
        [item('tab-close', 'Close', { disabled: !ctx.tabCanClose, mac })],
      ]);
    case 'terminal': {
      const clearOffered = ctx.inputState === 'prompt' && !ctx.agentBusy && !ctx.altScreen;
      return joinGroups([
        [
          item('copy-terminal', 'Copy', { shortcut: 'copy-terminal', disabled: !ctx.hasSelection, mac }),
          ctx.pasteRoutesToPty && item('paste', 'Paste', { shortcut: 'paste', mac }),
          item('select-all', 'Select all', { mac }),
          item('find-terminal', 'Find in terminal', { shortcut: 'find-terminal', mac }),
        ],
        [
          clearOffered && item('clear', 'Clear terminal', { shortcut: 'clear', mac }),
          item('scroll-bottom', 'Scroll to bottom', { mac }),
        ],
      ]);
    }
    case 'field':
      return joinGroups([
        [
          ctx.writable && item('cut', 'Cut', { shortcut: 'cut', disabled: !ctx.hasSelection, mac }),
          item('copy', 'Copy', { shortcut: 'copy', disabled: !ctx.hasSelection, mac }),
          ctx.writable && item('paste', 'Paste', { shortcut: 'paste', mac }),
        ],
        [item('select-all', 'Select all', { shortcut: 'select-all', mac })],
      ]);
    case 'chat':
      return joinGroups([
        [
          item('copy', 'Copy', { shortcut: 'copy', disabled: !ctx.hasSelection, mac }),
          ctx.message && item('copy-message', 'Copy message', { mac }),
        ],
        [
          item('select-all', 'Select all', { shortcut: 'select-all', mac }),
          item('find-chat', 'Find in chat', { shortcut: 'find', mac }),
        ],
        [
          ctx.code && item('copy-code', 'Copy code', { mac }),
          ctx.tool && item('copy-tool-input', 'Copy input', { mac }),
          ctx.tool && item('copy-tool-output', 'Copy output', { mac }),
          ctx.tool && item('copy-tool-json', 'Copy JSON', { mac }),
          ctx.link && item('copy-link', 'Copy link', { mac }),
        ],
      ]);
    case 'chip':
      return joinGroups([[item('copy-chip', 'Copy value', { mac })]]);
    case 'none':
      return [];
  }
}

/** The layout inspector item. Offered only while the developer setting is on (see withLayoutInspector). */
const LAYOUT_INSPECTOR: MenuItem = { id: 'open-layout-inspector', label: 'Open layout inspector' };

/**
 * Adds the layout inspector after its own separator, when the developer setting is on and the menu has items. Nothing
 * changes otherwise, so an empty menu stays empty. The app opens every menu through this, so the item is in each one.
 */
export function withLayoutInspector(items: MenuItem[], enabled: boolean): MenuItem[] {
  if (!enabled || items.length === 0) {
    return items;
  }
  return [...items, SEPARATOR, LAYOUT_INSPECTOR];
}

/** Indices of the items the keyboard and the mouse can land on: not a separator, not disabled. */
export function enabledIndices(items: readonly MenuItem[]): number[] {
  const out: number[] = [];
  items.forEach((it, i) => {
    if (!it.separator && !it.disabled) {
      out.push(i);
    }
  });
  return out;
}

/**
 * The enabled item `delta` steps from `index`, wrapping, skipping separators and disabled items. From no selection (-1),
 * a step forward is the first item and a step back is the last. -1 when nothing is enabled.
 */
export function stepIndex(items: readonly MenuItem[], index: number, delta: number): number {
  const enabled = enabledIndices(items);
  if (enabled.length === 0) {
    return -1;
  }
  const pos = enabled.indexOf(index);
  const from = pos !== -1 ? pos : delta > 0 ? -1 : 0;
  return enabled[moveMenuIndex(from, delta, enabled.length)];
}

/** The first or last enabled item, or -1 when nothing is enabled. */
export function edgeIndex(items: readonly MenuItem[], edge: 'first' | 'last'): number {
  const enabled = enabledIndices(items);
  if (enabled.length === 0) {
    return -1;
  }
  return edge === 'first' ? enabled[0] : enabled[enabled.length - 1];
}

/**
 * The next enabled item, after `index` and wrapping, whose label starts with `char` (case-insensitive). -1 when none does.
 * The search starts after `index`, so repeated letters walk through the items that share it.
 */
export function typeaheadIndex(items: readonly MenuItem[], char: string, index: number): number {
  const n = items.length;
  const lower = char.toLowerCase();
  for (let k = 1; k <= n; k++) {
    const i = (((index + k) % n) + n) % n;
    const it = items[i];
    if (!it.separator && !it.disabled && it.label.toLowerCase().startsWith(lower)) {
      return i;
    }
  }
  return -1;
}

export type MenuKey =
  | { kind: 'next' }
  | { kind: 'prev' }
  | { kind: 'first' }
  | { kind: 'last' }
  | { kind: 'activate' }
  | { kind: 'close' }
  | { kind: 'type'; char: string }
  | null;

/**
 * What a key pressed while the menu is open means. Arrows move (Up/Down), Home/End jump, Enter and Space activate, Escape
 * closes, and a single printable character types ahead. Tab is swallowed (it would move focus out of the menu). Modified
 * keys are null.
 */
export function menuKey(key: string, mods: MenuKeyMods): MenuKey {
  if (key === 'Tab') {
    return null;
  }
  const modified = mods.ctrl || mods.meta || mods.alt;
  switch (key) {
    case 'ArrowDown':
      return modified || mods.shift ? null : { kind: 'next' };
    case 'ArrowUp':
      return modified || mods.shift ? null : { kind: 'prev' };
    case 'Home':
      return modified ? null : { kind: 'first' };
    case 'End':
      return modified ? null : { kind: 'last' };
    case 'Enter':
      return modified ? null : { kind: 'activate' };
    case ' ':
      return modified ? null : { kind: 'activate' };
    case 'Escape':
      return { kind: 'close' };
    default:
      if (!modified && [...key].length === 1 && key.trim() !== '') {
        return { kind: 'type', char: key };
      }
      return null;
  }
}

/**
 * Where the menu's top-left corner goes for a point (the pointer, or the focused element's corner), kept inside the
 * viewport and flipped above the point when there is no room below. This is the tooltip's placement(): it centres its box
 * under an anchor, so the anchor is a zero-width box half a menu's width to the right of the point, which puts the menu's
 * left edge on the point.
 */
export function menuPlacement(point: { x: number; y: number }, menu: Size, viewport: Size): Placement {
  return placement({ left: point.x + menu.width / 2, top: point.y, width: 0, height: 0 }, menu, viewport);
}

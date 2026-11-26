// Reads the page under a right click (or the focused element, for the Menu key) and turns it into the menu's context and
// the texts its copy items need. The rules are in contextMenu.ts; this file only reads the DOM, so it is not unit-tested
// (vitest has no DOM). The region order matters: the terminal first (its helper textarea is a textarea), then fields (a
// question's input sits inside the transcript), then the chat, then the status chips.

import { emptyContext } from './contextMenu';
import type { MenuContext } from './contextMenu';
import type { InputState } from './types';

/** What the terminal side of the page says, read by the caller when the menu opens. */
export interface TerminalFacts {
  state: InputState;
  agentBusy: boolean;
  pasteRoutesToPty: boolean;
  altScreen: boolean;
  /** xterm has a selection. */
  hasSelection: boolean;
}

/** What the tab registry says about one terminal, read by the caller when the menu opens. */
export interface TabFacts {
  /** The chat owns the terminal (it is visible to the agent). */
  visible: boolean;
  /** The terminal may be closed (it is not the last one). */
  canClose: boolean;
  /** Another chat owns the terminal (the item moves it to this one). Absent or false: none does, or this chat does. */
  elsewhere?: boolean;
}

export type TextField = HTMLInputElement | HTMLTextAreaElement;

/** The input types that take typed text. Other inputs (checkbox, range, button) get no menu. */
const TEXT_INPUT_TYPES = ['text', 'search', 'url', 'email', 'tel', 'password', 'number'];

export interface MenuSnapshot {
  ctx: MenuContext;
  /** The focused element when the menu opened. Focus goes back to it on close (the terminal gets term.focus() instead). */
  origin: HTMLElement | null;
  /** The field, in the field region. */
  field: TextField | null;
  /** The transcript body, in the chat region (Select all selects its text). */
  transcript: HTMLElement | null;
  /** The terminal's id, in the tab region (the tab the items act on). */
  tabId: string | null;
  /** Texts for the copy items, read now so they are what the user saw. Empty when the region has none. */
  texts: { selection: string; message: string; code: string; toolInput: string; toolOutput: string; toolJson: string; link: string; chip: string };
}

function emptyTexts(): MenuSnapshot['texts'] {
  return { selection: '', message: '', code: '', toolInput: '', toolOutput: '', toolJson: '', link: '', chip: '' };
}

/** Whether the element is a field the user types into (any text-like input, or a textarea). */
export function isTextField(el: Element): el is TextField {
  if (el instanceof HTMLTextAreaElement) {
    return true;
  }
  return el instanceof HTMLInputElement && TEXT_INPUT_TYPES.includes(el.type);
}

/** The selected text of a field. A field with no selection, or one that has no selection API, gives ''. */
export function fieldSelectedText(field: TextField): string {
  const start = field.selectionStart ?? 0;
  const end = field.selectionEnd ?? 0;
  return start === end ? '' : field.value.slice(Math.min(start, end), Math.max(start, end));
}

/** A message's text as the user reads it: its body, without the speaker label, the attachment list and the copy controls. */
function messageText(msg: Element): string {
  const clone = msg.cloneNode(true) as Element;
  for (const junk of clone.querySelectorAll('.who, .msg-attachments, .msg-actions, .md-code-copy')) {
    junk.remove();
  }
  return (clone.textContent ?? '').trim();
}

/** A pane's text. '' when the pane is missing or shows a placeholder (ToolRow marks those with data-empty). */
function paneText(pre: Element | null | undefined): string {
  return pre == null || pre.hasAttribute('data-empty') ? '' : (pre.textContent ?? '');
}

/** The texts of a tool row: its Input and Output panes, and its JSON pane (see ToolRow.tsx). Empty for a group. */
function toolTexts(row: Element): Pick<MenuSnapshot['texts'], 'toolInput' | 'toolOutput' | 'toolJson'> | null {
  const io = row.querySelector(':scope > .tool-tabs > [id$="-panel-io"]');
  const json = row.querySelector(':scope > .tool-tabs > [id$="-panel-json"]');
  if (io === null || json === null) {
    return null;
  }
  const pres = io.querySelectorAll(':scope > pre');
  return {
    toolInput: paneText(pres[0]),
    toolOutput: paneText(pres[1]),
    toolJson: paneText(json.querySelector(':scope > pre')),
  };
}

/**
 * The menu for a target element: its context and texts, or null when the region has no items (dividers, empty chrome,
 * buttons, a chip with no value). `origin` is the element focused when the menu opens.
 */
export function resolveMenu(
  target: Element,
  terminal: TerminalFacts,
  origin: HTMLElement | null,
  tabFacts: (id: string) => TabFacts | null,
): MenuSnapshot | null {
  const texts = emptyTexts();
  const base = { origin, field: null, transcript: null, tabId: null, texts };

  if (target.closest('.xterm') !== null) {
    const ctx: MenuContext = {
      ...emptyContext('terminal'),
      hasSelection: terminal.hasSelection,
      pasteRoutesToPty: terminal.pasteRoutesToPty,
      inputState: terminal.state,
      altScreen: terminal.altScreen,
      agentBusy: terminal.agentBusy,
    };
    return { ...base, ctx };
  }

  // A tab, or its close button (both sit in the same cell). The strip is not the terminal, so the terminal menu is not offered.
  const cell = target.closest('.term-tab-cell');
  if (cell !== null) {
    const id = cell.querySelector('[data-tab-id]')?.getAttribute('data-tab-id') ?? null;
    const facts = id === null ? null : tabFacts(id);
    if (id === null || facts === null) {
      return null;
    }
    const ctx: MenuContext = { ...emptyContext('tab'), tabVisible: facts.visible, tabOther: facts.elsewhere === true, tabCanClose: facts.canClose };
    return { ...base, ctx, tabId: id };
  }

  if (isTextField(target)) {
    const ctx: MenuContext = {
      ...emptyContext('field'),
      writable: !target.readOnly && !target.disabled,
      hasSelection: fieldSelectedText(target) !== '',
    };
    return { ...base, ctx, field: target };
  }

  const transcript = target.closest<HTMLElement>('.transcript-body');
  if (transcript !== null) {
    const row = target.closest('details.tool');
    const msg = target.closest('.msg');
    const pre = target.closest('.md pre');
    const link = target.closest('.md-link');
    const href = link?.getAttribute('data-tip') ?? '';
    const tool = row !== null ? toolTexts(row) : null;
    texts.message = msg !== null ? messageText(msg) : '';
    texts.code = pre?.textContent ?? '';
    texts.link = href;
    texts.selection = target.ownerDocument.getSelection()?.toString() ?? '';
    if (tool !== null) {
      Object.assign(texts, tool);
    }
    const ctx: MenuContext = {
      ...emptyContext('chat'),
      hasSelection: texts.selection !== '',
      message: msg !== null,
      code: pre !== null,
      tool: tool !== null,
      link: href !== '',
    };
    return { ...base, ctx, transcript };
  }

  const chip = target.closest('.chip-model, .chip-provider, .chip-cwd');
  if (chip !== null) {
    // The provider chip shows "provider: name"; its tip is the name itself.
    const value = chip.classList.contains('chip-provider') ? (chip.getAttribute('data-tip') ?? '') : (chip.textContent ?? '').trim();
    if (value === '') {
      return null;
    }
    texts.chip = value;
    return { ...base, ctx: emptyContext('chip') };
  }

  return null;
}

// Ctrl+A (Cmd+A on macOS) outside a field must not select the whole page. Pure decisions here; App.tsx runs them from its
// window keydown listener. In a field the browser's own select-all runs, and in the terminal Ctrl+A stays the shell's key
// (readline's begin-of-line) and Cmd+A stays xterm's select-all, so neither is taken here.

import { IS_MAC } from './platform';

/** The fields of a keyboard event that the predicate reads. A DOM KeyboardEvent satisfies this. */
export interface SelectAllKeyEvent {
  type: string;
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

/** Ctrl+A on Windows and Linux, Cmd+A on macOS, with no other modifier (Ctrl+Shift+A is the agent bar's hotkey). */
export function isSelectAllKey(ev: SelectAllKeyEvent, mac: boolean = IS_MAC): boolean {
  if (ev.type !== 'keydown' || ev.altKey || ev.shiftKey || ev.key.toLowerCase() !== 'a') {
    return false;
  }
  return mac ? ev.metaKey && !ev.ctrlKey : ev.ctrlKey && !ev.metaKey;
}

/** Where the focus is, as far as select-all cares. */
export interface SelectAllFocus {
  /** The focused element is an input, a textarea or a contenteditable element. */
  editable: boolean;
  /** The focused element is inside the terminal (.xterm). */
  inTerminal: boolean;
  /** The focused element is inside the agent panel. */
  inAgentPanel: boolean;
}

/**
 * What select-all selects, or null when the key is not ours. Null for a field (the browser selects its text) and for the
 * terminal (xterm, or the shell for Ctrl+A). Inside the agent panel it is the transcript; anywhere else, the terminal.
 */
export function selectAllTarget(focus: SelectAllFocus): 'agent' | 'terminal' | null {
  if (focus.editable || focus.inTerminal) {
    return null;
  }
  return focus.inAgentPanel ? 'agent' : 'terminal';
}

/** Whether the element is a text field, by its tag or its contenteditable attribute. */
export function isEditableElement(el: Element | null): boolean {
  return el !== null && el.matches('input, textarea, [contenteditable]');
}

/** Selects the contents of the element, replacing the current selection. Does nothing for null. */
export function selectContentsOf(el: Element | null): void {
  if (el === null) {
    return;
  }
  const selection = el.ownerDocument.getSelection();
  if (selection === null) {
    return;
  }
  const range = el.ownerDocument.createRange();
  range.selectNodeContents(el);
  selection.removeAllRanges();
  selection.addRange(range);
}

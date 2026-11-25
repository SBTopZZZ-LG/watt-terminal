import { useContext, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { flushSync } from 'react-dom';
import type { Ref } from 'react';
import { api } from '../lib/api';
import { copyText, readClipboardText } from '../lib/clipboard';
import { edgeIndex, menuFor, menuKey, menuPlacement, stepIndex, typeaheadIndex, withLayoutInspector } from '../lib/contextMenu';
import type { MenuItem } from '../lib/contextMenu';
import { fieldSelectedText, resolveMenu } from '../lib/contextMenuDom';
import type { MenuSnapshot, TabFacts, TerminalFacts, TextField } from '../lib/contextMenuDom';
import { reportError, tabs } from '../lib/tabManager';
import { isOwnedByChat } from '../lib/tabStrip';
import { selectContentsOf } from '../lib/selectAll';
import { ActiveTabContext } from '../lib/activeTab';
import type { Placement } from '../lib/tooltip';
import type { InputState } from '../lib/types';
import type { Terminal } from '@xterm/xterm';
import { pasteFromClipboard } from './TerminalView';

/** The menu's id. One menu at a time, so a constant id is enough. */
export const CONTEXT_MENU_ID = 'context-menu';

/** The id of a row, for aria-activedescendant. Index-based, so it is stable for one menu. */
export function contextMenuItemId(index: number): string {
  return `context-menu-item-${index}`;
}

export interface ContextMenuProps {
  /** The terminal's state, read when the menu opens (the agent's busy flag and the input state decide Clear and Paste). */
  terminal: { state: InputState; agentBusy: boolean; pasteRoutesToPty: boolean };
  /** Clears the terminal, as the Clear hotkey does (App passes the same route). */
  onClearTerminal: () => void;
  /** Opens the terminal find, as Ctrl+Shift+F does. */
  onFindTerminal: () => void;
  /** Opens the chat find, as Ctrl+F does in the agent panel. */
  onFindChat: () => void;
  /** The developer setting: true adds "Open layout inspector" to every menu the app opens. Read when a menu opens. */
  devtools: boolean;
  /** Restarts a terminal's shell, as the strip's tab menu asks (App runs the chat's restart steps). */
  onRestartTab: (id: string) => void;
  /** The active chat: the tab items say whether the terminal is visible to it, and the visibility item assigns to it. */
  chatId: string;
}

interface OpenMenu {
  snap: MenuSnapshot;
  items: MenuItem[];
  /** The highlighted item's index, or -1 for none (a mouse open starts with none). */
  index: number;
  /** Where the menu's top-left corner is wanted, in viewport px. */
  point: { x: number; y: number };
  /** Null until measured: the menu renders hidden first, then is placed before the first paint. */
  pos: Placement | null;
}

/** The terminal facts the resolver reads: the caller's, plus what the terminal itself says now. */
function terminalFacts(t: ContextMenuProps['terminal'], term: Terminal | null): TerminalFacts {
  return {
    ...t,
    altScreen: term !== null && term.buffer.active.type === 'alternate',
    hasSelection: term !== null && term.hasSelection(),
  };
}

/** What the tab registry says about a terminal, for its tab items. Null for an id with no terminal. */
function tabFactsOf(id: string, chatId: string): TabFacts | null {
  const tab = tabs.get(id);
  if (tab === undefined) {
    return null;
  }
  return { visible: isOwnedByChat(tab.getSnapshot().owner, chatId), canClose: tabs.getSnapshot().ids.length > 1 };
}

/** Shift+F10 and the Menu key open the menu from the keyboard. */
function isMenuKeyPress(e: KeyboardEvent): boolean {
  return e.key === 'ContextMenu' || (e.key === 'F10' && e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey);
}

/** Where a keyboard-opened menu goes: just under the focused element's left edge. */
function pointBelow(el: Element): { x: number; y: number } {
  const box = el.getBoundingClientRect();
  return { x: box.left, y: box.bottom };
}

/** Focus goes back to what had it: the terminal, or the element focused when the menu opened. */
function restoreFocus(snap: MenuSnapshot, term: Terminal | null): void {
  if (snap.ctx.region === 'terminal') {
    term?.focus();
    return;
  }
  if (snap.origin !== null && snap.origin.isConnected) {
    snap.origin.focus({ preventScroll: true });
  }
}

async function copyOrReport(text: string): Promise<void> {
  if (text === '') {
    return;
  }
  const ok = await copyText(text, { clipboard: navigator.clipboard, document });
  if (!ok) {
    reportError('Could not copy to the clipboard.');
  }
}

/**
 * Pastes the clipboard's text at the field's caret, replacing its selection. execCommand('insertText') takes the editing
 * path, so a controlled input (React) sees the change as typed text and fires its onChange.
 */
async function pasteIntoField(field: TextField): Promise<void> {
  const text = await readClipboardText(navigator.clipboard);
  if (text === null) {
    reportError('Could not read the clipboard.');
    return;
  }
  if (text === '') {
    return;
  }
  field.focus();
  if (!document.execCommand('insertText', false, text)) {
    reportError('Could not paste the text.');
  }
}

/** Runs a menu item. The menu has already closed and given focus back, so the field or the terminal is the target. */
async function runAction(id: MenuItem['id'], snap: MenuSnapshot, props: ContextMenuProps, term: Terminal | null): Promise<void> {
  const { texts } = snap;
  switch (id) {
    case 'copy':
      if (snap.ctx.region === 'field' && snap.field !== null) {
        return copyOrReport(fieldSelectedText(snap.field));
      }
      return copyOrReport(texts.selection);
    case 'copy-terminal':
      return copyOrReport(term?.getSelection() ?? '');
    case 'cut': {
      const field = snap.field;
      if (field === null) {
        return;
      }
      const text = fieldSelectedText(field);
      if (text === '') {
        return;
      }
      const ok = await copyText(text, { clipboard: navigator.clipboard, document });
      if (!ok) {
        reportError('Could not copy to the clipboard.');
        return;
      }
      field.focus();
      document.execCommand('delete');
      return;
    }
    case 'paste':
      if (snap.field === null) {
        return term === null ? undefined : pasteFromClipboard(term);
      }
      return pasteIntoField(snap.field);
    case 'select-all':
      if (snap.field !== null) {
        snap.field.select();
      } else if (snap.ctx.region === 'chat') {
        selectContentsOf(snap.transcript);
      } else {
        term?.selectAll();
      }
      return;
    case 'find-terminal':
      props.onFindTerminal();
      return;
    case 'find-chat':
      props.onFindChat();
      return;
    case 'clear':
      props.onClearTerminal();
      return;
    case 'scroll-bottom':
      term?.scrollToBottom();
      return;
    case 'copy-message':
      return copyOrReport(texts.message);
    case 'copy-code':
      return copyOrReport(texts.code);
    case 'copy-tool-input':
      return copyOrReport(texts.toolInput);
    case 'copy-tool-output':
      return copyOrReport(texts.toolOutput);
    case 'copy-tool-json':
      return copyOrReport(texts.toolJson);
    case 'copy-link':
      return copyOrReport(texts.link);
    case 'copy-chip':
      return copyOrReport(texts.chip);
    case 'tab-visibility':
      // The registry shows a refusal (a busy chat that would lose the terminal) and re-reads the owners.
      if (snap.tabId !== null) {
        await tabs.assign(snap.tabId, !snap.ctx.tabVisible, props.chatId);
      }
      return;
    case 'tab-restart':
      if (snap.tabId !== null) {
        props.onRestartTab(snap.tabId);
      }
      return;
    case 'tab-close':
      if (snap.tabId !== null) {
        await tabs.close(snap.tabId);
      }
      return;
    case 'open-layout-inspector':
      // The backend refuses unless the setting is on; a refusal is reported like any other failed action.
      return api.openDevtools();
    case 'separator':
      return;
  }
}

/**
 * The app's one context menu. Mounted once. A capture-phase contextmenu listener always cancels the native menu, then opens
 * the themed one for what was under the pointer (none where there are no items). The Menu key and Shift+F10 open it at the
 * focused element. Keys, the mouse and the close rules are in the listeners below; the rules are in lib/contextMenu.ts.
 */
export function ContextMenu(props: ContextMenuProps) {
  const [open, setOpenState] = useState<OpenMenu | null>(null);
  // The open menu, read by the listeners, which are registered once. Kept in step with the state by setOpen.
  const openRef = useRef<OpenMenu | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);
  const latest = useRef(props);
  // The active terminal's xterm. Read by the listeners, which are registered once, so it is kept in a ref too.
  const tab = useContext(ActiveTabContext);
  const latestTerm = useRef<Terminal | null>(tab?.term ?? null);
  useEffect(() => {
    latest.current = props;
    latestTerm.current = tab?.term ?? null;
  });

  const setOpen = (next: OpenMenu | null) => {
    openRef.current = next;
    setOpenState(next);
  };

  const show = (snap: MenuSnapshot, point: { x: number; y: number }, keyboard: boolean) => {
    // Read from latest: show runs from listeners registered once, so the props of the first render would stay.
    const items = withLayoutInspector(menuFor(snap.ctx), latest.current.devtools);
    if (items.length === 0) {
      return;
    }
    setOpen({ snap, items, index: keyboard ? edgeIndex(items, 'first') : -1, point, pos: null });
  };

  const dismiss = () => setOpen(null);

  const highlight = (index: number) => {
    const cur = openRef.current;
    if (cur !== null && index !== -1 && index !== cur.index) {
      setOpen({ ...cur, index });
    }
  };

  const hover = (index: number) => {
    const cur = openRef.current;
    if (cur !== null && !cur.items[index]?.disabled) {
      highlight(index);
    }
  };

  const closeAndRestore = () => {
    const cur = openRef.current;
    if (cur === null) {
      return;
    }
    setOpen(null);
    restoreFocus(cur.snap, latestTerm.current);
  };

  /** Closes the menu for a scroll, a resize or a window blur. Focus goes back to the origin when it is in the menu or on the body. */
  const dismissRestoring = () => {
    const cur = openRef.current;
    if (cur === null) {
      return;
    }
    const active = document.activeElement;
    const lost = active === null || active === document.body || (menuRef.current?.contains(active) ?? false);
    setOpen(null);
    if (lost) {
      restoreFocus(cur.snap, latestTerm.current);
    }
  };

  const activate = (index: number) => {
    const cur = openRef.current;
    const item = cur?.items[index];
    if (cur === null || item === undefined || item.separator || item.disabled) {
      return;
    }
    // The menu leaves the DOM before focus moves back and the action runs, so nothing that happens later (the menu's
    // removal) can move the selection the action makes, such as Select all in the chat.
    flushSync(() => setOpen(null));
    restoreFocus(cur.snap, latestTerm.current);
    runAction(item.id, cur.snap, latest.current, latestTerm.current).catch(reportError);
  };

  const handleKey = (e: KeyboardEvent, cur: OpenMenu) => {
    const step = menuKey(e.key, { shift: e.shiftKey, ctrl: e.ctrlKey, meta: e.metaKey, alt: e.altKey });
    if (step === null) {
      return;
    }
    switch (step.kind) {
      case 'close':
        closeAndRestore();
        return;
      case 'activate':
        activate(cur.index);
        return;
      case 'next':
        highlight(stepIndex(cur.items, cur.index, 1));
        return;
      case 'prev':
        highlight(stepIndex(cur.items, cur.index, -1));
        return;
      case 'first':
      case 'last':
        highlight(edgeIndex(cur.items, step.kind));
        return;
      case 'type':
        highlight(typeaheadIndex(cur.items, step.char, cur.index));
        return;
    }
  };

  // Placed once rendered hidden: the measure runs before the first paint, so the menu never shows in the wrong place.
  useLayoutEffect(() => {
    const menu = menuRef.current;
    if (open === null || open.pos !== null || menu === null) {
      return;
    }
    const pos = menuPlacement(open.point, { width: menu.offsetWidth, height: menu.offsetHeight }, { width: window.innerWidth, height: window.innerHeight });
    setOpen({ ...open, pos });
  }, [open]);

  // Once placed, the menu takes focus, so the keys reach it. Its own focus is what restoreFocus gives back.
  const placed = open !== null && open.pos !== null;
  useLayoutEffect(() => {
    if (placed) {
      menuRef.current?.focus({ preventScroll: true });
    }
  }, [placed]);

  // Always on: the native menu is cancelled, the themed one opens, and the keys open it or drive it while it is open.
  useEffect(() => {
    const onContextMenu = (e: MouseEvent) => {
      e.preventDefault();
      const prev = openRef.current;
      const origin = prev !== null ? prev.snap.origin : document.activeElement instanceof HTMLElement ? document.activeElement : null;
      dismiss();
      const target = e.target;
      if (!(target instanceof Element)) {
        return;
      }
      // With mouse tracking on, a right click in the terminal belongs to the app inside it. Shift keeps the menu.
      if (!e.shiftKey && target.closest('.xterm') !== null && (latestTerm.current?.modes.mouseTrackingMode ?? 'none') !== 'none') {
        return;
      }
      const snap = resolveMenu(target, terminalFacts(latest.current.terminal, latestTerm.current), origin, (id) => tabFactsOf(id, latest.current.chatId));
      if (snap !== null) {
        show(snap, { x: e.clientX, y: e.clientY }, false);
      }
    };
    const onKeyDown = (e: KeyboardEvent) => {
      const cur = openRef.current;
      if (cur !== null) {
        e.preventDefault();
        e.stopPropagation();
        handleKey(e, cur);
        return;
      }
      if (isMenuKeyPress(e)) {
        e.preventDefault();
        e.stopPropagation();
        const el = document.activeElement;
        if (!(el instanceof HTMLElement)) {
          return;
        }
        const snap = resolveMenu(el, terminalFacts(latest.current.terminal, latestTerm.current), el, (id) => tabFactsOf(id, latest.current.chatId));
        if (snap !== null) {
          show(snap, pointBelow(el), true);
        }
      }
    };
    document.addEventListener('contextmenu', onContextMenu, true);
    document.addEventListener('keydown', onKeyDown, true);
    return () => {
      document.removeEventListener('contextmenu', onContextMenu, true);
      document.removeEventListener('keydown', onKeyDown, true);
    };
    // The listeners read the latest props and open state through refs, so they are registered once.
  }, []);

  // While open: a press outside closes it without moving focus. A scroll, a window blur or a resize closes it and gives focus back.
  const isOpen = open !== null;
  useEffect(() => {
    if (!isOpen) {
      return;
    }
    const onMouseDown = (e: MouseEvent) => {
      if (!menuRef.current?.contains(e.target as Node)) {
        dismiss();
      }
    };
    document.addEventListener('mousedown', onMouseDown, true);
    document.addEventListener('scroll', dismissRestoring, { capture: true, passive: true });
    window.addEventListener('blur', dismissRestoring);
    window.addEventListener('resize', dismissRestoring);
    return () => {
      document.removeEventListener('mousedown', onMouseDown, true);
      document.removeEventListener('scroll', dismissRestoring, { capture: true });
      window.removeEventListener('blur', dismissRestoring);
      window.removeEventListener('resize', dismissRestoring);
    };
  }, [isOpen]);

  if (open === null) {
    return null;
  }
  return <ContextMenuBox ref={menuRef} items={open.items} index={open.index} pos={open.pos} onActivate={activate} onHover={hover} />;
}

export interface ContextMenuBoxProps {
  items: MenuItem[];
  /** The highlighted item, or -1. */
  index: number;
  /** Null while measuring: the menu is then hidden, so it cannot flash in the wrong place. */
  pos: Placement | null;
  onActivate: (index: number) => void;
  onHover: (index: number) => void;
  ref?: Ref<HTMLDivElement>;
}

/**
 * The menu's markup. Items are role=menuitem (aria-disabled when disabled), separators role=separator. A mousedown on the
 * menu is prevented, so clicking an item does not move focus or collapse a selection.
 */
export function ContextMenuBox({ items, index, pos, onActivate, onHover, ref }: ContextMenuBoxProps) {
  return (
    <div
      ref={ref}
      id={CONTEXT_MENU_ID}
      role="menu"
      aria-label="Context menu"
      tabIndex={-1}
      aria-activedescendant={index >= 0 ? contextMenuItemId(index) : undefined}
      className="context-menu"
      style={pos === null ? { left: 0, top: 0, visibility: 'hidden' } : { left: pos.x, top: pos.y }}
      onMouseDown={(e) => e.preventDefault()}
    >
      {items.map((item, i) =>
        item.separator ? (
          <div key={`sep-${i}`} role="separator" className="context-menu-sep" />
        ) : (
          <div
            key={item.id}
            id={contextMenuItemId(i)}
            role="menuitem"
            aria-disabled={item.disabled ? 'true' : undefined}
            className={`context-menu-item${i === index ? ' is-highlighted' : ''}${item.disabled ? ' is-disabled' : ''}`}
            onClick={() => onActivate(i)}
            onMouseEnter={() => onHover(i)}
          >
            <span className="context-menu-label">{item.label}</span>
            {item.shortcut !== undefined && <span className="context-menu-shortcut">{item.shortcut}</span>}
          </div>
        ),
      )}
    </div>
  );
}

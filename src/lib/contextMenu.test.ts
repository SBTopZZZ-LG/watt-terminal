import { describe, expect, it } from 'vitest';
import { edgeIndex, emptyContext, enabledIndices, menuFor, menuKey, menuPlacement, stepIndex, typeaheadIndex, withLayoutInspector } from './contextMenu';
import type { MenuContext, MenuItem, MenuRegion } from './contextMenu';
import type { InputState } from './types';

/** The items as short strings: the label, `(off)` when disabled, `---` for a separator. */
function summary(items: MenuItem[]): string[] {
  return items.map((i) => (i.separator ? '---' : i.label + (i.disabled ? ' (off)' : '')));
}

function ctx(region: MenuRegion, over: Partial<MenuContext> = {}): MenuContext {
  return { ...emptyContext(region), ...over };
}

const TERMINAL_PROMPT: Partial<MenuContext> = { inputState: 'prompt', pasteRoutesToPty: true };

describe('menuFor: terminal', () => {
  it('offers Copy, Paste, Select all, Find, then Clear and Scroll to bottom at an idle prompt', () => {
    expect(summary(menuFor(ctx('terminal', { ...TERMINAL_PROMPT, hasSelection: true }), false))).toEqual([
      'Copy',
      'Paste',
      'Select all',
      'Find in terminal',
      '---',
      'Clear terminal',
      'Scroll to bottom',
    ]);
  });

  it('disables Copy without an xterm selection', () => {
    const items = menuFor(ctx('terminal', TERMINAL_PROMPT), false);
    expect(items.find((i) => i.id === 'copy-terminal')?.disabled).toBe(true);
  });

  it('hides Paste when keys do not reach the PTY (an exited shell)', () => {
    const items = menuFor(ctx('terminal', { ...TERMINAL_PROMPT, pasteRoutesToPty: false }), false);
    expect(items.some((i) => i.id === 'paste')).toBe(false);
  });

  it('hides Clear unless the shell is at the prompt, the agent is idle and the screen is not the alternate one', () => {
    const states: InputState[] = ['booting', 'submitted', 'continuation', 'running', 'unresponsive', 'alt', 'raw', 'exited'];
    for (const inputState of states) {
      const items = menuFor(ctx('terminal', { ...TERMINAL_PROMPT, inputState }), false);
      expect(items.some((i) => i.id === 'clear')).toBe(false);
    }
    expect(menuFor(ctx('terminal', { ...TERMINAL_PROMPT, agentBusy: true }), false).some((i) => i.id === 'clear')).toBe(false);
    expect(menuFor(ctx('terminal', { ...TERMINAL_PROMPT, altScreen: true }), false).some((i) => i.id === 'clear')).toBe(false);
  });

  it('keeps Scroll to bottom when Clear is hidden, with no separator left dangling', () => {
    const items = menuFor(ctx('terminal', { ...TERMINAL_PROMPT, agentBusy: true }), false);
    expect(summary(items).slice(-2)).toEqual(['---', 'Scroll to bottom']);
  });

  it('hints the platform shortcuts, and gives Select all none in the terminal (Ctrl+A is the shell key)', () => {
    const mac = menuFor(ctx('terminal', { ...TERMINAL_PROMPT, hasSelection: true }), true);
    const win = menuFor(ctx('terminal', { ...TERMINAL_PROMPT, hasSelection: true }), false);
    expect(mac.find((i) => i.id === 'copy-terminal')?.shortcut).toBe('Cmd+C');
    expect(win.find((i) => i.id === 'copy-terminal')?.shortcut).toBe('Ctrl+Shift+C');
    expect(win.find((i) => i.id === 'clear')?.shortcut).toBe('Ctrl+Shift+K');
    expect(mac.find((i) => i.id === 'clear')?.shortcut).toBe('Cmd+K');
    expect(win.find((i) => i.id === 'find-terminal')?.shortcut).toBe('Ctrl+Shift+F');
    expect(win.find((i) => i.id === 'select-all')?.shortcut).toBeUndefined();
  });
});

describe('menuFor: text field', () => {
  it('offers Cut, Copy and Paste on a writable field, with Cut and Copy off without a selection', () => {
    expect(summary(menuFor(ctx('field', { writable: true }), false))).toEqual(['Cut (off)', 'Copy (off)', 'Paste', '---', 'Select all']);
  });

  it('enables Cut and Copy with a selection', () => {
    expect(summary(menuFor(ctx('field', { writable: true, hasSelection: true }), false))).toEqual(['Cut', 'Copy', 'Paste', '---', 'Select all']);
  });

  it('offers no Cut or Paste on a read-only field', () => {
    expect(summary(menuFor(ctx('field', { writable: false, hasSelection: true }), false))).toEqual(['Copy', '---', 'Select all']);
  });

  it('hints Ctrl+X, Ctrl+C, Ctrl+V and Ctrl+A off macOS', () => {
    const items = menuFor(ctx('field', { writable: true, hasSelection: true }), false);
    expect(items.map((i) => i.shortcut)).toEqual(['Ctrl+X', 'Ctrl+C', 'Ctrl+V', undefined, 'Ctrl+A']);
  });
});

describe('menuFor: chat text', () => {
  it('offers Copy (off without a selection), Select all and Find in chat over plain text', () => {
    expect(summary(menuFor(ctx('chat'), false))).toEqual(['Copy (off)', '---', 'Select all', 'Find in chat']);
  });

  it('enables Copy with a selection', () => {
    expect(summary(menuFor(ctx('chat', { hasSelection: true }), false))[0]).toBe('Copy');
  });

  it('adds Copy message over a message', () => {
    expect(summary(menuFor(ctx('chat', { message: true }), false))).toEqual(['Copy (off)', 'Copy message', '---', 'Select all', 'Find in chat']);
  });

  it('adds Copy code over a code block', () => {
    expect(summary(menuFor(ctx('chat', { code: true }), false))).toEqual(['Copy (off)', '---', 'Select all', 'Find in chat', '---', 'Copy code']);
  });

  it('adds Copy input, output and JSON over a tool row', () => {
    expect(summary(menuFor(ctx('chat', { tool: true }), false))).toEqual([
      'Copy (off)',
      '---',
      'Select all',
      'Find in chat',
      '---',
      'Copy input',
      'Copy output',
      'Copy JSON',
    ]);
  });

  it('adds Copy link over a link', () => {
    expect(summary(menuFor(ctx('chat', { link: true }), false))).toEqual(['Copy (off)', '---', 'Select all', 'Find in chat', '---', 'Copy link']);
  });

  it('puts every chat item in order when all apply', () => {
    const items = menuFor(ctx('chat', { message: true, code: true, tool: true, link: true, hasSelection: true }), false);
    expect(items.map((i) => i.id)).toEqual([
      'copy',
      'copy-message',
      'separator',
      'select-all',
      'find-chat',
      'separator',
      'copy-code',
      'copy-tool-input',
      'copy-tool-output',
      'copy-tool-json',
      'copy-link',
    ]);
  });
});

describe('menuFor: chip and none', () => {
  it('offers Copy value on a status chip', () => {
    expect(summary(menuFor(ctx('chip'), false))).toEqual(['Copy value']);
  });

  it('offers nothing where there is no region (dividers, empty chrome, buttons)', () => {
    expect(menuFor(ctx('none'), false)).toEqual([]);
  });

  it('never starts or ends with a separator, and never has two in a row', () => {
    const regions: MenuRegion[] = ['terminal', 'field', 'chat', 'chip', 'tab', 'none'];
    const flags: Partial<MenuContext>[] = [
      {},
      { hasSelection: true, writable: true, message: true, code: true, tool: true, link: true, pasteRoutesToPty: true },
      { agentBusy: true, inputState: 'running', altScreen: true, pasteRoutesToPty: false },
      { tabVisible: true, tabCanClose: true },
    ];
    for (const region of regions) {
      for (const f of flags) {
        const items = menuFor(ctx(region, f), false);
        if (items.length === 0) {
          continue;
        }
        expect(items[0].separator).toBeFalsy();
        expect(items[items.length - 1].separator).toBeFalsy();
        for (let i = 1; i < items.length; i++) {
          expect(items[i].separator && items[i - 1].separator).toBeFalsy();
        }
      }
    }
  });
});

describe('menuFor: terminal tab', () => {
  it('offers Make visible to agent, Restart, then Close, with a separator before Close', () => {
    expect(summary(menuFor(ctx('tab', { tabCanClose: true }), false))).toEqual(['Make visible to agent', 'Restart', '---', 'Close']);
  });

  it('says Hide from agent for a terminal the chat can see', () => {
    expect(summary(menuFor(ctx('tab', { tabVisible: true, tabCanClose: true }), false))).toEqual(['Hide from agent', 'Restart', '---', 'Close']);
  });

  it('says Move to this chat for a terminal another chat owns', () => {
    expect(summary(menuFor(ctx('tab', { tabOther: true, tabCanClose: true }), false))).toEqual(['Move to this chat', 'Restart', '---', 'Close']);
  });

  it('disables Close for the last terminal, and leaves Restart and the assignment enabled', () => {
    expect(summary(menuFor(ctx('tab', { tabCanClose: false }), false))).toEqual(['Make visible to agent', 'Restart', '---', 'Close (off)']);
  });

  it('gives the item ids the handler reads', () => {
    const ids = menuFor(ctx('tab', { tabCanClose: true }), false).map((i) => i.id);
    expect(ids).toEqual(['tab-visibility', 'tab-restart', 'separator', 'tab-close']);
  });
});

describe('enabledIndices, stepIndex, edgeIndex', () => {
  // Copy (off), Cut (on), ---, Paste (off), Select (on)
  const items: MenuItem[] = [
    { id: 'copy', label: 'Copy', disabled: true },
    { id: 'cut', label: 'Cut' },
    { id: 'separator', label: '', separator: true },
    { id: 'paste', label: 'Paste', disabled: true },
    { id: 'select-all', label: 'Select all' },
  ];

  it('lists the indices a keyboard or mouse can land on', () => {
    expect(enabledIndices(items)).toEqual([1, 4]);
  });

  it('steps over disabled items and separators, and wraps', () => {
    expect(stepIndex(items, 1, 1)).toBe(4);
    expect(stepIndex(items, 4, 1)).toBe(1);
    expect(stepIndex(items, 1, -1)).toBe(4);
  });

  it('starts from no selection: forward is the first item, back is the last', () => {
    expect(stepIndex(items, -1, 1)).toBe(1);
    expect(stepIndex(items, -1, -1)).toBe(4);
  });

  it('returns -1 when nothing is enabled', () => {
    const off: MenuItem[] = [{ id: 'copy', label: 'Copy', disabled: true }];
    expect(stepIndex(off, -1, 1)).toBe(-1);
    expect(edgeIndex(off, 'first')).toBe(-1);
  });

  it('finds the first and the last enabled item', () => {
    expect(edgeIndex(items, 'first')).toBe(1);
    expect(edgeIndex(items, 'last')).toBe(4);
  });
});

describe('typeaheadIndex', () => {
  const items: MenuItem[] = [
    { id: 'cut', label: 'Cut' },
    { id: 'copy', label: 'Copy', disabled: true },
    { id: 'paste', label: 'Paste' },
    { id: 'select-all', label: 'Select all' },
  ];

  it('finds the next enabled item that starts with the letter, case-insensitively', () => {
    expect(typeaheadIndex(items, 'p', -1)).toBe(2);
    expect(typeaheadIndex(items, 'S', -1)).toBe(3);
  });

  it('skips disabled items', () => {
    expect(typeaheadIndex(items, 'c', -1)).toBe(0);
    expect(typeaheadIndex(items, 'c', 0)).toBe(0);
  });

  it('returns -1 when no enabled item starts with the letter', () => {
    expect(typeaheadIndex(items, 'z', 0)).toBe(-1);
  });
});

describe('menuKey', () => {
  const none = { shift: false, ctrl: false, meta: false, alt: false };

  it('maps the movement and the choice keys', () => {
    expect(menuKey('ArrowDown', none)).toEqual({ kind: 'next' });
    expect(menuKey('ArrowUp', none)).toEqual({ kind: 'prev' });
    expect(menuKey('Home', none)).toEqual({ kind: 'first' });
    expect(menuKey('End', none)).toEqual({ kind: 'last' });
    expect(menuKey('Enter', none)).toEqual({ kind: 'activate' });
    expect(menuKey(' ', none)).toEqual({ kind: 'activate' });
    expect(menuKey('Escape', none)).toEqual({ kind: 'close' });
  });

  it('types ahead on a single printable character only', () => {
    expect(menuKey('a', none)).toEqual({ kind: 'type', char: 'a' });
    expect(menuKey('Shift', none)).toBeNull();
    expect(menuKey('a', { ...none, ctrl: true })).toBeNull();
  });

  it('swallows Tab and leaves modified movement keys to nothing', () => {
    expect(menuKey('Tab', none)).toBeNull();
    expect(menuKey('ArrowDown', { ...none, shift: true })).toBeNull();
    expect(menuKey('Enter', { ...none, meta: true })).toBeNull();
  });
});

describe('withLayoutInspector', () => {
  const base = menuFor(ctx('field', { writable: true, hasSelection: true }));

  it('adds the inspector after its own separator while the developer setting is on', () => {
    const items = withLayoutInspector(base, true);
    expect(summary(items).slice(-2)).toEqual(['---', 'Open layout inspector']);
    expect(items[items.length - 1].id).toBe('open-layout-inspector');
    expect(items.slice(0, base.length)).toEqual(base);
  });

  it('adds nothing while the setting is off', () => {
    expect(withLayoutInspector(base, false)).toEqual(base);
  });

  it('adds nothing to an empty menu, so a region with no items still opens no menu', () => {
    expect(withLayoutInspector([], true)).toEqual([]);
    expect(withLayoutInspector(menuFor(ctx('none')), true)).toEqual([]);
  });
});

describe('menuPlacement', () => {
  const viewport = { width: 1000, height: 800 };
  const menu = { width: 200, height: 100 };

  it('puts the top-left corner at the point, a little below it', () => {
    expect(menuPlacement({ x: 100, y: 50 }, menu, viewport)).toEqual({ x: 100, y: 58, side: 'below' });
  });

  it('keeps the menu inside the right edge', () => {
    expect(menuPlacement({ x: 950, y: 50 }, menu, viewport).x).toBe(792);
  });

  it('flips above the point when there is no room below', () => {
    expect(menuPlacement({ x: 100, y: 790 }, menu, viewport)).toEqual({ x: 100, y: 682, side: 'above' });
  });
});

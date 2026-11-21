// Native webview behaviour that does not belong in an app window: middle-click autoscroll, dragging an image or a link out
// of the page, and the browser's own accelerator keys (browserKeys.ts). The predicates are pure and tested. File drops are
// in dropGuard.ts, which is separate: it handles dragover and drop of files, not dragstart.

import { installBrowserKeyGuard } from './browserKeys';

/** The parts of an element that the guards read. A DOM Element satisfies this; an event target that is not an element does not. */
interface GuardNode {
  closest?: (selector: string) => unknown;
  ownerDocument?: Document | null;
}

function closestOf(node: GuardNode | null | undefined, selector: string): boolean {
  return typeof node?.closest === 'function' && node.closest(selector) != null;
}

/**
 * Middle-button autoscroll starts on mousedown, so it is cancelled there. The terminal is left alone: xterm takes its own
 * mouse events there.
 */
export function isMiddleClickAutoscroll(button: number, inTerminal: boolean): boolean {
  return button === 1 && !inTerminal;
}

/**
 * Whether a dragstart is an image or a link being dragged out of the page. A drag of selected text is not: when the
 * selection covers the drag's element (a selection that starts inside a link), the drag is the text, so it is left alone.
 */
export function isImageOrLinkDrag(node: GuardNode | null | undefined, selectionCoversNode: boolean): boolean {
  return closestOf(node, 'img, svg, a') && !selectionCoversNode;
}

/**
 * Installs the webview guards on the target (the document in the app): the browser key guard, middle-click autoscroll and
 * image or link drags. Returns the function that removes them.
 */
export function installWebviewGuards(target: Pick<EventTarget, 'addEventListener' | 'removeEventListener'> = document): () => void {
  const removeKeys = installBrowserKeyGuard(target);

  const onMouseDown = (e: Event) => {
    const ev = e as MouseEvent;
    if (isMiddleClickAutoscroll(ev.button, closestOf(ev.target as GuardNode, '.xterm'))) {
      ev.preventDefault();
    }
  };

  const onDragStart = (e: Event) => {
    const node = e.target as GuardNode | null;
    const selection = node?.ownerDocument?.getSelection() ?? null;
    const covers = selection !== null && !selection.isCollapsed && selection.containsNode(node as Node, true);
    if (isImageOrLinkDrag(node, covers)) {
      e.preventDefault();
    }
  };

  target.addEventListener('mousedown', onMouseDown, true);
  target.addEventListener('dragstart', onDragStart, true);
  return () => {
    removeKeys();
    target.removeEventListener('mousedown', onMouseDown, { capture: true });
    target.removeEventListener('dragstart', onDragStart, { capture: true });
  };
}

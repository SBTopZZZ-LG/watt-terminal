import { describe, expect, it } from 'vitest';
import { installWebviewGuards, isImageOrLinkDrag, isMiddleClickAutoscroll } from './webviewGuards';

/** A node that answers closest() the way an element does: it matches when the selector names one of its tags. */
function node(...tags: string[]) {
  return { closest: (selector: string) => (selector.split(',').some((s) => tags.includes(s.trim())) ? {} : null) };
}

describe('isMiddleClickAutoscroll', () => {
  it('cancels the middle button outside the terminal only', () => {
    expect(isMiddleClickAutoscroll(1, false)).toBe(true);
    expect(isMiddleClickAutoscroll(1, true)).toBe(false);
    expect(isMiddleClickAutoscroll(0, false)).toBe(false);
    expect(isMiddleClickAutoscroll(2, false)).toBe(false);
  });
});

describe('isImageOrLinkDrag', () => {
  it('cancels a drag that starts on an image, an svg or a link', () => {
    expect(isImageOrLinkDrag(node('img'), false)).toBe(true);
    expect(isImageOrLinkDrag(node('svg'), false)).toBe(true);
    expect(isImageOrLinkDrag(node('a'), false)).toBe(true);
  });

  it('leaves other drags alone, and a drag of selected text that covers the element', () => {
    expect(isImageOrLinkDrag(node('p'), false)).toBe(false);
    expect(isImageOrLinkDrag(null, false)).toBe(false);
    expect(isImageOrLinkDrag(node('a'), true)).toBe(false);
  });
});

describe('installWebviewGuards', () => {
  it('cancels a middle-button mousedown on a non-element target and leaves the left button alone', () => {
    const target = new EventTarget();
    const remove = installWebviewGuards(target);
    const middle = new Event('mousedown', { cancelable: true });
    Object.defineProperty(middle, 'button', { value: 1 });
    const left = new Event('mousedown', { cancelable: true });
    Object.defineProperty(left, 'button', { value: 0 });
    target.dispatchEvent(middle);
    target.dispatchEvent(left);
    expect(middle.defaultPrevented).toBe(true);
    expect(left.defaultPrevented).toBe(false);
    remove();
  });

  it('removes its listeners', () => {
    const target = new EventTarget();
    const remove = installWebviewGuards(target);
    remove();
    const middle = new Event('mousedown', { cancelable: true });
    Object.defineProperty(middle, 'button', { value: 1 });
    target.dispatchEvent(middle);
    expect(middle.defaultPrevented).toBe(false);
  });
});

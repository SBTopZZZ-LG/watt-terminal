import { describe, expect, it } from 'vitest';
import { carriesFiles, installDropGuard } from './dropGuard';

/** An event of the given type, with a DataTransfer of the given types (node has no DragEvent). */
function drag(type: string, types: string[] | null): Event {
  const event = new Event(type, { cancelable: true });
  Object.defineProperty(event, 'dataTransfer', { value: types === null ? null : { types } });
  return event;
}

describe('carriesFiles', () => {
  it('is true only when the drag lists Files', () => {
    expect(carriesFiles({ types: ['Files'] } as unknown as DataTransfer)).toBe(true);
    expect(carriesFiles({ types: ['text/plain', 'Files'] } as unknown as DataTransfer)).toBe(true);
    expect(carriesFiles({ types: ['text/plain', 'text/html'] } as unknown as DataTransfer)).toBe(false);
    expect(carriesFiles(null)).toBe(false);
    expect(carriesFiles(undefined)).toBe(false);
  });
});

describe('installDropGuard', () => {
  it('cancels dragover and drop of files, so the page does not navigate to them', () => {
    const target = new EventTarget();
    const remove = installDropGuard(target);
    const over = drag('dragover', ['Files']);
    const drop = drag('drop', ['Files']);
    target.dispatchEvent(over);
    target.dispatchEvent(drop);
    expect(over.defaultPrevented).toBe(true);
    expect(drop.defaultPrevented).toBe(true);
    remove();
  });

  it('leaves a drag of text or of page content alone', () => {
    const target = new EventTarget();
    const remove = installDropGuard(target);
    const text = drag('drop', ['text/plain']);
    const over = drag('dragover', ['text/plain', 'text/html']);
    target.dispatchEvent(text);
    target.dispatchEvent(over);
    expect(text.defaultPrevented).toBe(false);
    expect(over.defaultPrevented).toBe(false);
    remove();
  });

  it('removes its listeners', () => {
    const target = new EventTarget();
    const remove = installDropGuard(target);
    remove();
    const drop = drag('drop', ['Files']);
    target.dispatchEvent(drop);
    expect(drop.defaultPrevented).toBe(false);
  });

  it('a drag with no DataTransfer is left alone', () => {
    const target = new EventTarget();
    const remove = installDropGuard(target);
    const drop = drag('drop', null);
    target.dispatchEvent(drop);
    expect(drop.defaultPrevented).toBe(false);
    remove();
  });
});

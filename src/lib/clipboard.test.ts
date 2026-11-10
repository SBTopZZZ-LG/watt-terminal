import { describe, expect, it, vi } from 'vitest';
import { copyText } from './clipboard';
import type { CopyDocument, ClipboardEnv } from './clipboard';

/** The fake stands in for the parts of Document the fallback reads and calls. */
const asDocument = (doc: unknown) => doc as CopyDocument;

/** A document that records what the fallback did with its hidden textarea. */
function fakeDocument(execCommand: () => boolean = () => true) {
  const appended: unknown[] = [];
  const removed: unknown[] = [];
  const area = {
    value: '',
    style: { position: '', left: '', top: '', opacity: '' },
    attributes: {} as Record<string, string>,
    selected: false,
    setAttribute(name: string, value: string) {
      this.attributes[name] = value;
    },
    select() {
      this.selected = true;
    },
  };
  const doc = {
    createElement: vi.fn(() => area),
    body: {
      appendChild: vi.fn((node) => appended.push(node)),
      removeChild: vi.fn((node) => removed.push(node)),
    },
    execCommand: vi.fn(execCommand),
  };
  return { doc, area, appended, removed };
}

describe('copyText', () => {
  it('uses navigator.clipboard.writeText when it works, and does not touch the document', async () => {
    const writeText = vi.fn(async () => {});
    const { doc } = fakeDocument();
    const env: ClipboardEnv = { clipboard: { writeText }, document: asDocument(doc) };

    await expect(copyText('# hi', env)).resolves.toBe(true);
    expect(writeText).toHaveBeenCalledWith('# hi');
    expect(doc.createElement).not.toHaveBeenCalled();
    expect(doc.execCommand).not.toHaveBeenCalled();
  });

  it('falls back to a hidden, selected textarea when writeText throws', async () => {
    const writeText = vi.fn(async () => {
      throw new Error('NotAllowedError');
    });
    const { doc, area, appended, removed } = fakeDocument();

    await expect(copyText('**bold**', { clipboard: { writeText }, document: asDocument(doc) })).resolves.toBe(true);
    expect(writeText).toHaveBeenCalledWith('**bold**');
    expect(area.value).toBe('**bold**');
    expect(area.selected).toBe(true);
    expect(area.attributes).toEqual({ readonly: '' });
    expect(appended).toEqual([area]);
    expect(removed).toEqual([area]);
    expect(doc.execCommand).toHaveBeenCalledWith('copy');
  });

  it('falls back when navigator.clipboard is not provided at all', async () => {
    const { doc, area, removed } = fakeDocument();

    await expect(copyText('text', { clipboard: undefined, document: asDocument(doc) })).resolves.toBe(true);
    expect(doc.execCommand).toHaveBeenCalledWith('copy');
    expect(removed).toEqual([area]);
  });

  it('returns false when both the clipboard and the fallback fail', async () => {
    const writeText = vi.fn(async () => {
      throw new Error('denied');
    });
    const { doc, removed, area } = fakeDocument(() => false);

    await expect(copyText('text', { clipboard: { writeText }, document: asDocument(doc) })).resolves.toBe(false);
    // The textarea is still removed when the copy command reports failure.
    expect(removed).toEqual([area]);
  });

  it('returns false when execCommand throws as well', async () => {
    const writeText = vi.fn(async () => {
      throw new Error('denied');
    });
    const { doc, removed } = fakeDocument(() => {
      throw new Error('unsupported');
    });

    await expect(copyText('text', { clipboard: { writeText }, document: asDocument(doc) })).resolves.toBe(false);
    expect(removed).toHaveLength(1);
  });

  it('returns false when there is neither a clipboard nor a document', async () => {
    await expect(copyText('text', { clipboard: null, document: null })).resolves.toBe(false);
    await expect(copyText('text', {})).resolves.toBe(false);
  });
});

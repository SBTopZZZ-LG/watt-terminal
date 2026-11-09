import { describe, expect, it } from 'vitest';
import {
  clampFontSize,
  FONT_SIZE_DEFAULT,
  FONT_SIZE_KEY,
  FONT_SIZE_MAX,
  FONT_SIZE_MIN,
  loadFontSize,
  parseFontSize,
  saveFontSize,
  stepFontSize,
} from './zoom';
import type { SizeStorage } from './zoom';

/** A Map-backed storage. Its `throws` flag makes every call throw, like a denied or full localStorage. */
function fakeStorage(initial: Record<string, string> = {}, throws = false): SizeStorage & { data: Map<string, string> } {
  const data = new Map(Object.entries(initial));
  return {
    data,
    getItem(key) {
      if (throws) {
        throw new Error('denied');
      }
      return data.get(key) ?? null;
    },
    setItem(key, value) {
      if (throws) {
        throw new Error('quota');
      }
      data.set(key, value);
    },
  };
}

describe('zoom constants', () => {
  it('uses 13 as the default, 8..32 as the clamp, and the agreed storage key', () => {
    expect(FONT_SIZE_DEFAULT).toBe(13);
    expect(FONT_SIZE_MIN).toBe(8);
    expect(FONT_SIZE_MAX).toBe(32);
    expect(FONT_SIZE_KEY).toBe('agent-term.fontSize');
  });
});

describe('clampFontSize', () => {
  it('keeps values inside the range unchanged', () => {
    expect(clampFontSize(8)).toBe(8);
    expect(clampFontSize(13)).toBe(13);
    expect(clampFontSize(32)).toBe(32);
  });

  it('clamps below and above the range', () => {
    expect(clampFontSize(7)).toBe(8);
    expect(clampFontSize(0)).toBe(8);
    expect(clampFontSize(-5)).toBe(8);
    expect(clampFontSize(33)).toBe(32);
    expect(clampFontSize(500)).toBe(32);
  });

  it('gives the default for values that are not finite', () => {
    expect(clampFontSize(NaN)).toBe(13);
    expect(clampFontSize(Infinity)).toBe(13);
    expect(clampFontSize(-Infinity)).toBe(13);
  });
});

describe('stepFontSize', () => {
  it('steps by one in each direction', () => {
    expect(stepFontSize(13, 1)).toBe(14);
    expect(stepFontSize(13, -1)).toBe(12);
  });

  it('stops at the clamp instead of passing it', () => {
    expect(stepFontSize(32, 1)).toBe(32);
    expect(stepFontSize(8, -1)).toBe(8);
  });
});

describe('parseFontSize', () => {
  it('reads a valid integer string', () => {
    expect(parseFontSize('16')).toBe(16);
    expect(parseFontSize(' 9 ')).toBe(9);
  });

  it('clamps an integer outside the range', () => {
    expect(parseFontSize('99')).toBe(32);
    expect(parseFontSize('2')).toBe(8);
    expect(parseFontSize('-4')).toBe(8);
  });

  it('gives null for missing, empty and blank values', () => {
    expect(parseFontSize(null)).toBeNull();
    expect(parseFontSize(undefined)).toBeNull();
    expect(parseFontSize('')).toBeNull();
    expect(parseFontSize('   ')).toBeNull();
  });

  it('gives null for non-numeric values', () => {
    expect(parseFontSize('abc')).toBeNull();
    expect(parseFontSize('13px')).toBeNull();
    expect(parseFontSize('null')).toBeNull();
    expect(parseFontSize('{"size":13}')).toBeNull();
  });

  it('gives null for non-integers', () => {
    expect(parseFontSize('13.5')).toBeNull();
    expect(parseFontSize('Infinity')).toBeNull();
    expect(parseFontSize('NaN')).toBeNull();
  });
});

describe('loadFontSize', () => {
  it('reads the saved size', () => {
    expect(loadFontSize(fakeStorage({ [FONT_SIZE_KEY]: '18' }))).toBe(18);
  });

  it('gives the default when nothing is saved', () => {
    expect(loadFontSize(fakeStorage())).toBe(13);
  });

  it('gives the default for a bad saved value', () => {
    expect(loadFontSize(fakeStorage({ [FONT_SIZE_KEY]: 'big' }))).toBe(13);
    expect(loadFontSize(fakeStorage({ [FONT_SIZE_KEY]: '' }))).toBe(13);
  });

  it('gives the default when storage is missing', () => {
    expect(loadFontSize(null)).toBe(13);
  });

  it('gives the default when storage throws', () => {
    expect(loadFontSize(fakeStorage({ [FONT_SIZE_KEY]: '20' }, true))).toBe(13);
  });
});

describe('saveFontSize', () => {
  it('writes the size under the storage key', () => {
    const storage = fakeStorage();
    saveFontSize(17, storage);
    expect(storage.data.get(FONT_SIZE_KEY)).toBe('17');
  });

  it('writes the clamped size', () => {
    const storage = fakeStorage();
    saveFontSize(64, storage);
    expect(storage.data.get(FONT_SIZE_KEY)).toBe('32');
  });

  it('does nothing without storage', () => {
    expect(() => saveFontSize(14, null)).not.toThrow();
  });

  it('does not throw when storage throws', () => {
    expect(() => saveFontSize(14, fakeStorage({}, true))).not.toThrow();
  });

  it('round-trips through load', () => {
    const storage = fakeStorage();
    saveFontSize(21, storage);
    expect(loadFontSize(storage)).toBe(21);
  });
});

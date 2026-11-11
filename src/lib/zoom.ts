// Terminal font size: the default, the step, the clamp, and the saved value in localStorage. Pure apart from the storage,
// which is passed in, so tests use a fake. A missing, unreadable or invalid stored value falls back to the default.

export const FONT_SIZE_DEFAULT = 13;
export const FONT_SIZE_MIN = 8;
export const FONT_SIZE_MAX = 32;
export const FONT_SIZE_STEP = 1;
export const FONT_SIZE_KEY = 'watt.fontSize';
/** The key before the rename to Watt. Read only while FONT_SIZE_KEY is absent (see readWithLegacyKey). */
export const FONT_SIZE_LEGACY_KEY = 'agent-term.fontSize';

/** The part of Storage the font size needs. */
export interface SizeStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/**
 * Reads `key`. When it is absent and `legacyKey` holds a value, that value is copied under `key`, so later reads find it
 * there, and it is returned. The old key is left in place. Null when neither is set. A getItem that throws propagates to
 * the caller; a setItem that throws is ignored, and the legacy value is still returned.
 */
export function readWithLegacyKey(storage: SizeStorage, key: string, legacyKey: string): string | null {
  const current = storage.getItem(key);
  if (current !== null) {
    return current;
  }
  const legacy = storage.getItem(legacyKey);
  if (legacy !== null) {
    try {
      storage.setItem(key, legacy);
    } catch {
      // Not copied. The legacy value still applies, and the copy is tried again on the next read.
    }
  }
  return legacy;
}

/** Keeps a size inside [FONT_SIZE_MIN, FONT_SIZE_MAX]. A value that is not a finite number gives the default. */
export function clampFontSize(size: number): number {
  if (!Number.isFinite(size)) {
    return FONT_SIZE_DEFAULT;
  }
  return Math.min(FONT_SIZE_MAX, Math.max(FONT_SIZE_MIN, size));
}

/** The size one step bigger (dir 1) or smaller (dir -1), clamped. */
export function stepFontSize(size: number, dir: 1 | -1): number {
  return clampFontSize(size + dir * FONT_SIZE_STEP);
}

/**
 * Reads a stored size. Null, empty, non-numeric or non-integer values give null (the caller uses the default). Integers
 * outside the range are clamped.
 */
export function parseFontSize(raw: string | null | undefined): number | null {
  if (typeof raw !== 'string' || raw.trim() === '') {
    return null;
  }
  const n = Number(raw);
  if (!Number.isInteger(n)) {
    return null;
  }
  return clampFontSize(n);
}

/** The saved size, or the default. A throwing storage (private mode, denied access) gives the default. */
export function loadFontSize(storage: SizeStorage | null): number {
  if (storage === null) {
    return FONT_SIZE_DEFAULT;
  }
  try {
    return parseFontSize(readWithLegacyKey(storage, FONT_SIZE_KEY, FONT_SIZE_LEGACY_KEY)) ?? FONT_SIZE_DEFAULT;
  } catch {
    return FONT_SIZE_DEFAULT;
  }
}

/** Saves the size. A throwing storage is ignored: the size still applies for this session. */
export function saveFontSize(size: number, storage: SizeStorage | null): void {
  if (storage === null) {
    return;
  }
  try {
    storage.setItem(FONT_SIZE_KEY, String(clampFontSize(size)));
  } catch {
    // Not persisted. The zoom still works until the next launch.
  }
}

/**
 * The browser's localStorage, or null when there is none. Reading the global can itself throw in some embedded
 * contexts, so the access is wrapped.
 */
export function defaultSizeStorage(): SizeStorage | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage;
  } catch {
    return null;
  }
}

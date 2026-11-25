// Per-conversation UI memory in localStorage: the unsent input draft, and the follow-up prompts shown after the last reply.
// All of it lives under one key, so a read or a write covers a change. The rules are pure over a cache value; the storage is
// passed in (tests use a fake, see zoom.ts). Nothing here throws: a missing, unreadable or invalid value reads as empty, and a
// failed write is dropped. This is a convenience only; the transcript and the backend are the truth.

import type { SizeStorage } from './zoom';

export const CACHE_KEY = 'watt.convCache';
export const CACHE_VERSION = 1;
/** Entries kept. Past it, the entries written longest ago are dropped. */
export const CACHE_MAX_ENTRIES = 100;
/** The draft key while the conversation has no id yet (the status has not been read). */
export const NEW_DRAFT_KEY = 'new';

/** The follow-up prompts for a reply, with the signature of the transcript tail they followed (see transcriptSig). */
export interface SavedSuggestions {
  items: string[];
  sig: string;
}

export interface CacheEntry {
  /** The unsent text. Absent when empty. */
  draft?: string;
  /** Absent when none is saved. */
  suggestions?: SavedSuggestions;
  /** When the entry was last written (ms). The oldest are evicted first. */
  at: number;
}

export interface ConvCache {
  v: typeof CACHE_VERSION;
  entries: Record<string, CacheEntry>;
}

/** The part of the transcript the signature reads: any item with a kind, and a text when it has one. */
export interface SigItem {
  kind: string;
  text?: string;
}

export function emptyCache(): ConvCache {
  return { v: CACHE_VERSION, entries: {} };
}

function isRecord(x: unknown): x is Record<string, unknown> {
  return typeof x === 'object' && x !== null && !Array.isArray(x);
}

function own(cache: ConvCache, key: string): CacheEntry | undefined {
  return Object.prototype.hasOwnProperty.call(cache.entries, key) ? cache.entries[key] : undefined;
}

/** A stored entry, with the parts that do not check out dropped. Null when nothing usable is left. */
function parseEntry(raw: unknown): CacheEntry | null {
  if (!isRecord(raw) || typeof raw.at !== 'number' || !Number.isFinite(raw.at)) {
    return null;
  }
  const entry: CacheEntry = { at: raw.at };
  if (typeof raw.draft === 'string' && raw.draft !== '') {
    entry.draft = raw.draft;
  }
  const s = raw.suggestions;
  if (isRecord(s) && typeof s.sig === 'string' && Array.isArray(s.items) && s.items.length > 0 && s.items.every((x) => typeof x === 'string')) {
    entry.suggestions = { items: [...(s.items as string[])], sig: s.sig };
  }
  return entry.draft !== undefined || entry.suggestions !== undefined ? entry : null;
}

/** Keeps the newest CACHE_MAX_ENTRIES by `at`. Among equal times, the key stored later (in insertion order) is newer. */
export function prune(cache: ConvCache): ConvCache {
  const keys = Object.keys(cache.entries);
  if (keys.length <= CACHE_MAX_ENTRIES) {
    return cache;
  }
  const newest = keys
    .map((key, index) => ({ key, index, at: cache.entries[key].at }))
    .sort((a, b) => b.at - a.at || b.index - a.index)
    .slice(0, CACHE_MAX_ENTRIES)
    .map((x) => x.key);
  const keep = new Set(newest);
  const entries: Record<string, CacheEntry> = {};
  for (const key of keys) {
    if (keep.has(key)) {
      entries[key] = cache.entries[key];
    }
  }
  return { v: CACHE_VERSION, entries };
}

/** The cache as stored. Anything that is not the current version's shape reads as empty; bad entries are dropped one by one. */
export function parseCache(raw: string | null): ConvCache {
  if (raw === null) {
    return emptyCache();
  }
  let data: unknown;
  try {
    data = JSON.parse(raw);
  } catch {
    return emptyCache();
  }
  if (!isRecord(data) || data.v !== CACHE_VERSION || !isRecord(data.entries)) {
    return emptyCache();
  }
  const entries: Record<string, CacheEntry> = {};
  for (const [key, value] of Object.entries(data.entries)) {
    // A `__proto__` key would replace the object's prototype on assignment. No conversation has that id.
    if (key === '__proto__') {
      continue;
    }
    const entry = parseEntry(value);
    if (entry !== null) {
      entries[key] = entry;
    }
  }
  return prune({ v: CACHE_VERSION, entries });
}

/** Reads the saved cache. A missing or throwing storage gives an empty cache. */
export function readCache(storage: SizeStorage | null): ConvCache {
  if (storage === null) {
    return emptyCache();
  }
  try {
    return parseCache(storage.getItem(CACHE_KEY));
  } catch {
    return emptyCache();
  }
}

/** Saves the cache. Returns false when it could not be saved (no storage, quota, or a throwing storage). */
export function writeCache(storage: SizeStorage | null, cache: ConvCache): boolean {
  if (storage === null) {
    return false;
  }
  try {
    storage.setItem(CACHE_KEY, JSON.stringify(prune(cache)));
    return true;
  } catch {
    return false;
  }
}

/** Reads the cache, applies one pure change, and writes it back. */
export function updateCache(storage: SizeStorage | null, change: (cache: ConvCache) => ConvCache): void {
  if (storage === null) {
    return;
  }
  writeCache(storage, change(readCache(storage)));
}

/** Sets or removes an entry, and keeps the cache within its cap. A null entry removes it. */
function put(cache: ConvCache, key: string, entry: CacheEntry | null): ConvCache {
  const entries = { ...cache.entries };
  if (entry === null) {
    delete entries[key];
  } else {
    entries[key] = entry;
  }
  return prune({ v: CACHE_VERSION, entries });
}

/** An entry is removed when neither its draft nor its suggestions remain. */
function settle(cache: ConvCache, key: string, entry: CacheEntry): ConvCache {
  return put(cache, key, entry.draft === undefined && entry.suggestions === undefined ? null : entry);
}

// ---- drafts ----

export function getDraft(cache: ConvCache, key: string): string {
  return own(cache, key)?.draft ?? '';
}

/** Sets the draft for `key`. Empty text removes it. The entry's suggestions are kept. */
export function withDraft(cache: ConvCache, key: string, text: string, now: number): ConvCache {
  const prev = own(cache, key);
  const entry: CacheEntry = { at: now };
  if (prev?.suggestions !== undefined) {
    entry.suggestions = prev.suggestions;
  }
  if (text !== '') {
    entry.draft = text;
  }
  return settle(cache, key, entry);
}

/** Removes the draft for `key` only when it is still `sent`: text typed since then stays. */
export function withoutDraftIfSame(cache: ConvCache, key: string, sent: string, now: number): ConvCache {
  return getDraft(cache, key) === sent ? withDraft(cache, key, '', now) : cache;
}

/** Where the input's draft goes when the conversation key changes. `save` is the old key's text (null when none is to be kept), `load` the new key's draft. */
export interface DraftSwitch {
  save: { key: string; text: string } | null;
  load: string;
}

/**
 * The draft handling of a key change. `text` is the input's text, which belongs to `prevKey`. `discardPrev` is set when the
 * conversation was deleted: its text is then dropped, not saved.
 */
export function draftOnKeyChange(cache: ConvCache, prevKey: string, nextKey: string, text: string, discardPrev: boolean): DraftSwitch {
  return {
    save: prevKey === nextKey || discardPrev ? null : { key: prevKey, text },
    load: getDraft(cache, nextKey),
  };
}

// ---- suggestions ----

/** The result of looking up saved suggestions: the items when the signature matches, 'stale' when an entry exists for another tail, null when none. */
export type SavedLookup = { items: string[] } | 'stale' | null;

export function lookupSuggestions(cache: ConvCache, key: string, sig: string): SavedLookup {
  const saved = own(cache, key)?.suggestions;
  if (saved === undefined) {
    return null;
  }
  return saved.sig === sig ? { items: [...saved.items] } : 'stale';
}

/** Saves the suggestions for `key` against the transcript tail `sig`. No items removes them. */
export function withSuggestions(cache: ConvCache, key: string, items: string[], sig: string, now: number): ConvCache {
  const prev = own(cache, key);
  const entry: CacheEntry = { at: now };
  if (prev?.draft !== undefined) {
    entry.draft = prev.draft;
  }
  if (items.length > 0) {
    entry.suggestions = { items: [...items], sig };
  }
  return settle(cache, key, entry);
}

export function withoutSuggestions(cache: ConvCache, key: string, now: number): ConvCache {
  return own(cache, key)?.suggestions === undefined ? cache : withSuggestions(cache, key, [], '', now);
}

/** Drops the suggestions of every entry (the setting was turned off). Drafts stay. */
export function withoutAllSuggestions(cache: ConvCache, now: number): ConvCache {
  let next = cache;
  for (const key of Object.keys(cache.entries)) {
    next = withoutSuggestions(next, key, now);
  }
  return next;
}

// ---- transcript signature ----

function fnv1a(text: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(16);
}

/**
 * A signature of the conversation's tail: the user messages sent since the last agent message, and the text of that agent
 * message (trimmed, hashed with its length). Only those count, so the live transcript (which also holds notices, tool rows
 * and streamed items) and the one rebuilt from the saved messages (itemsFromMessages) give the same value for the same chat.
 * Blank agent text is not a message on either side. The count stops at the last agent message, not at the start, so
 * older messages loaded by paging (see transcriptPaging.ts) do not change it.
 */
export function transcriptSig(items: readonly SigItem[]): string {
  let since = 0;
  let last: string | null = null;
  for (const item of items) {
    if (item.kind === 'user') {
      since += 1;
    } else if (item.kind === 'agent' && typeof item.text === 'string' && item.text.trim() !== '') {
      since = 0;
      last = item.text.trim();
    }
  }
  if (last === null) {
    return `${since}:-`;
  }
  return `${since}:${last.length}:${fnv1a(last)}`;
}

// ---- storage-level operations (used by the UI) ----

/** The saved draft for `key`, or ''. */
export function loadDraft(storage: SizeStorage | null, key: string): string {
  return getDraft(readCache(storage), key);
}

export function saveDraft(storage: SizeStorage | null, key: string, text: string): void {
  updateCache(storage, (c) => withDraft(c, key, text, Date.now()));
}

export function clearDraftIfSame(storage: SizeStorage | null, key: string, sent: string): void {
  updateCache(storage, (c) => withoutDraftIfSame(c, key, sent, Date.now()));
}

export function saveSuggestions(storage: SizeStorage | null, key: string, items: string[], sig: string): void {
  updateCache(storage, (c) => withSuggestions(c, key, items, sig, Date.now()));
}

/** The saved suggestions for `key` when they follow the tail `sig`. A stale entry is left in place for the caller to decide. */
export function loadSuggestions(storage: SizeStorage | null, key: string, sig: string): SavedLookup {
  return lookupSuggestions(readCache(storage), key, sig);
}

export function forgetSuggestions(storage: SizeStorage | null, key: string): void {
  updateCache(storage, (c) => withoutSuggestions(c, key, Date.now()));
}

export function forgetAllSuggestions(storage: SizeStorage | null): void {
  updateCache(storage, (c) => withoutAllSuggestions(c, Date.now()));
}

/** The conversation was deleted: its draft and suggestions go. */
export function forgetConversation(storage: SizeStorage | null, key: string): void {
  updateCache(storage, (c) => put(c, key, null));
}

/** Every conversation was deleted: the whole cache goes. */
export function forgetEverything(storage: SizeStorage | null): void {
  writeCache(storage, emptyCache());
}

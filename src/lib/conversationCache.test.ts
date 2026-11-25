import { describe, expect, it } from 'vitest';
import {
  CACHE_KEY,
  CACHE_MAX_ENTRIES,
  CACHE_VERSION,
  NEW_DRAFT_KEY,
  clearDraftIfSame,
  draftOnKeyChange,
  emptyCache,
  forgetAllSuggestions,
  forgetConversation,
  forgetEverything,
  forgetSuggestions,
  getDraft,
  loadDraft,
  loadSuggestions,
  lookupSuggestions,
  parseCache,
  prune,
  readCache,
  saveDraft,
  saveSuggestions,
  transcriptSig,
  withDraft,
  withSuggestions,
  withoutAllSuggestions,
  withoutDraftIfSame,
  withoutSuggestions,
  writeCache,
} from './conversationCache';
import type { SizeStorage } from './zoom';
import { itemsFromMessages } from './historyView';
import { EMPTY_TRANSCRIPT, transcriptReducer } from './transcript';
import type { TranscriptAction } from './transcript';

/** A fake of the localStorage part the cache uses. Writes or reads can be made to throw, as a quota or a denied storage does. */
function fakeStorage(init: Record<string, string> = {}, opts: { failWrite?: boolean; failRead?: boolean } = {}) {
  const data = new Map(Object.entries(init));
  const storage: SizeStorage & { data: Map<string, string> } = {
    data,
    getItem(key) {
      if (opts.failRead) {
        throw new Error('denied');
      }
      return data.get(key) ?? null;
    },
    setItem(key, value) {
      if (opts.failWrite) {
        throw new Error('quota');
      }
      data.set(key, value);
    },
  };
  return storage;
}

const NOW = 1_000;

describe('parseCache: the tolerant reader', () => {
  it('reads nothing as empty: missing, empty, not JSON, JSON that is not an object', () => {
    for (const raw of [null, '', 'not json', '[]', '"text"', 'null', '42']) {
      expect(parseCache(raw)).toEqual(emptyCache());
    }
  });

  it('reads a wrong version, or entries that are not an object, as empty', () => {
    expect(parseCache(JSON.stringify({ v: 2, entries: { a: { at: 1, draft: 'x' } } }))).toEqual(emptyCache());
    expect(parseCache(JSON.stringify({ v: CACHE_VERSION }))).toEqual(emptyCache());
    expect(parseCache(JSON.stringify({ v: CACHE_VERSION, entries: [] }))).toEqual(emptyCache());
  });

  it('drops bad entries one by one and keeps the good ones', () => {
    const raw = JSON.stringify({
      v: 1,
      entries: {
        good: { at: 5, draft: 'hello' },
        noTime: { draft: 'x' },
        badTime: { at: 'soon', draft: 'x' },
        badDraft: { at: 6, draft: 7 },
        badSugg: { at: 7, suggestions: { items: [1, 2], sig: 'a' } },
        empty: { at: 8, draft: '' },
        nul: null,
      },
    });
    expect(parseCache(raw).entries).toEqual({ good: { at: 5, draft: 'hello' } });
  });

  it('keeps a valid draft when its suggestions are invalid', () => {
    const raw = JSON.stringify({ v: 1, entries: { a: { at: 1, draft: 'keep', suggestions: { items: 'nope', sig: 's' } } } });
    expect(parseCache(raw).entries).toEqual({ a: { at: 1, draft: 'keep' } });
  });

  it('ignores a __proto__ key instead of letting it change the prototype', () => {
    const raw = '{"v":1,"entries":{"__proto__":{"at":1,"draft":"x"},"ok":{"at":2,"draft":"y"}}}';
    const cache = parseCache(raw);
    expect(Object.keys(cache.entries)).toEqual(['ok']);
    expect(Object.getPrototypeOf(cache.entries)).toBe(Object.prototype);
  });

  it('a stored value that throws on read, or a storage that is missing, reads as empty', () => {
    expect(readCache(fakeStorage({}, { failRead: true }))).toEqual(emptyCache());
    expect(readCache(null)).toEqual(emptyCache());
  });
});

describe('writeCache and the storage operations', () => {
  it('round-trips through the single key', () => {
    const storage = fakeStorage();
    expect(writeCache(storage, withDraft(emptyCache(), 'c1', 'draft one', NOW))).toBe(true);
    expect([...storage.data.keys()]).toEqual([CACHE_KEY]);
    expect(loadDraft(storage, 'c1')).toBe('draft one');
  });

  it('a write that throws (quota) returns false and keeps the old value readable', () => {
    const storage = fakeStorage({}, { failWrite: true });
    expect(writeCache(storage, withDraft(emptyCache(), 'c1', 'x', NOW))).toBe(false);
    expect(() => saveDraft(storage, 'c1', 'x')).not.toThrow();
    expect(loadDraft(storage, 'c1')).toBe('');
  });

  it('a missing storage is a no-op everywhere', () => {
    expect(writeCache(null, emptyCache())).toBe(false);
    expect(() => saveDraft(null, 'c1', 'x')).not.toThrow();
    expect(loadDraft(null, 'c1')).toBe('');
    expect(loadSuggestions(null, 'c1', 's')).toBeNull();
  });

  it('a bad stored value is replaced by the next write', () => {
    const storage = fakeStorage({ [CACHE_KEY]: '{garbage' });
    saveDraft(storage, 'c1', 'fresh');
    expect(loadDraft(storage, 'c1')).toBe('fresh');
  });
});

describe('the cap and eviction', () => {
  it('keeps the newest CACHE_MAX_ENTRIES, evicting the oldest by last write', () => {
    let cache = emptyCache();
    const total = CACHE_MAX_ENTRIES + 5;
    for (let i = 0; i < total; i++) {
      cache = withDraft(cache, `c${i}`, `text ${i}`, 100 + i);
    }
    expect(Object.keys(cache.entries)).toHaveLength(CACHE_MAX_ENTRIES);
    for (let i = 0; i < 5; i++) {
      expect(getDraft(cache, `c${i}`)).toBe('');
    }
    expect(getDraft(cache, `c${total - 1}`)).toBe(`text ${total - 1}`);
  });

  it('among equal times, the entry stored later is the newer one', () => {
    let cache = emptyCache();
    for (let i = 0; i <= CACHE_MAX_ENTRIES; i++) {
      cache = withDraft(cache, `k${i}`, 'x', 7);
    }
    expect(getDraft(cache, 'k0')).toBe('');
    expect(getDraft(cache, `k${CACHE_MAX_ENTRIES}`)).toBe('x');
  });

  it('a read of an over-cap stored value is capped too', () => {
    const entries: Record<string, { at: number; draft: string }> = {};
    for (let i = 0; i < CACHE_MAX_ENTRIES + 3; i++) {
      entries[`c${i}`] = { at: i, draft: 'x' };
    }
    const cache = parseCache(JSON.stringify({ v: 1, entries }));
    expect(Object.keys(cache.entries)).toHaveLength(CACHE_MAX_ENTRIES);
    expect(cache.entries.c0).toBeUndefined();
  });

  it('prune leaves a cache under the cap as it is', () => {
    const cache = withDraft(emptyCache(), 'a', 'x', NOW);
    expect(prune(cache)).toBe(cache);
  });
});

describe('drafts', () => {
  it('each key has its own draft', () => {
    let cache = withDraft(emptyCache(), 'a', 'alpha', NOW);
    cache = withDraft(cache, 'b', 'beta', NOW);
    expect(getDraft(cache, 'a')).toBe('alpha');
    expect(getDraft(cache, 'b')).toBe('beta');
    expect(getDraft(cache, 'c')).toBe('');
  });

  it('empty text removes the entry when nothing else is saved for it', () => {
    const cache = withDraft(withDraft(emptyCache(), 'a', 'x', NOW), 'a', '', NOW + 1);
    expect(cache.entries).toEqual({});
  });

  it('a draft is removed only by an empty write or a removal, not by writes to another key', () => {
    let cache = withDraft(emptyCache(), 'a', 'keep', NOW);
    cache = withDraft(cache, 'b', 'other', NOW);
    cache = withDraft(cache, 'b', '', NOW);
    expect(getDraft(cache, 'a')).toBe('keep');
  });

  it('withoutDraftIfSame removes only the text that was sent', () => {
    const cache = withDraft(emptyCache(), 'a', 'sent text', NOW);
    expect(getDraft(withoutDraftIfSame(cache, 'a', 'sent text', NOW), 'a')).toBe('');
    expect(getDraft(withoutDraftIfSame(cache, 'a', 'something typed later', NOW), 'a')).toBe('sent text');
  });

  it('clearDraftIfSame works on storage', () => {
    const storage = fakeStorage();
    saveDraft(storage, 'a', 'one');
    clearDraftIfSame(storage, 'a', 'two');
    expect(loadDraft(storage, 'a')).toBe('one');
    clearDraftIfSame(storage, 'a', 'one');
    expect(loadDraft(storage, 'a')).toBe('');
    expect(storage.data.get(CACHE_KEY)).toBe(JSON.stringify({ v: 1, entries: {} }));
  });

  it('draftOnKeyChange saves the old text under the old key and loads the new draft', () => {
    const cache = withDraft(emptyCache(), 'b', 'beta draft', NOW);
    expect(draftOnKeyChange(cache, 'a', 'b', 'typed in a', false)).toEqual({ save: { key: 'a', text: 'typed in a' }, load: 'beta draft' });
  });

  it('draftOnKeyChange loads the empty string for a key with no draft', () => {
    expect(draftOnKeyChange(emptyCache(), 'a', 'z', 'x', false)).toEqual({ save: { key: 'a', text: 'x' }, load: '' });
  });

  it('draftOnKeyChange saves nothing when the key did not change, or when the old conversation was deleted', () => {
    expect(draftOnKeyChange(emptyCache(), 'a', 'a', 'x', false).save).toBeNull();
    expect(draftOnKeyChange(emptyCache(), 'a', NEW_DRAFT_KEY, 'x', true).save).toBeNull();
  });

  it('the text of the "new" key is saved and loaded like any other key', () => {
    const storage = fakeStorage();
    saveDraft(storage, NEW_DRAFT_KEY, 'before the id');
    expect(loadDraft(storage, NEW_DRAFT_KEY)).toBe('before the id');
  });
});

describe('suggestions', () => {
  const ITEMS = ['Run the tests', 'Show the diff'];

  it('a saved set is returned for the same tail signature', () => {
    const cache = withSuggestions(emptyCache(), 'a', ITEMS, 'sig-1', NOW);
    expect(lookupSuggestions(cache, 'a', 'sig-1')).toEqual({ items: ITEMS });
  });

  it('a different tail is stale; an unknown key is null', () => {
    const cache = withSuggestions(emptyCache(), 'a', ITEMS, 'sig-1', NOW);
    expect(lookupSuggestions(cache, 'a', 'sig-2')).toBe('stale');
    expect(lookupSuggestions(cache, 'b', 'sig-1')).toBeNull();
  });

  it('suggestions are per key: another conversation does not see them', () => {
    const cache = withSuggestions(emptyCache(), 'a', ITEMS, 'sig-1', NOW);
    expect(lookupSuggestions(cache, 'b', 'sig-1')).toBeNull();
  });

  it('a lookup result is a copy, so the caller cannot change the saved list', () => {
    const cache = withSuggestions(emptyCache(), 'a', ITEMS, 'sig-1', NOW);
    const got = lookupSuggestions(cache, 'a', 'sig-1');
    if (got === null || got === 'stale') {
      throw new Error('expected a match');
    }
    got.items.push('mutated');
    expect(lookupSuggestions(cache, 'a', 'sig-1')).toEqual({ items: ITEMS });
  });

  it('no items removes the suggestions, and the entry when there is no draft', () => {
    const cache = withoutSuggestions(withSuggestions(emptyCache(), 'a', ITEMS, 'sig-1', NOW), 'a', NOW + 1);
    expect(cache.entries).toEqual({});
  });

  it('writing a draft keeps the suggestions, and writing suggestions keeps the draft', () => {
    let cache = withSuggestions(emptyCache(), 'a', ITEMS, 'sig-1', NOW);
    cache = withDraft(cache, 'a', 'half typed', NOW);
    expect(lookupSuggestions(cache, 'a', 'sig-1')).toEqual({ items: ITEMS });
    cache = withSuggestions(cache, 'a', [], '', NOW);
    expect(getDraft(cache, 'a')).toBe('half typed');
    expect(lookupSuggestions(cache, 'a', '')).toBeNull();
  });

  it('withoutAllSuggestions drops every saved set and keeps the drafts', () => {
    let cache = withSuggestions(emptyCache(), 'a', ITEMS, 'sig-1', NOW);
    cache = withSuggestions(cache, 'b', ITEMS, 'sig-2', NOW);
    cache = withDraft(cache, 'b', 'draft b', NOW);
    const next = withoutAllSuggestions(cache, NOW + 1);
    expect(lookupSuggestions(next, 'a', 'sig-1')).toBeNull();
    expect(lookupSuggestions(next, 'b', 'sig-2')).toBeNull();
    expect(getDraft(next, 'b')).toBe('draft b');
    expect(Object.keys(next.entries)).toEqual(['b']);
  });

  it('storage-level save, load (match and stale) and forget', () => {
    const storage = fakeStorage();
    saveSuggestions(storage, 'a', ITEMS, 'sig-1');
    expect(loadSuggestions(storage, 'a', 'sig-1')).toEqual({ items: ITEMS });
    expect(loadSuggestions(storage, 'a', 'sig-x')).toBe('stale');
    forgetSuggestions(storage, 'a');
    expect(loadSuggestions(storage, 'a', 'sig-1')).toBeNull();
  });

  it('forgetAllSuggestions works on storage', () => {
    const storage = fakeStorage();
    saveSuggestions(storage, 'a', ITEMS, 'sig-1');
    saveDraft(storage, 'b', 'kept');
    forgetAllSuggestions(storage);
    expect(loadSuggestions(storage, 'a', 'sig-1')).toBeNull();
    expect(loadDraft(storage, 'b')).toBe('kept');
  });

  it('a stored suggestion list with the wrong shape is ignored, not shown', () => {
    const raw = JSON.stringify({ v: 1, entries: { a: { at: 1, suggestions: { items: [], sig: 's' } } } });
    expect(loadSuggestions(fakeStorage({ [CACHE_KEY]: raw }), 'a', 's')).toBeNull();
  });
});

describe('deleting conversations', () => {
  it('forgetConversation removes the draft and the suggestions of that conversation only', () => {
    const storage = fakeStorage();
    saveDraft(storage, 'a', 'gone');
    saveSuggestions(storage, 'a', ['x'], 's');
    saveDraft(storage, 'b', 'kept');
    forgetConversation(storage, 'a');
    expect(loadDraft(storage, 'a')).toBe('');
    expect(loadSuggestions(storage, 'a', 's')).toBeNull();
    expect(loadDraft(storage, 'b')).toBe('kept');
  });

  it('forgetEverything clears the whole cache, as delete-all does', () => {
    const storage = fakeStorage();
    saveDraft(storage, 'a', 'one');
    saveSuggestions(storage, 'b', ['x'], 's');
    forgetEverything(storage);
    expect(loadDraft(storage, 'a')).toBe('');
    expect(loadSuggestions(storage, 'b', 's')).toBeNull();
  });
});

describe('transcriptSig: live and rebuilt transcripts agree', () => {
  /** Plays the actions through the live reducer, the way the app builds the transcript while it runs. */
  const live = (actions: TranscriptAction[]) => actions.reduce(transcriptReducer, EMPTY_TRANSCRIPT).items;

  const messages = [
    { role: 'user', content: 'what is in the log?' },
    { role: 'assistant', content: 'The log has three lines.' },
    { role: 'user', content: 'and the last one?' },
    { role: 'assistant', content: '  The last one is an error.  ' },
  ];

  it('the same chat gives the same signature whether it was streamed live or rebuilt from the saved messages', () => {
    const fromDisk = itemsFromMessages(messages);
    const streamed = live([
      { type: 'user_sent', clientId: 1, text: 'what is in the log?' },
      { type: 'agent_event', event: { type: 'text_delta', text: 'The log has ' } },
      { type: 'agent_event', event: { type: 'text_delta', text: 'three lines.' } },
      { type: 'agent_event', event: { type: 'done' } },
      { type: 'user_sent', clientId: 2, text: 'and the last one?' },
      { type: 'agent_event', event: { type: 'text', text: '  The last one is an error.  ' } },
      { type: 'agent_event', event: { type: 'done' } },
    ]);
    expect(transcriptSig(streamed)).toBe(transcriptSig(fromDisk));
  });

  it('notices in the live transcript do not change the signature', () => {
    const plain = live([
      { type: 'user_sent', clientId: 1, text: 'hi' },
      { type: 'agent_event', event: { type: 'text', text: 'hello' } },
    ]);
    const withNotice = live([
      { type: 'user_sent', clientId: 1, text: 'hi' },
      { type: 'agent_event', event: { type: 'text', text: 'hello' } },
      { type: 'notice', text: 'Renamed the conversation to "x".', muted: true },
    ]);
    expect(transcriptSig(withNotice)).toBe(transcriptSig(plain));
  });

  it('a different reply, or one more message, gives a different signature', () => {
    const base = itemsFromMessages(messages);
    const other = itemsFromMessages([...messages.slice(0, 3), { role: 'assistant', content: 'A different answer.' }]);
    const longer = itemsFromMessages([...messages, { role: 'user', content: 'thanks' }]);
    expect(transcriptSig(other)).not.toBe(transcriptSig(base));
    expect(transcriptSig(longer)).not.toBe(transcriptSig(base));
  });

  it('blank agent text is not a message on either side', () => {
    const withBlank = live([
      { type: 'user_sent', clientId: 1, text: 'hi' },
      { type: 'agent_event', event: { type: 'text', text: '   ' } },
      { type: 'agent_event', event: { type: 'text', text: 'hello' } },
    ]);
    const disk = itemsFromMessages([
      { role: 'user', content: 'hi' },
      { role: 'assistant', content: '   ' },
      { role: 'assistant', content: 'hello' },
    ]);
    expect(transcriptSig(withBlank)).toBe(transcriptSig(disk));
  });

  it('an empty transcript has a stable signature', () => {
    expect(transcriptSig([])).toBe('0:-');
  });

  it('paging does not change the signature: the newest chunk, and the whole conversation once every chunk is loaded, agree', () => {
    const many = Array.from({ length: 100 }, (_, i) => [
      { role: 'user', content: `q${i}` },
      { role: 'assistant', content: `a${i}` },
    ]).flat();
    const whole = transcriptSig(itemsFromMessages(many));
    let paged = transcriptReducer(EMPTY_TRANSCRIPT, { type: 'conversation_loaded', messages: many });
    expect(paged.older.length).toBeGreaterThan(0);
    expect(transcriptSig(paged.items)).toBe(whole);
    while (paged.older.length > 0) {
      paged = transcriptReducer(paged, { type: 'older_loaded' });
    }
    expect(transcriptSig(paged.items)).toBe(whole);
  });
});

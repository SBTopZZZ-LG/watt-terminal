import { describe, expect, it } from 'vitest';
import { visibleEscapes, visiblePreviewEscapes } from './visibleEscapes';

describe('visibleEscapes', () => {
  it('leaves printable ASCII and newlines as they are', () => {
    const plain = 'ls -la /tmp | grep "x" && echo ~/a_b-c.d';
    expect(visibleEscapes(plain)).toBe(plain);
    expect(visibleEscapes('line one\nline two')).toBe('line one\nline two');
    expect(visibleEscapes('')).toBe('');
  });

  it('keeps the whole printable range', () => {
    let all = '';
    for (let c = 0x20; c <= 0x7e; c++) {
      all += String.fromCharCode(c);
    }
    expect(visibleEscapes(all)).toBe(all);
  });

  it('escapes tab and other control characters, without padding', () => {
    expect(visibleEscapes('a\tb')).toBe('a\\u{9}b');
    expect(visibleEscapes('a\rb')).toBe('a\\u{D}b');
    expect(visibleEscapes('\u0000')).toBe('\\u{0}');
    expect(visibleEscapes('\u001b[31m')).toBe('\\u{1B}[31m');
    expect(visibleEscapes('\u007f')).toBe('\\u{7F}');
    expect(visibleEscapes('\r\n')).toBe('\\u{D}\n');
  });

  it('escapes each bidi override and isolate', () => {
    for (const cp of [0x202a, 0x202b, 0x202c, 0x202d, 0x202e, 0x2066, 0x2067, 0x2068, 0x2069]) {
      expect(visibleEscapes(`a${String.fromCodePoint(cp)}b`)).toBe(`a\\u{${cp.toString(16).toUpperCase()}}b`);
    }
    expect(visibleEscapes('rm \u202eevil')).toBe('rm \\u{202E}evil');
  });

  it('escapes each zero-width and invisible format character', () => {
    for (const cp of [0x200b, 0x200c, 0x200d, 0x200e, 0x200f, 0x2060, 0xfeff]) {
      expect(visibleEscapes(`a${String.fromCodePoint(cp)}b`)).toBe(`a\\u{${cp.toString(16).toUpperCase()}}b`);
    }
  });

  it('escapes lookalike non-ASCII letters instead of showing them', () => {
    // Cyrillic a (U+0430) looks like Latin a.
    expect(visibleEscapes('p\u0430ss')).toBe('p\\u{430}ss');
    // Non-breaking space (U+00A0) looks like a space.
    expect(visibleEscapes('a\u00a0b')).toBe('a\\u{A0}b');
  });

  it('escapes emoji and CJK characters', () => {
    expect(visibleEscapes('\u4e2d')).toBe('\\u{4E2D}');
    expect(visibleEscapes('echo \u4e2d\u6587')).toBe('echo \\u{4E2D}\\u{6587}');
  });

  it('shows a surrogate pair as one code point', () => {
    expect(visibleEscapes('\u{1F600}')).toBe('\\u{1F600}');
    expect(visibleEscapes('x\u{1F600}y')).toBe('x\\u{1F600}y');
  });

  it('shows a lone surrogate as its own escape', () => {
    expect(visibleEscapes('a\ud83db')).toBe('a\\u{D83D}b');
  });
});

describe('visiblePreviewEscapes', () => {
  it('keeps printable ASCII, newline and tab', () => {
    let all = '';
    for (let c = 0x20; c <= 0x7e; c++) {
      all += String.fromCharCode(c);
    }
    expect(visiblePreviewEscapes(all)).toBe(all);
    expect(visiblePreviewEscapes('line one\n\tline two')).toBe('line one\n\tline two');
    expect(visiblePreviewEscapes('')).toBe('');
  });

  it('keeps readable non-ASCII: punctuation, letters, symbols, emoji, CJK', () => {
    expect(visiblePreviewEscapes('a — b')).toBe('a — b');
    expect(visiblePreviewEscapes('café')).toBe('café');
    expect(visiblePreviewEscapes('x → y')).toBe('x → y');
    expect(visiblePreviewEscapes('ok \u{1F600}')).toBe('ok \u{1F600}');
    expect(visiblePreviewEscapes('中文')).toBe('中文');
    expect(visiblePreviewEscapes('é')).toBe('é');
  });

  it('escapes bidi overrides, zero-width and BOM', () => {
    expect(visiblePreviewEscapes('rm ‮evil')).toBe('rm \\u{202E}evil');
    expect(visiblePreviewEscapes('a​b')).toBe('a\\u{200B}b');
    expect(visiblePreviewEscapes('﻿x')).toBe('\\u{FEFF}x');
  });

  it('escapes spaces other than plain space, and line separators', () => {
    expect(visiblePreviewEscapes('a b')).toBe('a\\u{A0}b');
    expect(visiblePreviewEscapes('a b')).toBe('a\\u{2003}b');
    expect(visiblePreviewEscapes('a b')).toBe('a\\u{2028}b');
    expect(visiblePreviewEscapes('a b')).toBe('a\\u{2029}b');
  });

  it('escapes carriage return, NUL and other controls', () => {
    expect(visiblePreviewEscapes('a\rb')).toBe('a\\u{D}b');
    expect(visiblePreviewEscapes('\u0000')).toBe('\\u{0}');
    expect(visiblePreviewEscapes('\u001b[31m')).toBe('\\u{1B}[31m');
    expect(visiblePreviewEscapes('\u0085')).toBe('\\u{85}');
  });

  it('escapes soft hyphen, private use and unassigned code points', () => {
    expect(visiblePreviewEscapes('a­b')).toBe('a\\u{AD}b');
    expect(visiblePreviewEscapes('ab')).toBe('a\\u{E000}b');
    expect(visiblePreviewEscapes('a͸b')).toBe('a\\u{378}b');
  });

  it('escapes a lone surrogate and keeps a surrogate pair whole', () => {
    expect(visiblePreviewEscapes('a\ud83db')).toBe('a\\u{D83D}b');
    expect(visiblePreviewEscapes('x\u{1F600}y')).toBe('x\u{1F600}y');
  });

  it('differs from visibleEscapes only on the characters it keeps', () => {
    expect(visibleEscapes('a — b\tc')).toBe('a \\u{2014} b\\u{9}c');
    expect(visiblePreviewEscapes('a — b\tc')).toBe('a — b\tc');
  });
});

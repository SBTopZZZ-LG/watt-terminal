import { describe, expect, it } from 'vitest';
import { visibleEscapes } from './visibleEscapes';

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

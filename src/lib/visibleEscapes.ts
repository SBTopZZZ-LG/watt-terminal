// Shows text so that every character on screen is one a reader can tell apart. Used for approval commands, where a
// lookalike or a right-to-left override must not hide what will run.

/**
 * Printable ASCII (0x20-0x7E) and newline stay as they are. Newline is a real line break; the approval text is pre-wrap.
 * Everything else becomes \u{HEX}, with the code point in uppercase hex and no padding: tab is \u{9}, a control
 * character is escaped, and so is every non-ASCII character. That last rule covers the format characters that can
 * reverse or hide text (bidi overrides U+202A-202E and U+2066-2069, zero-width U+200B-200F, U+2060, U+FEFF).
 * Iterating by code point makes a surrogate pair one escape (U+1F600 is \u{1F600}); a lone surrogate gets its own.
 * Backslash is printable ASCII, so it stays as it is; that means a typed "\u{41}" looks like an escape too.
 */
export function visibleEscapes(text: string): string {
  let out = '';
  for (const ch of text) {
    const cp = ch.codePointAt(0) ?? 0;
    if (cp === 0x0a || (cp >= 0x20 && cp <= 0x7e)) {
      out += ch;
    } else {
      out += `\\u{${cp.toString(16).toUpperCase()}}`;
    }
  }
  return out;
}

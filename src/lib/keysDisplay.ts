// The decoded input of a send_keys call, shown in the tool row and the approval card. The short forms follow core/keys.rs
// (KeyPlan::display): the text, then each key as ⏎ (Enter), ^C (Ctrl-C), a space (Space) or <Name> for the rest. Pure, so
// the wording, the escaping and the length cap are tested without the DOM.

import { visibleEscapes } from './visibleEscapes';

/** The short form of one key name the model sent. Names are matched without regard to case, as the backend does. */
function keyShort(name: string): string {
  const lower = name.toLowerCase();
  if (lower === 'enter') {
    return '⏎';
  }
  if (lower === 'space') {
    return ' ';
  }
  const ctrl = /^ctrl-([a-z])$/i.exec(name);
  if (ctrl) {
    return `^${ctrl[1].toUpperCase()}`;
  }
  return `<${name}>`;
}

/** The decoded input of a send_keys call's arguments ({text?, keys?}). Empty when there is nothing to show. */
export function sendKeysDisplay(args: unknown): string {
  if (typeof args !== 'object' || args === null) {
    return '';
  }
  const { text, keys } = args as { text?: unknown; keys?: unknown };
  let out = typeof text === 'string' ? text : '';
  if (Array.isArray(keys)) {
    for (const key of keys) {
      if (typeof key === 'string') {
        out += keyShort(key);
      }
    }
  }
  return out;
}

/**
 * The decoded input as it is shown on screen. Every character is escaped as visibleEscapes does (a lookalike or a bidi
 * override must not hide what is typed), except the Enter mark, which is ours and is not typed text.
 */
export function displayText(display: string): string {
  return display.split('⏎').map(visibleEscapes).join('⏎');
}

/**
 * The decoded input cut to `max` characters, shown as displayText shows it. The cut is made before escaping, so an escape is
 * never split, and the ellipsis that marks the cut is not escaped.
 */
export function capDisplay(display: string, max: number): string {
  const chars = [...display];
  if (chars.length <= max) {
    return displayText(display);
  }
  return `${displayText(chars.slice(0, max - 1).join(''))}…`;
}

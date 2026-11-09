// Parsing for the input bar. Text starting with `!` is a shell command (the `!` and one optional space are
// removed); anything else is a message for the agent.

import type { BarMode } from './inputState';

export type ParsedInput =
  | { kind: 'empty' }
  | { kind: 'shell'; command: string }
  | { kind: 'shell_invalid'; reason: string }
  | { kind: 'agent'; text: string };

export function parseBarInput(text: string): ParsedInput {
  if (text.trim() === '') {
    return { kind: 'empty' };
  }
  if (text.startsWith('!')) {
    let command = text.slice(1);
    if (command.startsWith(' ')) {
      command = command.slice(1);
    }
    if (command.trim() === '') {
      return { kind: 'shell_invalid', reason: 'Nothing to run after !' };
    }
    return { kind: 'shell', command };
  }
  return { kind: 'agent', text: text.trim() };
}

/** The mode indicator shown next to the input. */
export function modeLabel(text: string): 'shell' | 'agent' {
  return text.startsWith('!') ? 'shell' : 'agent';
}

export type SubmitDecision =
  | { kind: 'none' }
  | { kind: 'reject'; reason: string }
  | { kind: 'shell'; command: string }
  | { kind: 'agent'; text: string };

/**
 * Decides what Enter does. `offHint` is the route's hint, shown when the bar is off. Rust still checks the
 * prompt on submit; this check only gives the user the reason before anything is sent.
 */
export function decideSubmit(bar: BarMode, offHint: string | null, text: string): SubmitDecision {
  const parsed = parseBarInput(text);
  switch (parsed.kind) {
    case 'empty':
      return { kind: 'none' };
    case 'shell_invalid':
      return { kind: 'reject', reason: parsed.reason };
    case 'shell':
      if (bar === 'full') {
        return { kind: 'shell', command: parsed.command };
      }
      return { kind: 'reject', reason: bar === 'off' ? (offHint ?? 'Input is off.') : 'Shell commands need an idle prompt.' };
    case 'agent':
      if (bar === 'full' || bar === 'agent_only') {
        return { kind: 'agent', text: parsed.text };
      }
      return { kind: 'reject', reason: offHint ?? 'Input is off.' };
  }
}

/**
 * Up/Down history in memory. `index` equals `entries.length` when showing the live line, whose text is kept
 * as the draft when the user starts walking back.
 */
export class History {
  private entries: string[] = [];
  private index = 0;
  private draft = '';

  /** Records a line that was sent, and returns to the live line. */
  record(text: string): void {
    if (text.trim() !== '' && this.entries[this.entries.length - 1] !== text) {
      this.entries.push(text);
    }
    this.index = this.entries.length;
    this.draft = '';
  }

  /** Steps back from `current`. Returns the older entry, or null when there is none. */
  back(current: string): string | null {
    if (this.index === 0) {
      return null;
    }
    if (this.index === this.entries.length) {
      this.draft = current;
    }
    this.index -= 1;
    return this.entries[this.index];
  }

  /** Steps forward. Returns the newer entry, the saved draft past the newest, or null when already live. */
  forward(): string | null {
    if (this.index >= this.entries.length) {
      return null;
    }
    this.index += 1;
    return this.index === this.entries.length ? this.draft : this.entries[this.index];
  }
}

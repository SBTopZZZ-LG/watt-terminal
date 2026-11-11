// Input bar text rules. The bar sends agent messages only, so every line is text for the agent (a leading `!` has no
// meaning here). Shell input is typed in the terminal.

/** The message to send for the bar's text: trimmed, or null when there is nothing to send. */
export function messageFor(text: string): string | null {
  const trimmed = text.trim();
  return trimmed === '' ? null : trimmed;
}

/**
 * Whether an Up or Down press takes history, rather than moving the caret inside a multi-line text. Up takes it only
 * when the caret is on the first line, Down only on the last. Single-line text always takes it.
 */
export function historyKeyTakesLine(dir: 'up' | 'down', text: string, caret: number): boolean {
  return dir === 'up' ? !text.slice(0, caret).includes('\n') : !text.slice(caret).includes('\n');
}

/**
 * The send button's state. While the agent is busy it is Stop, which interrupts. Otherwise it sends, and is disabled
 * when there is nothing to send or the bar is locked. Stop is disabled only when locked.
 */
export function sendButtonFor(busy: boolean, text: string, locked: boolean): { stop: boolean; disabled: boolean } {
  return { stop: busy, disabled: locked || (!busy && messageFor(text) === null) };
}

/**
 * Up/Down history of agent messages in memory. `index` equals `entries.length` when showing the live line, whose text
 * is kept as the draft when the user starts walking back.
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

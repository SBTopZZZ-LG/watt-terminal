// Input bar text rules. The bar sends agent messages only, so every line is text for the agent (a leading `!` has no
// meaning here). Shell input is typed in the terminal.

/** The message to send for the bar's text: trimmed, or null when there is nothing to send. */
export function messageFor(text: string): string | null {
  const trimmed = text.trim();
  return trimmed === '' ? null : trimmed;
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

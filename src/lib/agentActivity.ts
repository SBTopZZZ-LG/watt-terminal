// The hidden-agent activity text: the one-line summary shown while the agent works and the chat body is hidden.
// Pure, so the wording and the caps are tested without the DOM.

import { toolCaption, toolCaptionFull } from './toolDisplay';
import type { TranscriptItem } from './transcript';

/** An agent message keeps at most this many characters; the cut is marked with an ellipsis. */
export const MAX_ACTIVITY_CHARS = 160;

/** The activity line as shown, and the same line whole (the tooltip's text, when the shown one is cut). */
export interface ActivityLine {
  text: string;
  full: string;
}

/**
 * The activity line: the newest non-empty agent message first (whitespace collapsed, capped at ~160 characters),
 * else the newest run_command/send_keys tool caption as `<name> · <caption>`. With neither, `Working…` while a side
 * command runs, `Thinking…` while the model has not produced output yet, else `Working…`.
 * Only the current turn counts: items after the last user message. A reply from an earlier turn is not shown.
 */
export function agentActivity(allItems: TranscriptItem[], thinking: boolean, sideBusy: boolean): ActivityLine {
  const lastUser = allItems.map((i) => i.kind).lastIndexOf('user');
  const items = allItems.slice(lastUser + 1);
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i];
    if (item.kind === 'agent') {
      const collapsed = item.text.replace(/\s+/g, ' ').trim();
      if (collapsed !== '') {
        const chars = [...collapsed];
        const text = chars.length > MAX_ACTIVITY_CHARS ? `${chars.slice(0, MAX_ACTIVITY_CHARS - 1).join('')}…` : collapsed;
        return { text, full: collapsed };
      }
    }
  }
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i];
    if (item.kind === 'tool' && (item.name === 'run_command' || item.name === 'send_keys')) {
      const caption = toolCaption(item.args);
      if (caption !== null) {
        return { text: `${item.name} · ${caption}`, full: `${item.name} · ${toolCaptionFull(item.args) ?? caption}` };
      }
    }
  }
  const label = sideBusy ? 'Working…' : thinking ? 'Thinking…' : 'Working…';
  return { text: label, full: label };
}

/** The activity line as shown (see agentActivity). */
export function agentActivityText(items: TranscriptItem[], thinking: boolean, sideBusy: boolean): string {
  return agentActivity(items, thinking, sideBusy).text;
}

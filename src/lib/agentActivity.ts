// The hidden-agent activity text: the one-line summary shown while the agent works and the chat body is hidden.
// Pure, so the wording and the caps are tested without the DOM.

import { toolCaption } from './toolDisplay';
import type { TranscriptItem } from './transcript';

/** An agent message keeps at most this many characters; the cut is marked with an ellipsis. */
export const MAX_ACTIVITY_CHARS = 160;

/**
 * The activity line: the newest non-empty agent message first (whitespace collapsed, capped at ~160 characters),
 * else the newest run_command/send_keys tool caption as `<name> · <caption>`. With neither, `Working…` while a side
 * command runs, `Thinking…` while the model has not produced output yet, else `Working…`.
 */
export function agentActivityText(items: TranscriptItem[], thinking: boolean, sideBusy: boolean): string {
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i];
    if (item.kind === 'agent') {
      const collapsed = item.text.replace(/\s+/g, ' ').trim();
      if (collapsed !== '') {
        const chars = [...collapsed];
        return chars.length > MAX_ACTIVITY_CHARS ? `${chars.slice(0, MAX_ACTIVITY_CHARS - 1).join('')}…` : collapsed;
      }
    }
  }
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i];
    if (item.kind === 'tool' && (item.name === 'run_command' || item.name === 'send_keys')) {
      const caption = toolCaption(item.args);
      if (caption !== null) {
        return `${item.name} · ${caption}`;
      }
    }
  }
  if (sideBusy) {
    return 'Working…';
  }
  if (thinking) {
    return 'Thinking…';
  }
  return 'Working…';
}

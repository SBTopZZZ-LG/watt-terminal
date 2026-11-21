// What to tell the user when the agent needs them while Watt is in the background: a turn ended, a question is waiting,
// or an approval is waiting. Pure: the decision only. Sending the notification and flashing the window is in attentionClient.ts.

/** Longest body shown in the notification, in characters. */
export const ATTENTION_BODY_MAX = 120;

export type AttentionEvent =
  /** A turn ended. `lastText` is the agent's last reply, or null when it has none. */
  | { type: 'turn_idle'; lastText: string | null }
  /** The agent asked a question. `text` is the first question's text. */
  | { type: 'question'; text: string }
  /** The agent needs approval for a call. `text` is the command or summary the approval card shows. */
  | { type: 'approval'; text: string };

export interface AttentionContext {
  /** The notifications setting is on. */
  enabled: boolean;
  /** The Watt window has focus (native focus state). */
  focused: boolean;
  /** Messages waiting in the queue. A finished turn with a queue is not idle: the queue is about to send. */
  queueLength: number;
}

export interface AttentionDecision {
  kind: 'idle' | 'question' | 'approval';
  title: string;
  body: string;
  /** Question and approval flash until focused (critical); a finished turn flashes once (informational). */
  attention: 'critical' | 'informational';
}

/**
 * The notification for an event, or null when there is none: the setting is off, the window has focus, or a turn ended
 * with messages still queued.
 */
export function decideAttention(event: AttentionEvent, ctx: AttentionContext): AttentionDecision | null {
  if (!ctx.enabled || ctx.focused) {
    return null;
  }
  switch (event.type) {
    case 'turn_idle':
      if (ctx.queueLength > 0) {
        return null;
      }
      return {
        kind: 'idle',
        title: 'Watt · done',
        body: bodyOr(event.lastText, 'The agent has finished.'),
        attention: 'informational',
      };
    case 'question':
      return {
        kind: 'question',
        title: 'Watt · the agent has a question',
        body: bodyOr(event.text, 'Open Watt to answer.'),
        attention: 'critical',
      };
    case 'approval':
      return {
        kind: 'approval',
        title: 'Watt · approval needed',
        body: bodyOr(event.text, 'Open Watt to review the command.'),
        attention: 'critical',
      };
  }
}

/** The text as one short line, or the fallback when it has no text. */
function bodyOr(text: string | null, fallback: string): string {
  const line = clip(text ?? '');
  return line === '' ? fallback : line;
}

/** Whitespace collapsed to single spaces, trimmed, and cut to ATTENTION_BODY_MAX characters (code points) with an ellipsis. */
export function clip(text: string): string {
  const line = text.replace(/\s+/g, ' ').trim();
  const chars = Array.from(line);
  if (chars.length <= ATTENTION_BODY_MAX) {
    return line;
  }
  return `${chars.slice(0, ATTENTION_BODY_MAX - 1).join('').trimEnd()}…`;
}

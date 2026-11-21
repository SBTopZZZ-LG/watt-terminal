// Follow-up prompt suggestions: when to ask the model for them after a reply, and when a result may be shown. Pure, so the
// rules are tested without React or the backend. App.tsx makes the request, keeps the generation, and shows the list.

import type { TranscriptItem } from './transcript';

/** What the trigger needs at the moment a turn or side command has just ended. */
export interface SuggestTrigger {
  /** The suggest_prompts setting is on. */
  enabled: boolean;
  /** Busy (a turn or a side command) went from true to false in this render. */
  turnJustEnded: boolean;
  /** Messages still queued: a drain starts a new turn for them, so this edge asks for nothing. */
  queueLength: number;
  sideBusy: boolean;
  /** The reply the suggestions follow: the last transcript item is agent text. */
  lastItemIsAgentText: boolean;
}

/** Whether a finished turn should ask for suggestions. */
export function shouldSuggest(t: SuggestTrigger): boolean {
  return t.enabled && t.turnJustEnded && t.queueLength === 0 && !t.sideBusy && t.lastItemIsAgentText;
}

/** Whether the transcript ends with an agent reply that has some text. */
export function endsWithAgentText(items: TranscriptItem[]): boolean {
  const last = items[items.length - 1];
  return last !== undefined && last.kind === 'agent' && last.text.trim() !== '';
}

/**
 * The suggestions to show for a finished request. None when the request is stale (the conversation or the setting moved on
 * while it ran, `current` false) or the agent is busy again by the time it answers.
 */
export function acceptSuggestions(result: string[], state: { current: boolean; busy: boolean }): string[] {
  return state.current && !state.busy ? result : [];
}

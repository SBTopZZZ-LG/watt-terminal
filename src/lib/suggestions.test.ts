import { describe, expect, it } from 'vitest';
import { acceptSuggestions, endsWithAgentText, shouldSuggest } from './suggestions';
import type { SuggestTrigger } from './suggestions';
import type { TranscriptItem } from './transcript';

const READY: SuggestTrigger = { enabled: true, turnJustEnded: true, queueLength: 0, sideBusy: false, lastItemIsAgentText: true };

describe('shouldSuggest', () => {
  it('asks when the setting is on, a turn just ended with an agent reply, and nothing is queued or side-running', () => {
    expect(shouldSuggest(READY)).toBe(true);
  });

  it('asks for nothing while the setting is off', () => {
    expect(shouldSuggest({ ...READY, enabled: false })).toBe(false);
  });

  it('asks for nothing when no turn just ended', () => {
    expect(shouldSuggest({ ...READY, turnJustEnded: false })).toBe(false);
  });

  it('asks for nothing while queued messages will start the next turn', () => {
    expect(shouldSuggest({ ...READY, queueLength: 1 })).toBe(false);
  });

  it('asks for nothing while a side command runs', () => {
    expect(shouldSuggest({ ...READY, sideBusy: true })).toBe(false);
  });

  it('asks for nothing when the turn ended without an agent reply to follow', () => {
    expect(shouldSuggest({ ...READY, lastItemIsAgentText: false })).toBe(false);
  });
});

describe('endsWithAgentText', () => {
  const agent = (text: string): TranscriptItem => ({ id: 1, kind: 'agent', text, streaming: false });
  const notice: TranscriptItem = { id: 2, kind: 'notice', text: 'Interrupted.' };

  it('is true when the last item is agent text', () => {
    expect(endsWithAgentText([notice, agent('Done.')])).toBe(true);
  });

  it('is false for an empty reply, a whitespace-only reply, or no items', () => {
    expect(endsWithAgentText([agent('')])).toBe(false);
    expect(endsWithAgentText([agent('  \n ')])).toBe(false);
    expect(endsWithAgentText([])).toBe(false);
  });

  it('is false when the last item is not agent text, even if an earlier one is', () => {
    expect(endsWithAgentText([agent('Done.'), notice])).toBe(false);
  });
});

describe('acceptSuggestions', () => {
  const LIST = ['Run the tests', 'Show the diff'];

  it('shows the list when the request is current and the agent is idle', () => {
    expect(acceptSuggestions(LIST, { current: true, busy: false })).toEqual(LIST);
  });

  it('drops a stale result: the conversation or the setting moved on while it ran', () => {
    expect(acceptSuggestions(LIST, { current: false, busy: false })).toEqual([]);
  });

  it('drops a result that arrives after the agent is busy again', () => {
    expect(acceptSuggestions(LIST, { current: true, busy: true })).toEqual([]);
  });

  it('shows nothing for an empty list', () => {
    expect(acceptSuggestions([], { current: true, busy: false })).toEqual([]);
  });
});

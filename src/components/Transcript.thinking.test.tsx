import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { activityLabel, EMPTY_TRANSCRIPT, transcriptReducer } from '../lib/transcript';
import type { TranscriptAction } from '../lib/transcript';
import { Transcript } from './Transcript';

function render(actions: TranscriptAction[], busy: boolean): string {
  const t = actions.reduce(transcriptReducer, EMPTY_TRANSCRIPT);
  return renderToStaticMarkup(
    <Transcript
      items={t.items}
      busy={busy}
      thinking={t.thinking}
      open
      onToggle={() => {}}
      onApprove={() => {}}
      onAnswerQuestions={() => {}}
      onNewConversation={() => {}}
    />,
  );
}

const reasoning = (text: string): TranscriptAction => ({ type: 'agent_event', event: { type: 'reasoning_delta', text } });
const thinking = (active: boolean): TranscriptAction => ({ type: 'agent_event', event: { type: 'thinking', active } });

describe('Transcript: reasoning and the activity line', () => {
  it('a streamed reasoning block shows in the body', () => {
    const out = render([thinking(true), reasoning('Checking the cwd')], true);
    expect(out).toContain('Checking the cwd');
    expect(out).toContain('Thinking…');
  });

  it('the activity line is shown only while busy', () => {
    const busy = render([thinking(true)], true);
    expect(busy).toMatch(/class="agent-activity" role="status"/);
    expect(busy).toContain('<span>Thinking</span>');
    expect(render([], false)).not.toContain('agent-activity');
  });

  it('the old header thinking marker is gone', () => {
    expect(render([thinking(true)], true)).not.toContain('thinking...');
    expect(activityLabel(EMPTY_TRANSCRIPT)).toBe('Working');
  });
});

import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { EMPTY_TRANSCRIPT, transcriptReducer } from '../lib/transcript';
import type { TranscriptAction } from '../lib/transcript';
import { Transcript } from './Transcript';

function render(actions: TranscriptAction[]): string {
  const t = actions.reduce(transcriptReducer, EMPTY_TRANSCRIPT);
  return renderToStaticMarkup(
    <Transcript
      items={t.items}
      busy={false}
      thinking={false}
      open
      onToggle={() => {}}
      onApprove={() => {}}
      onNewConversation={() => {}}
    />,
  );
}

const delta = (text: string): TranscriptAction => ({ type: 'agent_event', event: { type: 'text_delta', text } });
const done: TranscriptAction = { type: 'agent_event', event: { type: 'done' } };
const text = (t: string): TranscriptAction => ({ type: 'agent_event', event: { type: 'text', text: t } });

const COPY = 'aria-label="Copy as Markdown"';

describe('Transcript: copy button', () => {
  it('is not shown while the agent message is still streaming', () => {
    const html = render([delta('partial reply')]);
    expect(html).toContain('partial reply');
    expect(html).not.toContain(COPY);
  });

  it('is shown once the streamed message is complete', () => {
    const html = render([delta('full '), delta('reply'), done]);
    expect(html).toContain(COPY);
    expect(html).toContain('title="Copy as Markdown"');
  });

  it('is shown on a message that arrived as one text event', () => {
    const html = render([text('one shot')]);
    expect(html.split(COPY)).toHaveLength(2);
  });

  it('is not shown on user messages', () => {
    const html = render([{ type: 'user_sent', clientId: 1, text: 'hello' }]);
    expect(html).not.toContain(COPY);
  });
});

describe('Transcript: tool groups', () => {
  it('a single tool call with a resolved approval renders as a group, with its rows inside the indented block', () => {
    const html = render([
      { type: 'agent_event', event: { type: 'tool_call', call_id: 'c1', name: 'run_command', args: {} } },
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'c1', name: 'run_command', text: 'ls' } },
      {
        type: 'agent_event',
        event: { type: 'tool_result', call_id: 'c1', name: 'run_command', ok: true, decision: 'manual', result: {} },
      },
    ]);
    expect(html).toContain('1 tool call: run_command');
    expect(html).toContain('class="tool-group-items"');
    expect(html.indexOf('class="tool-group-items"')).toBeLessThan(html.indexOf('class="approval approval-resolved"'));
  });
});

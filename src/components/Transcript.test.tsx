import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { EMPTY_TRANSCRIPT, transcriptReducer } from '../lib/transcript';
import type { TranscriptAction } from '../lib/transcript';
import { JumpToLatest, Transcript } from './Transcript';

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

describe('Transcript: jump to latest', () => {
  // The scroll behaviour needs a DOM (no jsdom here), so only the initial render is checked: the button starts hidden.
  it('is not shown on the initial render, even with a long conversation', () => {
    const html = render([delta('a'.repeat(50)), done, text('more'), { type: 'user_sent', clientId: 2, text: 'next' }]);
    expect(html).not.toContain('jump-to-latest');
  });

  it('says Scroll to bottom when the agent is idle', () => {
    const html = renderToStaticMarkup(<JumpToLatest busy={false} onClick={() => {}} />);
    expect(html).toContain('aria-label="Scroll to bottom"');
    expect(html).toContain('type="button"');
  });

  it('says Follow the agent while it is busy', () => {
    const html = renderToStaticMarkup(<JumpToLatest busy onClick={() => {}} />);
    expect(html).toContain('aria-label="Follow the agent (resume auto-scroll)"');
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

describe('Transcript: history button', () => {
  it('has the id App focuses when the History view closes, and is pressed while the view is shown', () => {
    const html = renderToStaticMarkup(
      <Transcript
        items={[]}
        busy={false}
        thinking={false}
        open
        onToggle={() => {}}
        onApprove={() => {}}
        onNewConversation={() => {}}
        view="history"
        onToggleHistory={() => {}}
        history={<div>history</div>}
      />,
    );
    expect(html).toContain('id="history-button"');
    expect(html).toMatch(/id="history-button"[^>]*aria-pressed="true"|aria-pressed="true"[^>]*id="history-button"/);
  });
});

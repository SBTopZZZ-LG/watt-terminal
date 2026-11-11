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

describe('Transcript: todo panel', () => {
  const todos = [{ content: 'plan', status: 'in_progress' as const }];
  const base = { items: [], busy: false, thinking: false, onToggle: () => {}, onApprove: () => {}, onNewConversation: () => {} };

  it('is pinned above the scrolling body, not inside it', () => {
    const html = renderToStaticMarkup(<Transcript {...base} todos={todos} open />);
    const panel = html.indexOf('class="todo-panel');
    expect(panel).toBeGreaterThan(-1);
    expect(panel).toBeLessThan(html.indexOf('class="transcript-body"'));
  });

  it('is hidden when the list is empty, and when the chat body is not shown', () => {
    expect(renderToStaticMarkup(<Transcript {...base} todos={[]} open />)).not.toContain('todo-panel');
    expect(renderToStaticMarkup(<Transcript {...base} todos={todos} open={false} />)).not.toContain('todo-panel');
    expect(renderToStaticMarkup(<Transcript {...base} todos={todos} open view="history" history={<div />} />)).not.toContain('todo-panel');
  });

  it('todo_write rows never render as tool rows', () => {
    // Run through the reducer, as the view gets them: the reducer drops the call and its result.
    const t = [
      { type: 'user_sent' as const, clientId: 1, text: 'plan' },
      { type: 'agent_event' as const, event: { type: 'tool_call' as const, call_id: 't1', name: 'todo_write', args: {} } },
      { type: 'agent_event' as const, event: { type: 'tool_result' as const, call_id: 't1', name: 'todo_write', ok: true, decision: 'read' as const, result: {} } },
    ].reduce(transcriptReducer, EMPTY_TRANSCRIPT);
    const html = renderToStaticMarkup(<Transcript {...base} items={t.items} open />);
    expect(html).toContain('plan');
    expect(html).not.toContain('todo_write');
    expect(html).not.toContain('class="tool');
  });
});

describe('Transcript: approval reasons', () => {
  const callAndResult = (decision: 'auto' | 'manual' | 'denied', reason?: 'rule' | 'gate_chars') => [
    { type: 'agent_event' as const, event: { type: 'tool_call' as const, call_id: 'c1', name: 'run_command', args: {} } },
    {
      type: 'agent_event' as const,
      event: { type: 'tool_result' as const, call_id: 'c1', name: 'run_command', ok: decision !== 'denied', decision, ...(reason ? { reason } : {}), result: {} },
    },
  ];

  it('an automatic run shows its reason in the label, and why in the tooltip', () => {
    const html = render(callAndResult('auto', 'rule'));
    expect(html).toContain('auto · rule');
    expect(html).toContain('title="Ran without asking: an allow rule matches this command."');
  });

  it('an approved call keeps the label "manual", and its tooltip says why it asked', () => {
    const html = render(callAndResult('manual', 'gate_chars'));
    expect(html).toContain('decision-manual');
    expect(html).not.toContain('manual · ');
    expect(html).toContain('title="Asked because the command has a character outside letters, digits, spaces and _-./:=@+, so your rules do not apply to it."');
  });

  it('a call with no reason has the bare label and no tooltip', () => {
    const html = render(callAndResult('auto'));
    expect(html).toContain('decision-auto');
    expect(html).not.toContain('auto · ');
    expect(html).not.toContain('title="Ran without');
  });

  it('the approval card says why it asks, in one muted line', () => {
    const html = render([
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'c1', name: 'run_command', text: 'ls | wc', reason: 'gate_chars' } },
    ]);
    expect(html).toContain('<div class="approval-reason">Asked because the command has a character outside');
  });

  it('an approval card without a reason has no reason line', () => {
    const html = render([{ type: 'agent_event', event: { type: 'approval_request', call_id: 'c1', name: 'send_keys', text: 'ls' } }]);
    expect(html).toContain('class="approval approval-pending"');
    expect(html).not.toContain('approval-reason');
  });
});

describe('Transcript: a request_integration row', () => {
  const callIntegrate: TranscriptAction = {
    type: 'agent_event',
    event: { type: 'tool_call', call_id: 'c1', name: 'request_integration', args: {} },
  };

  it('a declined request is a tool row with the status declined, not failed and not denied', () => {
    const html = render([
      callIntegrate,
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'c1', name: 'request_integration', ok: false, decision: 'denied', result: 'declined: the user did not integrate this shell' } },
    ]);
    expect(html).toContain('class="tool tool-declined"');
    expect(html).toContain('request_integration');
    expect(html).toContain(': declined</summary>');
    expect(html).not.toContain('decision-denied');
    expect(html).not.toContain('tool-failed');
  });

  it('an integrated request is a plain ok row, like the other tool rows', () => {
    const html = render([
      callIntegrate,
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'c1', name: 'request_integration', ok: true, decision: 'manual', result: 'Integrated ssh ubuntu@lidia (zsh).' } },
    ]);
    expect(html).toContain('class="tool tool-ok"');
    expect(html).toContain('decision decision-manual');
    expect(html).toContain(': ok</summary>');
  });

  it('a request run without a dialog (auto) shows auto, like any other tool', () => {
    const html = render([
      callIntegrate,
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'c1', name: 'request_integration', ok: true, decision: 'auto', result: 'x' } },
    ]);
    expect(html).toContain('auto');
  });
});
